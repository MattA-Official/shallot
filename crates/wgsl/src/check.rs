//! Name resolution, type checking and lowering from the syntax tree to [`ir`].

use crate::ast::{self, BinaryOp, ExprKind as A, StmtKind as S, UnaryOp};
use crate::consts::{self, ConstValue};
use crate::ir::{self, BuiltinFn, Expr, ExprKind, IoBinding, Stage, Stmt};
use crate::types::*;
use crate::{Error, Span};
use std::collections::{BTreeSet, HashMap};

type Result<T> = std::result::Result<T, Error>;

/// Checks a parsed module and lowers it to the typed IR.
pub fn check(module: &ast::Module) -> Result<ir::Module> {
    let mut checker = Checker::new(module)?;
    for i in 0..module.decls.len() {
        checker.resolve_decl(i)?;
    }
    checker.finish()
}

#[derive(Clone)]
enum Resolved {
    Struct(usize),
    Alias(Type),
    Const(ConstValue, Type),
    Global(usize),
    Function(usize),
    Assertion,
}

#[derive(Clone)]
enum DeclState {
    Pending,
    InProgress,
    Done(Resolved),
}

#[derive(Clone)]
enum LocalBinding {
    Local(usize),
    Param(usize),
    Const(ConstValue, Type),
}

struct FnCtx {
    scopes: Vec<HashMap<String, LocalBinding>>,
    params: Vec<ir::Param>,
    locals: Vec<ir::Local>,
    ret: Type,
    globals: BTreeSet<usize>,
    calls: BTreeSet<usize>,
    loops: u32,
    breakable: u32,
}

struct Checker<'a> {
    ast: &'a ast::Module,
    names: HashMap<&'a str, usize>,
    states: Vec<DeclState>,
    module: ir::Module,
    func: Option<FnCtx>,
    /// AST declaration of each IR function, for entry-point attributes.
    function_decls: Vec<usize>,
    /// Functions whose bodies are being checked, innermost last.
    active: Vec<usize>,
}

fn err<T>(message: impl Into<String>, span: Span) -> Result<T> {
    Err(Error::new(message, span))
}

fn has_attr<'b>(attrs: &'b [ast::Attribute], name: &str) -> Option<&'b ast::Attribute> {
    attrs.iter().find(|a| a.name == name)
}

/// Same shape, scalar ignored: `vec3<f32>` and `vec3<abstract-int>` match.
fn same_shape(a: &Type, b: &Type) -> bool {
    a.with_scalar(Scalar::Bool) == b.with_scalar(Scalar::Bool)
}

fn vector_or_scalar(n: Option<u8>, s: Scalar) -> Type {
    match n {
        Some(n) => Type::Vector(n, s),
        None => Type::Scalar(s),
    }
}

impl<'a> Checker<'a> {
    fn new(ast: &'a ast::Module) -> Result<Self> {
        let mut names = HashMap::new();
        for (i, decl) in ast.decls.iter().enumerate() {
            let (name, span) = match decl {
                ast::Decl::Struct(d) => (d.name.as_str(), d.span),
                ast::Decl::Fn(d) => (d.name.as_str(), d.span),
                ast::Decl::Var(d) => (d.name.as_str(), d.span),
                ast::Decl::Const(d) | ast::Decl::Override(d) => (d.name.as_str(), d.span),
                ast::Decl::Alias { name, span, .. } => (name.as_str(), *span),
                ast::Decl::ConstAssert(_) => continue,
            };
            if names.insert(name, i).is_some() {
                return err(format!("redeclaration of `{name}`"), span);
            }
        }
        Ok(Checker {
            ast,
            names,
            states: vec![DeclState::Pending; ast.decls.len()],
            module: ir::Module {
                enables: ast.enables.clone(),
                ..Default::default()
            },
            func: None,
            function_decls: Vec::new(),
            active: Vec::new(),
        })
    }

    fn structs(&self) -> &[StructType] {
        &self.module.structs
    }

    fn type_name(&self, ty: &Type) -> String {
        ty.display(&self.module.structs).to_string()
    }

    // ---------------------------------------------------------------------------------------
    // Module scope

    fn resolve_decl(&mut self, i: usize) -> Result<Resolved> {
        match &self.states[i] {
            DeclState::Done(r) => return Ok(r.clone()),
            DeclState::InProgress => {
                return err("declaration depends on itself", self.decl_span(i));
            }
            DeclState::Pending => {}
        }
        self.states[i] = DeclState::InProgress;
        // Declarations resolve outside any function, even when reached from a function body.
        let saved = self.func.take();
        let result = self.resolve_decl_inner(i);
        self.func = saved;
        let resolved = result?;
        self.states[i] = DeclState::Done(resolved.clone());
        Ok(resolved)
    }

    fn decl_span(&self, i: usize) -> Span {
        match &self.ast.decls[i] {
            ast::Decl::Struct(d) => d.span,
            ast::Decl::Fn(d) => d.span,
            ast::Decl::Var(d) => d.span,
            ast::Decl::Const(d) | ast::Decl::Override(d) => d.span,
            ast::Decl::Alias { span, .. } => *span,
            ast::Decl::ConstAssert(e) => e.span,
        }
    }

    fn resolve_decl_inner(&mut self, i: usize) -> Result<Resolved> {
        let decl = &self.ast.decls[i];
        match decl {
            ast::Decl::Struct(d) => self.struct_decl(d),
            ast::Decl::Alias { ty, .. } => Ok(Resolved::Alias(self.resolve_type(ty)?)),
            ast::Decl::Const(d) => {
                let Some(init) = &d.init else {
                    return err("`const` declarations need an initializer", d.span);
                };
                let value = self.expr(init)?;
                let value = match &d.ty {
                    Some(ty) => {
                        let ty = self.resolve_type(ty)?;
                        self.coerce(value, &ty, init.span)?
                    }
                    None => self.value(value),
                };
                match value.kind {
                    ExprKind::Const(v) => Ok(Resolved::Const(v, value.ty)),
                    _ => err(
                        "`const` initializers must be constant expressions",
                        init.span,
                    ),
                }
            }
            ast::Decl::Override(d) => err("`override` declarations are not supported yet", d.span),
            ast::Decl::Var(d) => self.global_var(d),
            ast::Decl::Fn(d) => self.function(i, d),
            ast::Decl::ConstAssert(e) => {
                self.const_assert(e)?;
                Ok(Resolved::Assertion)
            }
        }
    }

    fn const_assert(&mut self, e: &ast::Expr) -> Result<()> {
        let value = self.expr(e)?;
        let value = self.coerce(value, &BOOL, e.span)?;
        match value.as_const() {
            Some(ConstValue::Bool(true)) => Ok(()),
            Some(_) => err("const assertion failed", e.span),
            None => err("`const_assert` needs a constant expression", e.span),
        }
    }

    fn const_u32(&mut self, e: &ast::Expr, what: &str) -> Result<u32> {
        let value = self.expr(e)?;
        let value = self.value(value);
        match (&value.kind, value.ty.scalar()) {
            (ExprKind::Const(ConstValue::Int(v)), Some(s))
                if s.is_integer() && *v >= 0 && *v <= u32::MAX as i64 =>
            {
                Ok(*v as u32)
            }
            _ => err(
                format!("{what} must be a non-negative constant integer"),
                e.span,
            ),
        }
    }

    fn struct_decl(&mut self, d: &ast::StructDecl) -> Result<Resolved> {
        if d.members.is_empty() {
            return err("structs need at least one member", d.span);
        }
        let mut members = Vec::new();
        let mut explicit = Vec::new();
        for (index, m) in d.members.iter().enumerate() {
            if d.members[..index].iter().any(|other| other.name == m.name) {
                return err(format!("duplicate member `{}`", m.name), m.span);
            }
            let ty = self.resolve_type(&m.ty)?;
            let runtime = matches!(ty, Type::Array { count: None, .. });
            if runtime && index + 1 != d.members.len() {
                return err(
                    "runtime-sized arrays must be a struct's last member",
                    m.span,
                );
            }
            let align = match has_attr(&m.attrs, "align") {
                Some(a) => Some(self.attr_u32(a)?),
                None => None,
            };
            let size = match has_attr(&m.attrs, "size") {
                Some(a) => Some(self.attr_u32(a)?),
                None => None,
            };
            let io = self.io_attrs(&m.attrs, &ty, m.span)?;
            let (builtin, location, interpolation) = match io {
                Some(IoBinding::Builtin(b)) => (Some(b), None, None),
                Some(IoBinding::Location {
                    location,
                    interpolation,
                }) => (None, Some(location), interpolation),
                None => (None, None, None),
            };
            members.push(StructMember {
                name: m.name.clone(),
                ty,
                offset: 0,
                builtin,
                location,
                interpolation,
            });
            explicit.push((align, size));
        }
        let (align, size) = layout_struct(&mut members, &explicit, self.structs());
        self.module.structs.push(StructType {
            name: d.name.clone(),
            members,
            align,
            size,
        });
        Ok(Resolved::Struct(self.module.structs.len() - 1))
    }

    fn attr_u32(&mut self, attr: &ast::Attribute) -> Result<u32> {
        match attr.args.as_slice() {
            [arg] => self.const_u32(arg, &format!("@{}", attr.name)),
            _ => err(format!("@{} takes one argument", attr.name), attr.span),
        }
    }

    fn attr_word<'b>(&self, attr: &'b ast::Attribute, index: usize) -> Result<&'b str> {
        match attr.args.get(index).map(|a| &a.kind) {
            Some(A::Ident(id)) if id.template.is_empty() => Ok(&id.name),
            _ => err(format!("@{} expects a name here", attr.name), attr.span),
        }
    }

    fn io_attrs(
        &mut self,
        attrs: &[ast::Attribute],
        ty: &Type,
        span: Span,
    ) -> Result<Option<IoBinding>> {
        if let Some(attr) = has_attr(attrs, "builtin") {
            let name = self.attr_word(attr, 0)?;
            let Some(builtin) = Builtin::from_name(name) else {
                return err(format!("unknown builtin `{name}`"), attr.span);
            };
            if builtin.ty() != *ty {
                return err(
                    format!(
                        "builtin `{name}` must have type {}",
                        self.type_name(&builtin.ty())
                    ),
                    span,
                );
            }
            return Ok(Some(IoBinding::Builtin(builtin)));
        }
        if let Some(attr) = has_attr(attrs, "location") {
            let location = self.attr_u32(attr)?;
            let ok = match ty {
                Type::Scalar(s) | Type::Vector(_, s) => s.is_numeric() && !s.is_abstract(),
                _ => false,
            };
            if !ok {
                return err("@location values must be numeric scalars or vectors", span);
            }
            let interpolation = match has_attr(attrs, "interpolate") {
                Some(attr) => {
                    let sampling = match attr.args.len() {
                        1 => Sampling::Center,
                        _ => match self.attr_word(attr, 1)? {
                            "center" => Sampling::Center,
                            "centroid" => Sampling::Centroid,
                            "sample" => Sampling::Sample,
                            "first" | "either" => Sampling::Center,
                            other => return err(format!("unknown sampling `{other}`"), attr.span),
                        },
                    };
                    Some(match self.attr_word(attr, 0)? {
                        "perspective" => Interpolation::Perspective(sampling),
                        "linear" => Interpolation::Linear(sampling),
                        "flat" => Interpolation::Flat,
                        other => return err(format!("unknown interpolation `{other}`"), attr.span),
                    })
                }
                None => None,
            };
            return Ok(Some(IoBinding::Location {
                location,
                interpolation,
            }));
        }
        Ok(None)
    }

    fn global_var(&mut self, d: &ast::VarDecl) -> Result<Resolved> {
        let ty = match &d.ty {
            Some(ty) => Some(self.resolve_type(ty)?),
            None => None,
        };
        let is_handle = matches!(ty, Some(Type::Sampler { .. } | Type::Texture { .. }));
        let space = match d.space.as_deref() {
            None if is_handle => AddressSpace::Handle,
            None => return err("module-scope `var` needs an address space", d.span),
            Some("private") => AddressSpace::Private,
            Some("workgroup") => AddressSpace::Workgroup,
            Some("uniform") => AddressSpace::Uniform,
            Some("storage") => AddressSpace::Storage,
            Some("function") => {
                return err("`function` variables must be declared in functions", d.span)
            }
            Some(other) => return err(format!("unknown address space `{other}`"), d.span),
        };
        let access = match (space, d.access.as_deref()) {
            (AddressSpace::Storage, None | Some("read")) => Access::Read,
            (AddressSpace::Storage, Some("read_write")) => Access::ReadWrite,
            (AddressSpace::Storage, Some(other)) => {
                return err(format!("invalid access mode `{other}`"), d.span)
            }
            (_, Some(_)) => return err("only `storage` variables take an access mode", d.span),
            (AddressSpace::Uniform | AddressSpace::Handle, None) => Access::Read,
            (_, None) => Access::ReadWrite,
        };
        let init = match &d.init {
            Some(init) => {
                if space != AddressSpace::Private {
                    return err("only `private` variables may have initializers", init.span);
                }
                let value = self.expr(init)?;
                let value = match &ty {
                    Some(ty) => self.coerce(value, ty, init.span)?,
                    None => self.concretize(value, init.span)?,
                };
                if value.as_const().is_none() {
                    return err(
                        "module-scope initializers must be constant expressions",
                        init.span,
                    );
                }
                Some(value)
            }
            None => None,
        };
        let ty = match (ty, &init) {
            (Some(ty), _) => ty,
            (None, Some(init)) => init.ty.clone(),
            (None, None) => return err("`var` needs a type or an initializer", d.span),
        };
        match space {
            AddressSpace::Uniform | AddressSpace::Storage => {
                if !ty.is_host_shareable(self.structs()) {
                    return err(
                        format!(
                            "{} cannot be stored in a {} buffer",
                            self.type_name(&ty),
                            space.name()
                        ),
                        d.span,
                    );
                }
                if space == AddressSpace::Uniform && !ty.is_constructible(self.structs()) {
                    return err("uniform buffers cannot hold runtime-sized arrays", d.span);
                }
            }
            AddressSpace::Private | AddressSpace::Workgroup => {
                if matches!(ty, Type::Array { count: None, .. }) {
                    return err(
                        "runtime-sized arrays are only allowed in storage buffers",
                        d.span,
                    );
                }
            }
            _ => {}
        }
        let group_binding = match (has_attr(&d.attrs, "group"), has_attr(&d.attrs, "binding")) {
            (Some(g), Some(b)) => Some((self.attr_u32(g)?, self.attr_u32(b)?)),
            (None, None) => None,
            _ => return err("resources need both @group and @binding", d.span),
        };
        let is_resource = matches!(
            space,
            AddressSpace::Uniform | AddressSpace::Storage | AddressSpace::Handle
        );
        if is_resource != group_binding.is_some() {
            return err(
                if is_resource {
                    "resource variables need @group and @binding"
                } else {
                    "only resource variables take @group and @binding"
                },
                d.span,
            );
        }
        let index = self.module.globals.len();
        if let Some((group, binding)) = group_binding {
            if let Some(other) = self
                .module
                .bindings
                .iter()
                .find(|b| b.group == group && b.binding == binding)
            {
                return err(
                    format!(
                        "@group({group}) @binding({binding}) is already used by `{}`",
                        other.name
                    ),
                    d.span,
                );
            }
            let kind = self.resource_kind(&ty, space, access);
            self.module.bindings.push(ir::Binding {
                group,
                binding,
                name: d.name.clone(),
                global: index,
                kind,
            });
        }
        self.module.globals.push(ir::Global {
            name: d.name.clone(),
            space,
            access,
            ty,
            group_binding,
            init,
        });
        Ok(Resolved::Global(index))
    }

    fn resource_kind(&self, ty: &Type, space: AddressSpace, access: Access) -> ir::ResourceKind {
        use ir::{ResourceKind as R, TextureSample};
        let size = align_and_size(ty, self.structs()).1 as u64;
        match (space, ty) {
            (AddressSpace::Uniform, _) => R::UniformBuffer { min_size: size },
            (AddressSpace::Storage, _) => R::StorageBuffer {
                read_only: access == Access::Read,
                min_size: size,
            },
            (_, Type::Sampler { comparison }) => R::Sampler {
                comparison: *comparison,
            },
            (_, Type::Texture { dim, arrayed, kind }) => match kind {
                TextureKind::Sampled {
                    sampled,
                    multisampled,
                } => R::Texture {
                    dim: *dim,
                    arrayed: *arrayed,
                    sample: match sampled {
                        Scalar::I32 => TextureSample::Sint,
                        Scalar::U32 => TextureSample::Uint,
                        _ => TextureSample::Float,
                    },
                    multisampled: *multisampled,
                },
                TextureKind::Depth { multisampled } => R::Texture {
                    dim: *dim,
                    arrayed: *arrayed,
                    sample: TextureSample::Depth,
                    multisampled: *multisampled,
                },
                TextureKind::Storage { format, access } => R::StorageTexture {
                    dim: *dim,
                    arrayed: *arrayed,
                    format: format.clone(),
                    access: *access,
                },
                TextureKind::External => R::ExternalTexture,
            },
            _ => R::UniformBuffer { min_size: size },
        }
    }

    fn function(&mut self, decl_index: usize, d: &'a ast::FnDecl) -> Result<Resolved> {
        let index = self.module.functions.len();
        self.module.functions.push(ir::Function {
            name: d.name.clone(),
            params: Vec::new(),
            ret: Type::Void,
            ret_io: None,
            locals: Vec::new(),
            body: Vec::new(),
            globals: Vec::new(),
            calls: Vec::new(),
        });
        self.function_decls.push(decl_index);
        // Mark resolved before the body so the body's own calls see it as a cycle, not pending.
        self.states[decl_index] = DeclState::Done(Resolved::Function(index));

        let mut params = Vec::new();
        let mut scope = HashMap::new();
        for (i, p) in d.params.iter().enumerate() {
            let ty = self.resolve_type(&p.ty)?;
            if scope
                .insert(p.name.clone(), LocalBinding::Param(i))
                .is_some()
            {
                return err(format!("duplicate parameter `{}`", p.name), p.span);
            }
            let io = self.io_attrs(&p.attrs, &ty, p.span)?;
            params.push(ir::Param {
                name: p.name.clone(),
                ty,
                io,
            });
        }
        let (ret, ret_io) = match &d.ret {
            Some((attrs, ty)) => {
                let ty = self.resolve_type(ty)?;
                let io = self.io_attrs(attrs, &ty, d.span)?;
                (ty, io)
            }
            None => (Type::Void, None),
        };
        {
            let f = &mut self.module.functions[index];
            f.params = params.clone();
            f.ret = ret.clone();
            f.ret_io = ret_io;
        }
        self.func = Some(FnCtx {
            scopes: vec![scope],
            params,
            locals: Vec::new(),
            ret,
            globals: BTreeSet::new(),
            calls: BTreeSet::new(),
            loops: 0,
            breakable: 0,
        });
        self.active.push(index);
        let body = self.block(&d.body);
        self.active.pop();
        let body = body?;
        let ctx = self.func.take().unwrap();
        let f = &mut self.module.functions[index];
        f.body = body;
        f.locals = ctx.locals;
        f.globals = ctx.globals.into_iter().collect();
        f.calls = ctx.calls.into_iter().collect();
        Ok(Resolved::Function(index))
    }

    fn finish(mut self) -> Result<ir::Module> {
        for (index, &decl_index) in self.function_decls.clone().iter().enumerate() {
            let ast::Decl::Fn(d) = &self.ast.decls[decl_index] else {
                unreachable!()
            };
            let stage = if has_attr(&d.attrs, "vertex").is_some() {
                Stage::Vertex
            } else if has_attr(&d.attrs, "fragment").is_some() {
                Stage::Fragment
            } else if has_attr(&d.attrs, "compute").is_some() {
                Stage::Compute
            } else {
                continue;
            };
            let mut workgroup_size = [1, 1, 1];
            if stage == Stage::Compute {
                let Some(attr) = has_attr(&d.attrs, "workgroup_size") else {
                    return err("compute entry points need @workgroup_size", d.span);
                };
                if attr.args.is_empty() || attr.args.len() > 3 {
                    return err("@workgroup_size takes one to three values", attr.span);
                }
                for (i, arg) in attr.args.iter().enumerate() {
                    workgroup_size[i] = self.const_u32(arg, "@workgroup_size")?;
                    if workgroup_size[i] == 0 {
                        return err("workgroup sizes must be at least 1", arg.span);
                    }
                }
            }
            let entry = self.entry_point(index, stage, workgroup_size, d)?;
            self.module.entry_points.push(entry);
        }
        Ok(self.module)
    }

    fn interface(
        &self,
        ty: &Type,
        io: Option<IoBinding>,
        span: Span,
        out: &mut Vec<ir::InterfaceVar>,
    ) -> Result<()> {
        match (io, ty) {
            (Some(io), _) => out.push(ir::InterfaceVar { io, ty: ty.clone() }),
            (None, Type::Struct(i)) => {
                for m in &self.structs()[*i].members {
                    let io = match (m.builtin, m.location) {
                        (Some(b), _) => IoBinding::Builtin(b),
                        (None, Some(location)) => IoBinding::Location {
                            location,
                            interpolation: m.interpolation,
                        },
                        (None, None) => {
                            return err(
                                format!("member `{}` needs @builtin or @location", m.name),
                                span,
                            )
                        }
                    };
                    out.push(ir::InterfaceVar {
                        io,
                        ty: m.ty.clone(),
                    });
                }
            }
            (None, _) => return err("entry-point values need @builtin or @location", span),
        }
        Ok(())
    }

    fn entry_point(
        &self,
        index: usize,
        stage: Stage,
        workgroup_size: [u32; 3],
        d: &ast::FnDecl,
    ) -> Result<ir::EntryPoint> {
        let f = &self.module.functions[index];
        let mut inputs = Vec::new();
        for (p, ast_param) in f.params.iter().zip(&d.params) {
            self.interface(&p.ty, p.io, ast_param.span, &mut inputs)?;
        }
        let mut outputs = Vec::new();
        if f.ret != Type::Void {
            self.interface(&f.ret, f.ret_io, d.span, &mut outputs)?;
        }
        for var in &inputs {
            let allowed = match var.io {
                IoBinding::Builtin(b) => match stage {
                    Stage::Vertex => matches!(b, Builtin::VertexIndex | Builtin::InstanceIndex),
                    Stage::Fragment => matches!(
                        b,
                        Builtin::Position
                            | Builtin::FrontFacing
                            | Builtin::SampleIndex
                            | Builtin::SampleMask
                    ),
                    Stage::Compute => matches!(
                        b,
                        Builtin::LocalInvocationId
                            | Builtin::LocalInvocationIndex
                            | Builtin::GlobalInvocationId
                            | Builtin::WorkgroupId
                            | Builtin::NumWorkgroups
                    ),
                },
                IoBinding::Location { .. } => stage != Stage::Compute,
            };
            if !allowed {
                return err("this input is not available in this shader stage", d.span);
            }
        }
        for var in &outputs {
            let allowed = match (var.io, stage) {
                (_, Stage::Compute) => false,
                (IoBinding::Builtin(b), Stage::Vertex) => b == Builtin::Position,
                (IoBinding::Builtin(b), Stage::Fragment) => {
                    matches!(b, Builtin::FragDepth | Builtin::SampleMask)
                }
                (IoBinding::Location { .. }, _) => true,
            };
            if !allowed {
                return err("this output is not available in this shader stage", d.span);
            }
        }
        if stage == Stage::Vertex
            && !outputs
                .iter()
                .any(|v| v.io == IoBinding::Builtin(Builtin::Position))
        {
            return err("vertex entry points must output @builtin(position)", d.span);
        }
        let mut seen = std::collections::HashSet::new();
        for var in inputs.iter().chain(&outputs) {
            let _ = var;
        }
        for (list, what) in [(&inputs, "input"), (&outputs, "output")] {
            seen.clear();
            for var in list {
                let key = match var.io {
                    IoBinding::Builtin(b) => format!("{b:?}"),
                    IoBinding::Location { location, .. } => format!("@location({location})"),
                };
                if !seen.insert(key.clone()) {
                    return err(format!("duplicate {what} {key}"), d.span);
                }
            }
        }

        // Globals used by the entry point and everything it calls.
        let mut globals = BTreeSet::new();
        let mut stack = vec![index];
        let mut visited = BTreeSet::new();
        while let Some(f) = stack.pop() {
            if !visited.insert(f) {
                continue;
            }
            let function = &self.module.functions[f];
            globals.extend(function.globals.iter().copied());
            stack.extend(function.calls.iter().copied());
        }
        Ok(ir::EntryPoint {
            name: d.name.clone(),
            stage,
            function: index,
            workgroup_size,
            inputs,
            outputs,
            globals: globals.into_iter().collect(),
        })
    }

    // ---------------------------------------------------------------------------------------
    // Types

    fn lookup_module(&mut self, name: &str) -> Result<Option<Resolved>> {
        match self.names.get(name) {
            Some(&i) => Ok(Some(self.resolve_decl(i)?)),
            None => Ok(None),
        }
    }

    fn lookup_local(&self, name: &str) -> Option<LocalBinding> {
        let ctx = self.func.as_ref()?;
        ctx.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }

    fn scalar_type(&self, name: &str, span: Span) -> Result<Option<Scalar>> {
        Ok(Some(match name {
            "bool" => Scalar::Bool,
            "i32" => Scalar::I32,
            "u32" => Scalar::U32,
            "f32" => Scalar::F32,
            "f16" => {
                if !self.module.enables.iter().any(|e| e == "f16") {
                    return err("using f16 requires `enable f16;`", span);
                }
                Scalar::F16
            }
            _ => return Ok(None),
        }))
    }

    fn template_type(&mut self, id: &ast::Ident, index: usize) -> Result<Type> {
        match id.template.get(index) {
            Some(ast::Expr {
                kind: A::Ident(inner),
                ..
            }) => self.resolve_type(inner),
            Some(other) => err("expected a type", other.span),
            None => err(format!("`{}` needs a type argument", id.name), id.span),
        }
    }

    fn template_word<'b>(&self, id: &'b ast::Ident, index: usize) -> Result<Option<&'b str>> {
        match id.template.get(index) {
            Some(ast::Expr {
                kind: A::Ident(inner),
                ..
            }) if inner.template.is_empty() => Ok(Some(&inner.name)),
            Some(other) => err("expected a name", other.span),
            None => Ok(None),
        }
    }

    /// Resolves predeclared type names. `Ok(None)` when `id` is not a predeclared type.
    fn predeclared_type(&mut self, id: &ast::Ident) -> Result<Option<Type>> {
        let name = id.name.as_str();
        if let Some(s) = self.scalar_type(name, id.span)? {
            return Ok(Some(Type::Scalar(s)));
        }
        if let Some((size, scalar)) = vector_alias(name) {
            return Ok(Some(Type::Vector(size, scalar)));
        }
        if let Some((columns, rows, scalar)) = matrix_alias(name) {
            return Ok(Some(Type::Matrix {
                columns,
                rows,
                scalar,
            }));
        }
        if let Some(n) = vector_generic(name) {
            let element = self.template_type(id, 0)?;
            return match element {
                Type::Scalar(s) => Ok(Some(Type::Vector(n, s))),
                _ => err("vector components must be scalars", id.span),
            };
        }
        if let Some((columns, rows)) = matrix_generic(name) {
            return match self.template_type(id, 0)? {
                Type::Scalar(s) if s.is_float() => Ok(Some(Type::Matrix {
                    columns,
                    rows,
                    scalar: s,
                })),
                _ => err("matrix components must be floats", id.span),
            };
        }
        let ty = match name {
            "array" => {
                let element = self.template_type(id, 0)?;
                if matches!(element, Type::Array { count: None, .. }) || !self.is_storable(&element)
                {
                    return err("invalid array element type", id.span);
                }
                let count = match id.template.get(1) {
                    Some(e) => {
                        let n = self.const_u32(e, "array element counts")?;
                        if n == 0 {
                            return err("array element counts must be at least 1", e.span);
                        }
                        Some(n)
                    }
                    None => None,
                };
                Type::Array {
                    element: Box::new(element),
                    count,
                }
            }
            "atomic" => match self.template_type(id, 0)? {
                Type::Scalar(s @ (Scalar::I32 | Scalar::U32)) => Type::Atomic(s),
                _ => return err("atomics hold i32 or u32", id.span),
            },
            "ptr" => {
                let space = match self.template_word(id, 0)? {
                    Some("function") => AddressSpace::Function,
                    Some("private") => AddressSpace::Private,
                    Some("workgroup") => AddressSpace::Workgroup,
                    Some("uniform") => AddressSpace::Uniform,
                    Some("storage") => AddressSpace::Storage,
                    _ => return err("expected an address space", id.span),
                };
                let pointee = self.template_type(id, 1)?;
                let access = match (self.template_word(id, 2)?, space) {
                    (None, AddressSpace::Uniform | AddressSpace::Storage) => Access::Read,
                    (None, _) => Access::ReadWrite,
                    (Some("read"), _) => Access::Read,
                    (Some("write"), _) => Access::Write,
                    (Some("read_write"), _) => Access::ReadWrite,
                    _ => return err("expected an access mode", id.span),
                };
                Type::Pointer {
                    space,
                    pointee: Box::new(pointee),
                    access,
                }
            }
            "sampler" => Type::Sampler { comparison: false },
            "sampler_comparison" => Type::Sampler { comparison: true },
            "texture_external" => Type::Texture {
                dim: TextureDim::D2,
                arrayed: false,
                kind: TextureKind::External,
            },
            _ => {
                if let Some((dim, arrayed, multisampled)) = sampled_texture(name) {
                    let sampled = match self.template_type(id, 0)? {
                        Type::Scalar(s @ (Scalar::F32 | Scalar::I32 | Scalar::U32)) => s,
                        _ => return err("texture sample types are f32, i32 or u32", id.span),
                    };
                    Type::Texture {
                        dim,
                        arrayed,
                        kind: TextureKind::Sampled {
                            sampled,
                            multisampled,
                        },
                    }
                } else if let Some((dim, arrayed, multisampled)) = depth_texture(name) {
                    Type::Texture {
                        dim,
                        arrayed,
                        kind: TextureKind::Depth { multisampled },
                    }
                } else if let Some((dim, arrayed)) = storage_texture(name) {
                    let Some(format) = self.template_word(id, 0)? else {
                        return err("storage textures need a texel format", id.span);
                    };
                    let access = match self.template_word(id, 1)? {
                        Some("read") => Access::Read,
                        Some("write") => Access::Write,
                        Some("read_write") => Access::ReadWrite,
                        _ => return err("storage textures need an access mode", id.span),
                    };
                    Type::Texture {
                        dim,
                        arrayed,
                        kind: TextureKind::Storage {
                            format: format.to_string(),
                            access,
                        },
                    }
                } else {
                    return Ok(None);
                }
            }
        };
        Ok(Some(ty))
    }

    fn is_storable(&self, ty: &Type) -> bool {
        !matches!(
            ty,
            Type::Void | Type::Pointer { .. } | Type::Sampler { .. } | Type::Texture { .. }
        )
    }

    fn resolve_type(&mut self, id: &ast::Ident) -> Result<Type> {
        if self.lookup_local(&id.name).is_some() {
            return err(format!("`{}` is not a type", id.name), id.span);
        }
        if let Some(resolved) = self.lookup_module(&id.name)? {
            return match resolved {
                Resolved::Struct(i) => Ok(Type::Struct(i)),
                Resolved::Alias(ty) => Ok(ty),
                _ => err(format!("`{}` is not a type", id.name), id.span),
            };
        }
        match self.predeclared_type(id)? {
            Some(ty) => Ok(ty),
            None => err(format!("unknown type `{}`", id.name), id.span),
        }
    }

    // ---------------------------------------------------------------------------------------
    // Values and conversions

    fn value(&self, e: Expr) -> Expr {
        if e.reference.is_some() {
            Expr {
                ty: e.ty.clone(),
                kind: ExprKind::Load(Box::new(e)),
                reference: None,
            }
        } else {
            e
        }
    }

    fn coerce(&self, e: Expr, to: &Type, span: Span) -> Result<Expr> {
        let e = self.value(e);
        if e.ty == *to {
            return Ok(e);
        }
        let convertible = same_shape(&e.ty, to)
            && match (e.ty.leaf_scalar(), to.leaf_scalar()) {
                (Some(from), Some(target)) => from.is_abstract() && from.converts_to(target),
                _ => false,
            };
        if convertible {
            if let ExprKind::Const(value) = &e.kind {
                let value = consts::convert(value, &e.ty, to, self.structs())
                    .map_err(|m| Error::new(m, span))?;
                return Ok(Expr::constant(value, to.clone()));
            }
        }
        err(
            format!(
                "expected {}, found {}",
                self.type_name(to),
                self.type_name(&e.ty)
            ),
            span,
        )
    }

    fn concretize(&self, e: Expr, span: Span) -> Result<Expr> {
        let e = self.value(e);
        if e.ty.is_abstract() {
            let ty = e.ty.concretize();
            self.coerce(e, &ty, span)
        } else {
            Ok(e)
        }
    }

    /// The scalar both sides convert to, if any.
    fn unify_scalars(&self, scalars: &[Scalar]) -> Option<Scalar> {
        let concrete: Vec<Scalar> = scalars
            .iter()
            .copied()
            .filter(|s| !s.is_abstract())
            .collect();
        if let Some(&first) = concrete.first() {
            if concrete.iter().any(|&s| s != first) {
                return None;
            }
            if scalars.iter().all(|s| s.converts_to(first)) {
                return Some(first);
            }
            return None;
        }
        if scalars.contains(&Scalar::AbstractFloat) {
            Some(Scalar::AbstractFloat)
        } else {
            scalars.first().copied()
        }
    }

    /// Converts every value to one shared scalar type, keeping each value's shape.
    fn unify_values(&self, values: Vec<Expr>, span: Span) -> Result<(Vec<Expr>, Scalar)> {
        let scalars: Vec<Scalar> = values
            .iter()
            .map(|v| {
                v.ty.leaf_scalar()
                    .ok_or_else(|| Error::new("expected a numeric or boolean value", span))
            })
            .collect::<Result<_>>()?;
        let Some(target) = self.unify_scalars(&scalars) else {
            let names: Vec<String> = values.iter().map(|v| self.type_name(&v.ty)).collect();
            return err(format!("mismatched types: {}", names.join(", ")), span);
        };
        let out = values
            .into_iter()
            .map(|v| {
                let ty = v.ty.with_scalar(target);
                self.coerce(v, &ty, span)
            })
            .collect::<Result<_>>()?;
        Ok((out, target))
    }

    fn ctx(&mut self) -> &mut FnCtx {
        self.func.as_mut().expect("statement outside a function")
    }

    // ---------------------------------------------------------------------------------------
    // Expressions

    fn expr(&mut self, e: &ast::Expr) -> Result<Expr> {
        let span = e.span;
        match &e.kind {
            A::Bool(b) => Ok(Expr::constant(ConstValue::Bool(*b), BOOL)),
            A::Int(v, suffix) => {
                let scalar = match suffix {
                    crate::lexer::IntSuffix::None => Scalar::AbstractInt,
                    crate::lexer::IntSuffix::I => Scalar::I32,
                    crate::lexer::IntSuffix::U => Scalar::U32,
                };
                // `-2147483648i` lexes as negation of a literal one past i32::MAX, so allow it
                // here and let negation range-check.
                let value =
                    consts::convert_scalar(&ConstValue::Int(*v), Scalar::AbstractInt, scalar)
                        .or_else(|m| {
                            if scalar == Scalar::I32 && *v == 1 << 31 {
                                Ok(ConstValue::Int(*v))
                            } else {
                                Err(m)
                            }
                        })
                        .map_err(|m| Error::new(m, span))?;
                Ok(Expr::constant(value, Type::Scalar(scalar)))
            }
            A::Float(v, suffix) => {
                let scalar = match suffix {
                    crate::lexer::FloatSuffix::None => Scalar::AbstractFloat,
                    crate::lexer::FloatSuffix::F => Scalar::F32,
                    crate::lexer::FloatSuffix::H => {
                        self.scalar_type("f16", span)?;
                        Scalar::F16
                    }
                };
                let value =
                    consts::convert_scalar(&ConstValue::Float(*v), Scalar::AbstractFloat, scalar)
                        .map_err(|m| Error::new(m, span))?;
                Ok(Expr::constant(value, Type::Scalar(scalar)))
            }
            A::Ident(id) => self.ident(id),
            A::Unary(op, operand) => {
                let operand = self.expr(operand)?;
                self.unary(*op, operand, span)
            }
            A::Binary(op, lhs, rhs) => {
                let lhs = self.expr(lhs)?;
                let rhs = self.expr(rhs)?;
                self.binary(*op, lhs, rhs, span)
            }
            A::Call(callee, args) => {
                let call = self.call(callee, args, span)?;
                if call.ty == Type::Void {
                    return err("this function has no return value", span);
                }
                Ok(call)
            }
            A::Member(base, name, name_span) => {
                let base = self.expr(base)?;
                self.member(base, name, *name_span)
            }
            A::Index(base, index) => {
                let base = self.expr(base)?;
                let index = self.expr(index)?;
                self.index(base, index, span)
            }
        }
    }

    fn ident(&mut self, id: &ast::Ident) -> Result<Expr> {
        if !id.template.is_empty() {
            return err(format!("`{}` is a type, not a value", id.name), id.span);
        }
        if let Some(binding) = self.lookup_local(&id.name) {
            let ctx = self.func.as_ref().unwrap();
            return Ok(match binding {
                LocalBinding::Local(i) => {
                    let local = &ctx.locals[i];
                    Expr {
                        kind: ExprKind::Local(i),
                        ty: local.ty.clone(),
                        reference: local
                            .mutable
                            .then_some((AddressSpace::Function, Access::ReadWrite)),
                    }
                }
                LocalBinding::Param(i) => Expr {
                    kind: ExprKind::Param(i),
                    ty: ctx.params[i].ty.clone(),
                    reference: None,
                },
                LocalBinding::Const(value, ty) => Expr::constant(value, ty),
            });
        }
        match self.lookup_module(&id.name)? {
            Some(Resolved::Const(value, ty)) => Ok(Expr::constant(value, ty)),
            Some(Resolved::Global(i)) => {
                let global = &self.module.globals[i];
                let expr = Expr {
                    kind: ExprKind::Global(i),
                    ty: global.ty.clone(),
                    reference: Some((global.space, global.access)),
                };
                match self.func.as_mut() {
                    Some(ctx) => {
                        ctx.globals.insert(i);
                    }
                    None => return err("module-scope variables cannot be used here", id.span),
                }
                Ok(expr)
            }
            Some(Resolved::Function(_)) => {
                err(format!("function `{}` must be called", id.name), id.span)
            }
            Some(_) => err(format!("`{}` is a type, not a value", id.name), id.span),
            None => err(format!("unresolved identifier `{}`", id.name), id.span),
        }
    }

    fn unary(&mut self, op: UnaryOp, operand: Expr, span: Span) -> Result<Expr> {
        match op {
            UnaryOp::AddrOf => {
                let Some((space, access)) = operand.reference else {
                    return err("`&` needs a variable, member or element", span);
                };
                if space == AddressSpace::Handle {
                    return err("cannot take the address of a texture or sampler", span);
                }
                if matches!(operand.kind, ExprKind::Index(ref base, _) if matches!(base.ty, Type::Vector(..)))
                {
                    return err("cannot take the address of a vector component", span);
                }
                let ty = Type::Pointer {
                    space,
                    pointee: Box::new(operand.ty.clone()),
                    access,
                };
                Ok(Expr {
                    kind: ExprKind::AddrOf(Box::new(operand)),
                    ty,
                    reference: None,
                })
            }
            UnaryOp::Deref => {
                let operand = self.value(operand);
                let Type::Pointer {
                    space,
                    pointee,
                    access,
                } = operand.ty.clone()
                else {
                    return err("`*` needs a pointer", span);
                };
                Ok(Expr {
                    kind: ExprKind::Deref(Box::new(operand)),
                    ty: *pointee,
                    reference: Some((space, access)),
                })
            }
            _ => {
                let operand = self.value(operand);
                let scalar = match &operand.ty {
                    Type::Scalar(s) | Type::Vector(_, s) => *s,
                    other => {
                        return err(
                            format!("cannot apply `{op:?}` to {}", self.type_name(other)),
                            span,
                        )
                    }
                };
                let ok = match op {
                    UnaryOp::Neg => scalar.is_float() || scalar.is_signed(),
                    UnaryOp::Not => scalar == Scalar::Bool,
                    UnaryOp::BitNot => scalar.is_integer(),
                    _ => unreachable!(),
                };
                if !ok {
                    let symbol = match op {
                        UnaryOp::Neg => "-",
                        UnaryOp::Not => "!",
                        _ => "~",
                    };
                    return err(
                        format!("cannot apply `{symbol}` to {}", self.type_name(&operand.ty)),
                        span,
                    );
                }
                if let ExprKind::Const(value) = &operand.kind {
                    let folded =
                        consts::unary(op, value, &operand.ty).map_err(|m| Error::new(m, span))?;
                    return Ok(Expr::constant(folded, operand.ty));
                }
                let ty = operand.ty.clone();
                Ok(Expr {
                    kind: ExprKind::Unary(op, Box::new(operand)),
                    ty,
                    reference: None,
                })
            }
        }
    }

    fn splat(&self, e: Expr, n: u8) -> Expr {
        let scalar = e.ty.scalar().unwrap();
        let ty = Type::Vector(n, scalar);
        if let ExprKind::Const(value) = &e.kind {
            return Expr::constant(ConstValue::Composite(vec![value.clone(); n as usize]), ty);
        }
        Expr {
            kind: ExprKind::Splat(Box::new(e)),
            ty,
            reference: None,
        }
    }

    fn binary(&mut self, op: BinaryOp, lhs: Expr, rhs: Expr, span: Span) -> Result<Expr> {
        use BinaryOp::*;
        let lhs = self.value(lhs);
        let rhs = self.value(rhs);
        let mismatch = |this: &Self, l: &Expr, r: &Expr| -> Result<Expr> {
            err(
                format!(
                    "cannot apply `{}` to {} and {}",
                    op.symbol(),
                    this.type_name(&l.ty),
                    this.type_name(&r.ty)
                ),
                span,
            )
        };

        if matches!(op, Shl | Shr) {
            let lhs_size = match &lhs.ty {
                Type::Scalar(s) if s.is_integer() => None,
                Type::Vector(n, s) if s.is_integer() => Some(*n),
                _ => return mismatch(self, &lhs, &rhs),
            };
            let rhs = self.coerce(rhs, &vector_or_scalar(lhs_size, Scalar::U32), span)?;
            let lhs = if lhs.ty.is_abstract() && rhs.as_const().is_none() {
                self.concretize(lhs, span)?
            } else {
                lhs
            };
            if let (Some(a), Some(b)) = (lhs.as_const(), rhs.as_const()) {
                let folded = consts::binary(op, a, b, &lhs.ty).map_err(|m| Error::new(m, span))?;
                return Ok(Expr::constant(folded, lhs.ty));
            }
            let ty = lhs.ty.clone();
            return Ok(Expr {
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                ty,
                reference: None,
            });
        }

        let (values, scalar) = match self.unify_values(vec![lhs.clone(), rhs.clone()], span) {
            Ok(v) => v,
            Err(_) => return mismatch(self, &lhs, &rhs),
        };
        let [mut lhs, mut rhs]: [Expr; 2] = values.try_into().unwrap();

        let arithmetic = matches!(op, Add | Sub | Mul | Div | Rem);
        let result_ty = match (&lhs.ty, &rhs.ty) {
            (
                Type::Matrix { columns, rows, .. },
                Type::Matrix {
                    columns: c2,
                    rows: r2,
                    ..
                },
            ) => match op {
                Add | Sub if (columns, rows) == (c2, r2) => lhs.ty.clone(),
                Mul if columns == r2 => Type::Matrix {
                    columns: *c2,
                    rows: *rows,
                    scalar,
                },
                _ => return mismatch(self, &lhs, &rhs),
            },
            (Type::Matrix { .. }, Type::Scalar(_)) | (Type::Scalar(_), Type::Matrix { .. })
                if op == Mul =>
            {
                if lhs.ty.vector_size().is_none() && matches!(lhs.ty, Type::Matrix { .. }) {
                    lhs.ty.clone()
                } else {
                    rhs.ty.clone()
                }
            }
            (Type::Matrix { columns, rows, .. }, Type::Vector(n, _))
                if op == Mul && columns == n =>
            {
                Type::Vector(*rows, scalar)
            }
            (Type::Vector(n, _), Type::Matrix { columns, rows, .. }) if op == Mul && rows == n => {
                Type::Vector(*columns, scalar)
            }
            (Type::Matrix { .. }, _) | (_, Type::Matrix { .. }) => {
                return mismatch(self, &lhs, &rhs)
            }
            (Type::Vector(n, _), Type::Scalar(_)) if arithmetic => {
                rhs = self.splat(rhs, *n);
                lhs.ty.clone()
            }
            (Type::Scalar(_), Type::Vector(n, _)) if arithmetic => {
                lhs = self.splat(lhs, *n);
                rhs.ty.clone()
            }
            (a, b) if a == b && matches!(a, Type::Scalar(_) | Type::Vector(..)) => {
                let size = a.vector_size();
                let ok = match op {
                    Add | Sub | Mul | Div | Rem => scalar.is_numeric(),
                    BitAnd | BitOr => scalar.is_integer() || scalar == Scalar::Bool,
                    BitXor => scalar.is_integer(),
                    And | Or => scalar == Scalar::Bool && size.is_none(),
                    Eq | Ne => true,
                    Lt | Le | Gt | Ge => scalar.is_numeric(),
                    Shl | Shr => unreachable!(),
                };
                if !ok {
                    return mismatch(self, &lhs, &rhs);
                }
                match op {
                    Eq | Ne | Lt | Le | Gt | Ge => vector_or_scalar(size, Scalar::Bool),
                    _ => a.clone(),
                }
            }
            _ => return mismatch(self, &lhs, &rhs),
        };

        if let (Some(a), Some(b)) = (lhs.as_const(), rhs.as_const()) {
            if let Ok(folded) = consts::binary(op, a, b, &lhs.ty) {
                return Ok(Expr::constant(folded, result_ty));
            } else if !lhs.ty.is_abstract() && !matches!(lhs.ty, Type::Matrix { .. }) {
                // Concrete constant errors (overflow, division by zero) are real errors.
                let message = consts::binary(op, a, b, &lhs.ty).unwrap_err();
                return err(message, span);
            }
        }
        if lhs.ty.is_abstract() || rhs.ty.is_abstract() {
            let lhs = self.concretize(lhs, span)?;
            let rhs = self.concretize(rhs, span)?;
            return self.binary(op, lhs, rhs, span);
        }
        Ok(Expr {
            kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
            ty: result_ty,
            reference: None,
        })
    }

    /// Pointers can be used directly for member and element access.
    fn auto_deref(&self, base: Expr) -> Expr {
        if base.reference.is_none() {
            if let Type::Pointer {
                space,
                pointee,
                access,
            } = &base.ty
            {
                let ty = (**pointee).clone();
                let reference = Some((*space, *access));
                return Expr {
                    kind: ExprKind::Deref(Box::new(base)),
                    ty,
                    reference,
                };
            }
        }
        base
    }

    fn member(&mut self, base: Expr, name: &str, span: Span) -> Result<Expr> {
        let base = self.auto_deref(base);
        match base.ty.clone() {
            Type::Struct(i) => {
                let Some(index) = self.structs()[i]
                    .members
                    .iter()
                    .position(|m| m.name == name)
                else {
                    return err(
                        format!("{} has no member `{name}`", self.structs()[i].name),
                        span,
                    );
                };
                let ty = self.structs()[i].members[index].ty.clone();
                if let ExprKind::Const(ConstValue::Composite(parts)) = &base.kind {
                    return Ok(Expr::constant(parts[index].clone(), ty));
                }
                let reference = base.reference;
                Ok(Expr {
                    kind: ExprKind::Member(Box::new(base), index as u32),
                    ty,
                    reference,
                })
            }
            Type::Vector(n, scalar) => {
                let components = swizzle(name, n)
                    .ok_or_else(|| Error::new(format!("invalid swizzle `{name}`"), span))?;
                if components.len() == 1 && base.reference.is_some() {
                    let index = Expr::constant(ConstValue::Int(components[0] as i64), U32);
                    let reference = base.reference;
                    return Ok(Expr {
                        kind: ExprKind::Index(Box::new(base), Box::new(index)),
                        ty: Type::Scalar(scalar),
                        reference,
                    });
                }
                let base = self.value(base);
                let ty = vector_or_scalar(
                    (components.len() > 1).then_some(components.len() as u8),
                    scalar,
                );
                if let ExprKind::Const(value) = &base.kind {
                    let parts = value.components();
                    let picked: Vec<ConstValue> = components
                        .iter()
                        .map(|&c| parts[c as usize].clone())
                        .collect();
                    let value = if picked.len() == 1 {
                        picked[0].clone()
                    } else {
                        ConstValue::Composite(picked)
                    };
                    return Ok(Expr::constant(value, ty));
                }
                Ok(Expr {
                    kind: ExprKind::Swizzle(Box::new(base), components),
                    ty,
                    reference: None,
                })
            }
            other => err(format!("{} has no members", self.type_name(&other)), span),
        }
    }

    fn index(&mut self, base: Expr, index: Expr, span: Span) -> Result<Expr> {
        let base = self.auto_deref(base);
        let index = self.value(index);
        let index = match index.ty {
            Type::Scalar(Scalar::I32 | Scalar::U32) => index,
            Type::Scalar(Scalar::AbstractInt) => self.coerce(index, &I32, span)?,
            _ => return err("indices must be i32 or u32", span),
        };
        let (element, count) = match &base.ty {
            Type::Array { element, count } => ((**element).clone(), *count),
            Type::Vector(n, s) => (Type::Scalar(*s), Some(*n as u32)),
            Type::Matrix {
                columns,
                rows,
                scalar,
            } => (Type::Vector(*rows, *scalar), Some(*columns as u32)),
            other => return err(format!("cannot index {}", self.type_name(other)), span),
        };
        if let (Some(ConstValue::Int(i)), Some(count)) = (index.as_const(), count) {
            if *i < 0 || *i >= count as i64 {
                return err(
                    format!("index {i} is out of bounds for {count} elements"),
                    span,
                );
            }
            if let ExprKind::Const(ConstValue::Composite(parts)) = &base.kind {
                return Ok(Expr::constant(parts[*i as usize].clone(), element));
            }
        }
        let base = if base.ty.is_abstract() {
            self.concretize(base, span)?
        } else {
            base
        };
        let element = if element.is_abstract() {
            element.concretize()
        } else {
            element
        };
        let reference = base.reference;
        Ok(Expr {
            kind: ExprKind::Index(Box::new(base), Box::new(index)),
            ty: element,
            reference,
        })
    }

    fn call(&mut self, callee: &ast::Ident, args: &[ast::Expr], span: Span) -> Result<Expr> {
        if self.lookup_local(&callee.name).is_some() {
            return err(format!("`{}` is not callable", callee.name), callee.span);
        }
        let module = self.lookup_module(&callee.name)?;
        let checked: Vec<Expr> = args.iter().map(|a| self.expr(a)).collect::<Result<_>>()?;
        let spans: Vec<Span> = args.iter().map(|a| a.span).collect();
        match module {
            Some(Resolved::Function(f)) => return self.user_call(f, checked, &spans, span),
            Some(Resolved::Struct(i)) => {
                return self.construct(Ctor::Type(Type::Struct(i)), checked, span)
            }
            Some(Resolved::Alias(ty)) => return self.construct(Ctor::Type(ty), checked, span),
            Some(_) => return err(format!("`{}` is not callable", callee.name), callee.span),
            None => {}
        }
        if callee.name == "bitcast" {
            let target = self.template_type(callee, 0)?;
            return self.bitcast(target, checked, span);
        }
        if callee.template.is_empty() {
            if let Some(n) = vector_generic(&callee.name) {
                return self.construct(Ctor::Vector(n), checked, span);
            }
            if let Some((c, r)) = matrix_generic(&callee.name) {
                return self.construct(Ctor::Matrix(c, r), checked, span);
            }
            if callee.name == "array" {
                return self.construct(Ctor::Array, checked, span);
            }
        }
        if let Some(ty) = self.predeclared_type(callee)? {
            return self.construct(Ctor::Type(ty), checked, span);
        }
        if let Some(f) = BuiltinFn::from_name(&callee.name) {
            return self.builtin(f, checked, span);
        }
        err(format!("unknown function `{}`", callee.name), callee.span)
    }

    fn user_call(&mut self, f: usize, args: Vec<Expr>, spans: &[Span], span: Span) -> Result<Expr> {
        if self.active.contains(&f) {
            return err("recursion is not allowed", span);
        }
        let params: Vec<Type> = self.module.functions[f]
            .params
            .iter()
            .map(|p| p.ty.clone())
            .collect();
        let ret = self.module.functions[f].ret.clone();
        if params.len() != args.len() {
            return err(
                format!("expected {} arguments, found {}", params.len(), args.len()),
                span,
            );
        }
        let args = args
            .into_iter()
            .zip(&params)
            .zip(spans)
            .map(|((arg, ty), s)| self.coerce(arg, ty, *s))
            .collect::<Result<_>>()?;
        self.ctx().calls.insert(f);
        Ok(Expr {
            kind: ExprKind::Call(f, args),
            ty: ret,
            reference: None,
        })
    }

    fn bitcast(&self, target: Type, args: Vec<Expr>, span: Span) -> Result<Expr> {
        let [arg]: [Expr; 1] = args
            .try_into()
            .map_err(|_| Error::new("bitcast takes one argument", span))?;
        let arg = self.concretize(arg, span)?;
        let width = |t: &Type| match t {
            Type::Scalar(s) if s.is_numeric() => Some(s.size()),
            Type::Vector(n, s) if s.is_numeric() => Some(s.size() * *n as u32),
            _ => None,
        };
        match (width(&arg.ty), width(&target)) {
            (Some(a), Some(b)) if a == b => {}
            _ => {
                return err(
                    format!(
                        "cannot bitcast {} to {}",
                        self.type_name(&arg.ty),
                        self.type_name(&target)
                    ),
                    span,
                )
            }
        }
        if arg.ty == target {
            return Ok(arg);
        }
        Ok(Expr {
            kind: ExprKind::Bitcast(Box::new(arg)),
            ty: target,
            reference: None,
        })
    }

    /// Value conversion of a scalar, vector or matrix to `to` (same shape).
    fn convert(&self, arg: Expr, to: &Type, span: Span) -> Result<Expr> {
        let arg = self.value(arg);
        if arg.ty == *to {
            return Ok(arg);
        }
        if let ExprKind::Const(value) = &arg.kind {
            let value = consts::convert(value, &arg.ty, to, self.structs())
                .map_err(|m| Error::new(m, span))?;
            return Ok(Expr::constant(value, to.clone()));
        }
        let arg = self.concretize(arg, span)?;
        if arg.ty == *to {
            return Ok(arg);
        }
        Ok(Expr {
            kind: ExprKind::Convert(Box::new(arg)),
            ty: to.clone(),
            reference: None,
        })
    }

    fn construct(&mut self, ctor: Ctor, args: Vec<Expr>, span: Span) -> Result<Expr> {
        let args: Vec<Expr> = args.into_iter().map(|a| self.value(a)).collect();
        let ty = match ctor {
            Ctor::Type(ty) => {
                if !ty.is_constructible(self.structs()) {
                    return err(
                        format!("{} cannot be constructed", self.type_name(&ty)),
                        span,
                    );
                }
                ty
            }
            Ctor::Vector(n) => {
                if args.is_empty() {
                    Type::Vector(n, Scalar::AbstractInt)
                } else {
                    let scalars: Vec<Scalar> = args.iter().filter_map(|a| a.ty.scalar()).collect();
                    let s = self
                        .unify_scalars(&scalars)
                        .ok_or_else(|| Error::new("mismatched component types", span))?;
                    Type::Vector(n, s)
                }
            }
            Ctor::Matrix(columns, rows) => {
                if args.is_empty() {
                    return err("matrix constructors without a type need arguments", span);
                }
                let scalars: Vec<Scalar> = args.iter().filter_map(|a| a.ty.scalar()).collect();
                let s = self
                    .unify_scalars(&scalars)
                    .ok_or_else(|| Error::new("mismatched component types", span))?;
                let s = if s == Scalar::AbstractInt {
                    Scalar::AbstractFloat
                } else {
                    s
                };
                Type::Matrix {
                    columns,
                    rows,
                    scalar: s,
                }
            }
            Ctor::Array => {
                if args.is_empty() {
                    return err("array constructors without a type need elements", span);
                }
                let first = args[0].ty.clone();
                if args.iter().any(|a| !same_shape(&a.ty, &first)) {
                    return err("array elements must have the same type", span);
                }
                let element = match first.leaf_scalar() {
                    Some(_) => {
                        let scalars: Vec<Scalar> =
                            args.iter().filter_map(|a| a.ty.leaf_scalar()).collect();
                        let s = self
                            .unify_scalars(&scalars)
                            .ok_or_else(|| Error::new("mismatched element types", span))?;
                        first.with_scalar(s)
                    }
                    None => first,
                };
                Type::Array {
                    element: Box::new(element),
                    count: Some(args.len() as u32),
                }
            }
        };

        if args.is_empty() {
            return Ok(Expr::constant(ConstValue::zero(&ty, self.structs()), ty));
        }

        let components: Vec<Expr> = match &ty {
            Type::Scalar(_) => {
                let [arg]: [Expr; 1] = args
                    .try_into()
                    .map_err(|_| Error::new("scalar constructors take one argument", span))?;
                if !matches!(arg.ty, Type::Scalar(_)) {
                    return err("scalar constructors take a scalar", span);
                }
                return self.convert(arg, &ty, span);
            }
            Type::Vector(n, s) => {
                if args.len() == 1 {
                    let arg = args.into_iter().next().unwrap();
                    return match &arg.ty {
                        Type::Scalar(_) => {
                            let arg = self.coerce(arg, &Type::Scalar(*s), span)?;
                            Ok(self.splat(arg, *n))
                        }
                        Type::Vector(m, _) if m == n => self.convert(arg, &ty, span),
                        _ => err(
                            format!("cannot construct {} from this", self.type_name(&ty)),
                            span,
                        ),
                    };
                }
                let mut total = 0;
                let mut out = Vec::new();
                for arg in args {
                    let shape = match &arg.ty {
                        Type::Scalar(_) => None,
                        Type::Vector(m, _) => Some(*m),
                        _ => return err("vector components must be scalars or vectors", span),
                    };
                    total += shape.unwrap_or(1);
                    out.push(self.coerce(arg, &vector_or_scalar(shape, *s), span)?);
                }
                if total != *n {
                    return err(format!("expected {n} components, found {total}"), span);
                }
                out
            }
            Type::Matrix {
                columns,
                rows,
                scalar,
            } => {
                let column = Type::Vector(*rows, *scalar);
                if args.len() == 1 && matches!(args[0].ty, Type::Matrix { .. }) {
                    let arg = args.into_iter().next().unwrap();
                    if !same_shape(&arg.ty, &ty) {
                        return err("matrix conversions keep the shape", span);
                    }
                    return self.convert(arg, &ty, span);
                }
                if args.len() == *columns as usize {
                    args.into_iter()
                        .map(|a| self.coerce(a, &column, span))
                        .collect::<Result<_>>()?
                } else if args.len() == (*columns as usize) * (*rows as usize) {
                    let scalars: Vec<Expr> = args
                        .into_iter()
                        .map(|a| self.coerce(a, &Type::Scalar(*scalar), span))
                        .collect::<Result<_>>()?;
                    let mut out = Vec::new();
                    for chunk in scalars.chunks(*rows as usize) {
                        out.push(self.construct(
                            Ctor::Type(column.clone()),
                            chunk.to_vec(),
                            span,
                        )?);
                    }
                    out
                } else {
                    return err("matrix constructors take columns or every component", span);
                }
            }
            Type::Array { element, count } => {
                if Some(args.len() as u32) != *count {
                    return err(
                        format!(
                            "expected {} elements, found {}",
                            count.unwrap_or(0),
                            args.len()
                        ),
                        span,
                    );
                }
                args.into_iter()
                    .map(|a| self.coerce(a, element, span))
                    .collect::<Result<_>>()?
            }
            Type::Struct(i) => {
                let members: Vec<Type> = self.structs()[*i]
                    .members
                    .iter()
                    .map(|m| m.ty.clone())
                    .collect();
                if members.len() != args.len() {
                    return err(
                        format!("expected {} members, found {}", members.len(), args.len()),
                        span,
                    );
                }
                args.into_iter()
                    .zip(&members)
                    .map(|(a, t)| self.coerce(a, t, span))
                    .collect::<Result<_>>()?
            }
            _ => {
                return err(
                    format!("{} cannot be constructed", self.type_name(&ty)),
                    span,
                )
            }
        };

        if components.iter().all(|c| c.as_const().is_some()) {
            let value = match &ty {
                // Vector components may be vectors themselves; flatten them.
                Type::Vector(..) => ConstValue::Composite(
                    components
                        .iter()
                        .flat_map(|c| c.as_const().unwrap().components().to_vec())
                        .collect(),
                ),
                _ => ConstValue::Composite(
                    components
                        .iter()
                        .map(|c| c.as_const().unwrap().clone())
                        .collect(),
                ),
            };
            return Ok(Expr::constant(value, ty));
        }
        if ty.is_abstract() {
            return err("abstract values must be constant", span);
        }
        Ok(Expr {
            kind: ExprKind::Construct(components),
            ty,
            reference: None,
        })
    }

    fn builtin(&mut self, f: BuiltinFn, args: Vec<Expr>, span: Span) -> Result<Expr> {
        use BuiltinFn::*;
        let args: Vec<Expr> = args.into_iter().map(|a| self.value(a)).collect();
        let arity = |n: usize| -> Result<()> {
            if args.len() == n {
                Ok(())
            } else {
                err(
                    format!("expected {n} arguments, found {}", args.len()),
                    span,
                )
            }
        };
        let float_like = |ty: &Type| matches!(ty, Type::Scalar(s) | Type::Vector(_, s) if s.is_float() || *s == Scalar::AbstractInt);
        let numeric_like =
            |ty: &Type| matches!(ty, Type::Scalar(s) | Type::Vector(_, s) if s.is_numeric());
        let int_like =
            |ty: &Type| matches!(ty, Type::Scalar(s) | Type::Vector(_, s) if s.is_integer());

        match f {
            Acos | Acosh | Asin | Asinh | Atan | Atanh | Ceil | Cos | Cosh | Degrees | Exp
            | Exp2 | Floor | Fract | InverseSqrt | Log | Log2 | Radians | Round | Saturate
            | Sin | Sinh | Sqrt | Tan | Tanh | Trunc | Atan2 | Pow | Step | Fma | Smoothstep => {
                let n = match f {
                    Atan2 | Pow | Step => 2,
                    Fma | Smoothstep => 3,
                    _ => 1,
                };
                arity(n)?;
                if !args.iter().all(|a| float_like(&a.ty)) {
                    return err("expected float arguments", span);
                }
                let (args, scalar) = self.unify_values(args, span)?;
                let args = self.floatify(args, scalar, span)?;
                self.finish_builtin(f, args, None, span)
            }
            Dpdx | Dpdy | Fwidth => {
                arity(1)?;
                if !float_like(&args[0].ty) {
                    return err("expected a float argument", span);
                }
                let args = self.floatify(args, Scalar::F32, span)?;
                let args = args
                    .into_iter()
                    .map(|a| self.concretize(a, span))
                    .collect::<Result<Vec<_>>>()?;
                self.finish_builtin(f, args, None, span)
            }
            Abs | Min | Max | Clamp | Sign => {
                arity(match f {
                    Min | Max => 2,
                    Clamp => 3,
                    _ => 1,
                })?;
                if !args.iter().all(|a| numeric_like(&a.ty)) {
                    return err("expected numeric arguments", span);
                }
                let (args, scalar) = self.unify_values(args, span)?;
                if f == Sign && scalar == Scalar::U32 {
                    return err("sign takes signed arguments", span);
                }
                self.finish_builtin(f, args, None, span)
            }
            Mix => {
                arity(3)?;
                let mut args = args;
                // mix(vecN, vecN, scalar) splats the blend factor.
                if let (Some(n), Type::Scalar(_)) = (args[0].ty.vector_size(), &args[2].ty) {
                    let factor = args.pop().unwrap();
                    args.push(self.splat(factor, n));
                }
                if !args.iter().all(|a| float_like(&a.ty)) {
                    return err("expected float arguments", span);
                }
                let (args, scalar) = self.unify_values(args, span)?;
                let args = self.floatify(args, scalar, span)?;
                self.finish_builtin(f, args, None, span)
            }
            Cross | Distance | Dot | Length | Normalize | Reflect | FaceForward | Refract => {
                arity(match f {
                    Length | Normalize => 1,
                    FaceForward | Refract => 3,
                    _ => 2,
                })?;
                let mut args = args;
                let refract_eta = if f == Refract { args.pop() } else { None };
                if f != Dot && !args.iter().all(|a| float_like(&a.ty)) {
                    return err("expected float arguments", span);
                }
                let (args, scalar) = self.unify_values(args, span)?;
                let mut args = if f == Dot {
                    args
                } else {
                    self.floatify(args, scalar, span)?
                };
                let args_ty = args[0].ty.clone();
                let ok = match f {
                    Cross => args_ty == Type::Vector(3, args_ty.scalar().unwrap()),
                    Length | Distance => matches!(args_ty, Type::Scalar(_) | Type::Vector(..)),
                    Dot => matches!(args_ty, Type::Vector(_, s) if s.is_numeric()),
                    _ => matches!(args_ty, Type::Vector(..)),
                };
                if !ok {
                    return err(
                        format!("invalid argument type {}", self.type_name(&args_ty)),
                        span,
                    );
                }
                if let Some(eta) = refract_eta {
                    let scalar = args_ty.scalar().unwrap();
                    args.push(self.coerce(eta, &Type::Scalar(scalar), span)?);
                }
                let ret = match f {
                    Distance | Dot | Length => Some(Type::Scalar(args_ty.scalar().unwrap())),
                    _ => None,
                };
                self.finish_builtin(f, args, ret, span)
            }
            Determinant | Transpose => {
                arity(1)?;
                let arg = self.concretize(args.into_iter().next().unwrap(), span)?;
                let Type::Matrix {
                    columns,
                    rows,
                    scalar,
                } = arg.ty
                else {
                    return err("expected a matrix", span);
                };
                let ret = match f {
                    Determinant if columns == rows => Type::Scalar(scalar),
                    Determinant => return err("determinant needs a square matrix", span),
                    _ => Type::Matrix {
                        columns: rows,
                        rows: columns,
                        scalar,
                    },
                };
                Ok(Expr {
                    kind: ExprKind::Builtin(f, vec![arg]),
                    ty: ret,
                    reference: None,
                })
            }
            All | Any => {
                arity(1)?;
                let arg = args.into_iter().next().unwrap();
                if arg.ty.scalar() != Some(Scalar::Bool) || matches!(arg.ty, Type::Matrix { .. }) {
                    return err("expected a bool or vector of bools", span);
                }
                if let Type::Scalar(_) = arg.ty {
                    return Ok(arg);
                }
                if let Some(value) = arg.as_const() {
                    let parts: Vec<bool> = value
                        .components()
                        .iter()
                        .filter_map(ConstValue::as_bool)
                        .collect();
                    let r = if f == All {
                        parts.iter().all(|b| *b)
                    } else {
                        parts.iter().any(|b| *b)
                    };
                    return Ok(Expr::constant(ConstValue::Bool(r), BOOL));
                }
                Ok(Expr {
                    kind: ExprKind::Builtin(f, vec![arg]),
                    ty: BOOL,
                    reference: None,
                })
            }
            Select => {
                arity(3)?;
                let mut args = args;
                let cond = args.pop().unwrap();
                let (values, _) = self.unify_values(args, span)?;
                let mut values: Vec<Expr> = values
                    .into_iter()
                    .map(|v| self.concretize(v, span))
                    .collect::<Result<_>>()?;
                let ty = values[0].ty.clone();
                let cond_ty = match &ty {
                    Type::Vector(n, _) if cond.ty.vector_size().is_some() => {
                        Type::Vector(*n, Scalar::Bool)
                    }
                    Type::Scalar(_) | Type::Vector(..) => BOOL,
                    _ => return err("select chooses between scalars or vectors", span),
                };
                let cond = self.coerce(cond, &cond_ty, span)?;
                values.push(cond);
                Ok(Expr {
                    kind: ExprKind::Builtin(f, values),
                    ty,
                    reference: None,
                })
            }
            CountOneBits | ReverseBits | FirstLeadingBit | FirstTrailingBit | ExtractBits
            | InsertBits => {
                let fixed = match f {
                    ExtractBits => 2,
                    InsertBits => 2,
                    _ => 0,
                };
                let values = match f {
                    InsertBits => 2,
                    _ => 1,
                };
                arity(values + fixed)?;
                let mut args = args;
                let tail: Vec<Expr> = args.split_off(values);
                if !args.iter().all(|a| int_like(&a.ty)) {
                    return err("expected integer arguments", span);
                }
                let (args, _) = self.unify_values(args, span)?;
                let mut args: Vec<Expr> = args
                    .into_iter()
                    .map(|a| self.concretize(a, span))
                    .collect::<Result<_>>()?;
                for t in tail {
                    args.push(self.coerce(t, &U32, span)?);
                }
                let ty = args[0].ty.clone();
                Ok(Expr {
                    kind: ExprKind::Builtin(f, args),
                    ty,
                    reference: None,
                })
            }
            Pack4x8Snorm | Pack4x8Unorm | Pack2x16Snorm | Pack2x16Unorm | Pack2x16Float => {
                arity(1)?;
                let n = if matches!(f, Pack4x8Snorm | Pack4x8Unorm) {
                    4
                } else {
                    2
                };
                let arg = self.coerce(
                    args.into_iter().next().unwrap(),
                    &Type::Vector(n, Scalar::F32),
                    span,
                )?;
                Ok(Expr {
                    kind: ExprKind::Builtin(f, vec![arg]),
                    ty: U32,
                    reference: None,
                })
            }
            Unpack4x8Snorm | Unpack4x8Unorm | Unpack2x16Snorm | Unpack2x16Unorm
            | Unpack2x16Float => {
                arity(1)?;
                let n = if matches!(f, Unpack4x8Snorm | Unpack4x8Unorm) {
                    4
                } else {
                    2
                };
                let arg = self.coerce(args.into_iter().next().unwrap(), &U32, span)?;
                Ok(Expr {
                    kind: ExprKind::Builtin(f, vec![arg]),
                    ty: Type::Vector(n, Scalar::F32),
                    reference: None,
                })
            }
            WorkgroupBarrier | StorageBarrier => {
                arity(0)?;
                Ok(Expr {
                    kind: ExprKind::Builtin(f, Vec::new()),
                    ty: Type::Void,
                    reference: None,
                })
            }
            ArrayLength => {
                arity(1)?;
                let arg = args.into_iter().next().unwrap();
                match &arg.ty {
                    Type::Pointer {
                        space: AddressSpace::Storage,
                        pointee,
                        ..
                    } if matches!(**pointee, Type::Array { count: None, .. }) => {}
                    _ => return err("arrayLength takes a pointer to a runtime-sized array", span),
                }
                Ok(Expr {
                    kind: ExprKind::Builtin(f, vec![arg]),
                    ty: U32,
                    reference: None,
                })
            }
        }
    }

    /// Abstract integers become abstract floats (constant arguments) or `f32` (runtime ones),
    /// as WGSL's overload resolution does for float-only builtins.
    fn floatify(&self, args: Vec<Expr>, scalar: Scalar, span: Span) -> Result<Vec<Expr>> {
        let all_const = args.iter().all(|a| a.as_const().is_some());
        let target = match scalar {
            Scalar::AbstractInt | Scalar::AbstractFloat if all_const => Scalar::AbstractFloat,
            Scalar::AbstractInt | Scalar::AbstractFloat => Scalar::F32,
            s => s,
        };
        args.into_iter()
            .map(|a| {
                let ty = a.ty.with_scalar(target);
                self.coerce(a, &ty, span)
            })
            .collect()
    }

    /// Folds component-wise builtins of constant arguments, and otherwise concretizes them.
    fn finish_builtin(
        &self,
        f: BuiltinFn,
        args: Vec<Expr>,
        ret: Option<Type>,
        span: Span,
    ) -> Result<Expr> {
        let ty = ret.clone().unwrap_or_else(|| args[0].ty.clone());
        if args.iter().all(|a| a.as_const().is_some())
            && matches!(ty, Type::Scalar(_) | Type::Vector(..))
        {
            let consts: Vec<&ConstValue> = args.iter().map(|a| a.as_const().unwrap()).collect();
            if let Some(value) = fold_builtin(f, &consts, ty.scalar().unwrap()) {
                if ty.is_abstract()
                    || value
                        .components()
                        .iter()
                        .all(|c| !matches!(c, ConstValue::Float(v) if !v.is_finite()))
                {
                    return Ok(Expr::constant(value, ty));
                }
            }
        }
        if ty.is_abstract() || args.iter().any(|a| a.ty.is_abstract()) {
            let args: Vec<Expr> = args
                .into_iter()
                .map(|a| self.concretize(a, span))
                .collect::<Result<_>>()?;
            return self.finish_builtin(f, args, ret.map(|t| t.concretize()), span);
        }
        Ok(Expr {
            kind: ExprKind::Builtin(f, args),
            ty,
            reference: None,
        })
    }

    // ---------------------------------------------------------------------------------------
    // Statements

    fn push_scope(&mut self) {
        self.ctx().scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.ctx().scopes.pop();
    }

    fn bind(&mut self, name: &str, binding: LocalBinding, span: Span) -> Result<()> {
        let scope = self.ctx().scopes.last_mut().unwrap();
        if scope.insert(name.to_string(), binding).is_some() {
            return err(format!("redeclaration of `{name}` in the same scope"), span);
        }
        Ok(())
    }

    fn block(&mut self, block: &ast::Block) -> Result<Vec<Stmt>> {
        self.push_scope();
        let result = self.stmts(&block.stmts);
        self.pop_scope();
        result
    }

    fn stmts(&mut self, stmts: &[ast::Stmt]) -> Result<Vec<Stmt>> {
        let mut out = Vec::new();
        for s in stmts {
            self.stmt(s, &mut out)?;
        }
        Ok(out)
    }

    fn new_local(&mut self, name: &str, ty: Type, mutable: bool, span: Span) -> Result<usize> {
        let ctx = self.ctx();
        let index = ctx.locals.len();
        ctx.locals.push(ir::Local {
            name: name.to_string(),
            ty,
            mutable,
        });
        self.bind(name, LocalBinding::Local(index), span)?;
        Ok(index)
    }

    fn assignable(&self, target: &Expr, span: Span) -> Result<()> {
        match target.reference {
            Some((_, Access::Write | Access::ReadWrite)) => Ok(()),
            Some(_) => err("cannot assign to a read-only value", span),
            None => err(
                "cannot assign to a value; only variables, members and elements",
                span,
            ),
        }
    }

    fn stmt(&mut self, s: &ast::Stmt, out: &mut Vec<Stmt>) -> Result<()> {
        let span = s.span;
        match &s.kind {
            S::Block(block) => out.push(Stmt::Block(self.block(block)?)),
            S::Let(d) => {
                let Some(init) = &d.init else {
                    return err("`let` needs an initializer", span);
                };
                let value = self.expr(init)?;
                let value = match &d.ty {
                    Some(ty) => {
                        let ty = self.resolve_type(ty)?;
                        self.coerce(value, &ty, init.span)?
                    }
                    None => self.concretize(value, init.span)?,
                };
                if !value.ty.is_constructible(self.structs())
                    && !matches!(value.ty, Type::Pointer { .. })
                {
                    return err(
                        format!("`let` cannot hold {}", self.type_name(&value.ty)),
                        span,
                    );
                }
                let index = self.new_local(&d.name, value.ty.clone(), false, span)?;
                out.push(Stmt::Let(index, value));
            }
            S::Var(d) => {
                if d.space.as_deref().is_some_and(|s| s != "function") {
                    return err(
                        "function-scope variables are in the `function` address space",
                        span,
                    );
                }
                let declared = match &d.ty {
                    Some(ty) => Some(self.resolve_type(ty)?),
                    None => None,
                };
                let init = match &d.init {
                    Some(init) => {
                        let value = self.expr(init)?;
                        Some(match &declared {
                            Some(ty) => self.coerce(value, ty, init.span)?,
                            None => self.concretize(value, init.span)?,
                        })
                    }
                    None => None,
                };
                let ty = match (declared, &init) {
                    (Some(ty), _) => ty,
                    (None, Some(init)) => init.ty.clone(),
                    (None, None) => return err("`var` needs a type or an initializer", span),
                };
                if !ty.is_constructible(self.structs()) {
                    return err(
                        format!("variables cannot hold {}", self.type_name(&ty)),
                        span,
                    );
                }
                let index = self.new_local(&d.name, ty, true, span)?;
                out.push(Stmt::Var(index, init));
            }
            S::Const(d) => {
                let Some(init) = &d.init else {
                    return err("`const` needs an initializer", span);
                };
                let value = self.expr(init)?;
                let value = match &d.ty {
                    Some(ty) => {
                        let ty = self.resolve_type(ty)?;
                        self.coerce(value, &ty, init.span)?
                    }
                    None => self.value(value),
                };
                let ExprKind::Const(constant) = value.kind else {
                    return err(
                        "`const` initializers must be constant expressions",
                        init.span,
                    );
                };
                self.bind(&d.name, LocalBinding::Const(constant, value.ty), span)?;
            }
            S::Assign { lhs, op, rhs } => {
                let target = self.expr(lhs)?;
                self.assignable(&target, lhs.span)?;
                let value = self.expr(rhs)?;
                match op {
                    None => {
                        let value = self.coerce(value, &target.ty, rhs.span)?;
                        out.push(Stmt::Store { target, value });
                    }
                    Some(op) => out.push(self.update(target, *op, value, span)?),
                }
            }
            S::Increment(lhs) | S::Decrement(lhs) => {
                let target = self.expr(lhs)?;
                self.assignable(&target, lhs.span)?;
                if !matches!(target.ty, Type::Scalar(s) if s.is_integer()) {
                    return err("`++` and `--` need an integer variable", span);
                }
                let one = Expr::constant(ConstValue::Int(1), target.ty.clone());
                let op = if matches!(s.kind, S::Increment(_)) {
                    BinaryOp::Add
                } else {
                    BinaryOp::Sub
                };
                out.push(Stmt::Update {
                    target,
                    op,
                    value: one,
                });
            }
            S::Phony(e) => {
                let value = self.expr(e)?;
                let value = self.concretize(value, e.span)?;
                if value.as_const().is_none() {
                    out.push(Stmt::Eval(value));
                }
            }
            S::Call(e) => {
                let A::Call(callee, args) = &e.kind else {
                    unreachable!()
                };
                let call = self.call(callee, args, e.span)?;
                if !matches!(call.kind, ExprKind::Call(..) | ExprKind::Builtin(..)) {
                    return err("only function calls can be statements", span);
                }
                out.push(Stmt::Eval(call));
            }
            S::Return(value) => {
                let ret = self.ctx().ret.clone();
                let value = match (value, &ret) {
                    (None, Type::Void) => None,
                    (None, _) => return err("this function must return a value", span),
                    (Some(_), Type::Void) => {
                        return err("this function does not return a value", span)
                    }
                    (Some(e), ty) => {
                        let value = self.expr(e)?;
                        Some(self.coerce(value, ty, e.span)?)
                    }
                };
                out.push(Stmt::Return(value));
            }
            S::If {
                cond,
                then,
                otherwise,
            } => {
                let c = self.expr(cond)?;
                let c = self.coerce(c, &BOOL, cond.span)?;
                let then = self.block(then)?;
                let otherwise = match otherwise {
                    Some(stmt) => {
                        let mut out = Vec::new();
                        self.stmt(stmt, &mut out)?;
                        out
                    }
                    None => Vec::new(),
                };
                out.push(Stmt::If {
                    cond: c,
                    then,
                    otherwise,
                });
            }
            S::Switch { selector, cases } => {
                let sel = self.expr(selector)?;
                let sel = self.concretize(sel, selector.span)?;
                if !matches!(sel.ty, Type::Scalar(Scalar::I32 | Scalar::U32)) {
                    return err("switch selectors must be i32 or u32", selector.span);
                }
                let mut seen = BTreeSet::new();
                let mut has_default = false;
                let mut out_cases = Vec::new();
                self.ctx().breakable += 1;
                for case in cases {
                    let mut selectors = Vec::new();
                    for s in &case.selectors {
                        match s {
                            None => {
                                if std::mem::replace(&mut has_default, true) {
                                    return err("a switch has at most one default", span);
                                }
                                selectors.push(None);
                            }
                            Some(e) => {
                                let v = self.expr(e)?;
                                let v = self.coerce(v, &sel.ty, e.span)?;
                                let Some(ConstValue::Int(value)) = v.as_const() else {
                                    return err("case selectors must be constant", e.span);
                                };
                                if !seen.insert(*value) {
                                    return err(format!("duplicate case {value}"), e.span);
                                }
                                selectors.push(Some(*value));
                            }
                        }
                    }
                    let body = self.block(&case.body)?;
                    out_cases.push(ir::SwitchCase { selectors, body });
                }
                self.ctx().breakable -= 1;
                if !has_default {
                    return err("a switch needs a default case", span);
                }
                out.push(Stmt::Switch {
                    selector: sel,
                    cases: out_cases,
                });
            }
            S::Loop { body, continuing } => {
                self.enter_loop();
                self.push_scope();
                let body_stmts = self.stmts(&body.stmts);
                // The continuing block sees the loop body's declarations.
                let cont = match (&body_stmts, continuing) {
                    (Ok(_), Some(c)) => Some(self.continuing(c)),
                    _ => None,
                };
                self.pop_scope();
                self.exit_loop();
                let body = body_stmts?;
                let (continuing, break_if) = match cont {
                    Some(c) => c?,
                    None => (Vec::new(), None),
                };
                out.push(Stmt::Loop {
                    body,
                    continuing,
                    break_if,
                });
            }
            S::For {
                init,
                cond,
                update,
                body,
            } => {
                self.push_scope();
                let mut block = Vec::new();
                if let Some(init) = init {
                    self.stmt(init, &mut block)?;
                }
                let cond = match cond {
                    Some(c) => {
                        let v = self.expr(c)?;
                        Some(self.coerce(v, &BOOL, c.span)?)
                    }
                    None => None,
                };
                let mut continuing = Vec::new();
                if let Some(update) = update {
                    self.stmt(update, &mut continuing)?;
                }
                self.enter_loop();
                let body = self.block(body);
                self.exit_loop();
                self.pop_scope();
                let mut loop_body = Vec::new();
                if let Some(cond) = cond {
                    loop_body.push(break_unless(cond));
                }
                loop_body.extend(body?);
                block.push(Stmt::Loop {
                    body: loop_body,
                    continuing,
                    break_if: None,
                });
                out.push(Stmt::Block(block));
            }
            S::While { cond, body } => {
                let c = self.expr(cond)?;
                let c = self.coerce(c, &BOOL, cond.span)?;
                self.enter_loop();
                let body = self.block(body);
                self.exit_loop();
                let mut loop_body = vec![break_unless(c)];
                loop_body.extend(body?);
                out.push(Stmt::Loop {
                    body: loop_body,
                    continuing: Vec::new(),
                    break_if: None,
                });
            }
            S::Break => {
                if self.ctx().breakable == 0 {
                    return err("`break` outside a loop or switch", span);
                }
                out.push(Stmt::Break);
            }
            S::Continue => {
                if self.ctx().loops == 0 {
                    return err("`continue` outside a loop", span);
                }
                out.push(Stmt::Continue);
            }
            S::Discard => out.push(Stmt::Discard),
            S::ConstAssert(e) => self.const_assert(e)?,
        }
        Ok(())
    }

    fn enter_loop(&mut self) {
        let ctx = self.ctx();
        ctx.loops += 1;
        ctx.breakable += 1;
    }

    fn exit_loop(&mut self) {
        let ctx = self.ctx();
        ctx.loops -= 1;
        ctx.breakable -= 1;
    }

    fn continuing(&mut self, c: &ast::Continuing) -> Result<(Vec<Stmt>, Option<Expr>)> {
        self.push_scope();
        let stmts = self.stmts(&c.body.stmts);
        let break_if = match (&stmts, &c.break_if) {
            (Ok(_), Some(e)) => {
                let v = self.expr(e);
                Some(v.and_then(|v| self.coerce(v, &BOOL, e.span)))
            }
            _ => None,
        };
        self.pop_scope();
        Ok((stmts?, break_if.transpose()?))
    }

    fn update(&mut self, target: Expr, op: BinaryOp, value: Expr, span: Span) -> Result<Stmt> {
        let current = self.value(target.clone());
        let combined = self.binary(op, current, value, span)?;
        if combined.ty != target.ty {
            return err(
                format!(
                    "compound assignment would change the type to {}",
                    self.type_name(&combined.ty)
                ),
                span,
            );
        }
        let ExprKind::Binary(_, _, rhs) = combined.kind else {
            return err("unsupported compound assignment", span);
        };
        Ok(Stmt::Update {
            target,
            op,
            value: *rhs,
        })
    }
}

enum Ctor {
    Type(Type),
    Vector(u8),
    Matrix(u8, u8),
    Array,
}

fn break_unless(cond: Expr) -> Stmt {
    let negated = match cond.kind {
        ExprKind::Const(ConstValue::Bool(b)) => Expr::constant(ConstValue::Bool(!b), BOOL),
        _ => Expr {
            kind: ExprKind::Unary(UnaryOp::Not, Box::new(cond)),
            ty: BOOL,
            reference: None,
        },
    };
    Stmt::If {
        cond: negated,
        then: vec![Stmt::Break],
        otherwise: Vec::new(),
    }
}

fn swizzle(name: &str, size: u8) -> Option<Vec<u8>> {
    if name.is_empty() || name.len() > 4 {
        return None;
    }
    let sets = ["xyzw", "rgba"];
    sets.iter().find_map(|set| {
        name.chars()
            .map(|c| set.find(c).map(|i| i as u8).filter(|&i| i < size))
            .collect::<Option<Vec<u8>>>()
    })
}

fn vector_alias(name: &str) -> Option<(u8, Scalar)> {
    let rest = name.strip_prefix("vec")?;
    let mut chars = rest.chars();
    let n = chars.next()?.to_digit(10)? as u8;
    if !(2..=4).contains(&n) {
        return None;
    }
    let scalar = match chars.as_str() {
        "f" => Scalar::F32,
        "i" => Scalar::I32,
        "u" => Scalar::U32,
        "h" => Scalar::F16,
        _ => return None,
    };
    Some((n, scalar))
}

fn vector_generic(name: &str) -> Option<u8> {
    match name {
        "vec2" => Some(2),
        "vec3" => Some(3),
        "vec4" => Some(4),
        _ => None,
    }
}

fn matrix_generic(name: &str) -> Option<(u8, u8)> {
    let rest = name.strip_prefix("mat")?;
    let bytes = rest.as_bytes();
    if bytes.len() != 3 || bytes[1] != b'x' {
        return None;
    }
    let c = (bytes[0] as char).to_digit(10)? as u8;
    let r = (bytes[2] as char).to_digit(10)? as u8;
    ((2..=4).contains(&c) && (2..=4).contains(&r)).then_some((c, r))
}

fn matrix_alias(name: &str) -> Option<(u8, u8, Scalar)> {
    let scalar = match name.chars().last()? {
        'f' => Scalar::F32,
        'h' => Scalar::F16,
        _ => return None,
    };
    let (c, r) = matrix_generic(&name[..name.len() - 1])?;
    Some((c, r, scalar))
}

fn sampled_texture(name: &str) -> Option<(TextureDim, bool, bool)> {
    Some(match name {
        "texture_1d" => (TextureDim::D1, false, false),
        "texture_2d" => (TextureDim::D2, false, false),
        "texture_2d_array" => (TextureDim::D2, true, false),
        "texture_3d" => (TextureDim::D3, false, false),
        "texture_cube" => (TextureDim::Cube, false, false),
        "texture_cube_array" => (TextureDim::Cube, true, false),
        "texture_multisampled_2d" => (TextureDim::D2, false, true),
        _ => return None,
    })
}

fn depth_texture(name: &str) -> Option<(TextureDim, bool, bool)> {
    Some(match name {
        "texture_depth_2d" => (TextureDim::D2, false, false),
        "texture_depth_2d_array" => (TextureDim::D2, true, false),
        "texture_depth_cube" => (TextureDim::Cube, false, false),
        "texture_depth_cube_array" => (TextureDim::Cube, true, false),
        "texture_depth_multisampled_2d" => (TextureDim::D2, false, true),
        _ => return None,
    })
}

fn storage_texture(name: &str) -> Option<(TextureDim, bool)> {
    Some(match name {
        "texture_storage_1d" => (TextureDim::D1, false),
        "texture_storage_2d" => (TextureDim::D2, false),
        "texture_storage_2d_array" => (TextureDim::D2, true),
        "texture_storage_3d" => (TextureDim::D3, false),
        _ => return None,
    })
}

/// Constant evaluation of component-wise builtins.
fn fold_builtin(f: BuiltinFn, args: &[&ConstValue], scalar: Scalar) -> Option<ConstValue> {
    use BuiltinFn::*;
    let len = args[0].components().len();
    let is_vector = matches!(args[0], ConstValue::Composite(_));
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let parts: Vec<&ConstValue> = args.iter().map(|a| &a.components()[i]).collect();
        let value = match (scalar.is_float(), f) {
            (true, _) => {
                let x: Vec<f64> = parts
                    .iter()
                    .map(|p| match p {
                        ConstValue::Float(v) => Some(*v),
                        ConstValue::Int(v) => Some(*v as f64),
                        _ => None,
                    })
                    .collect::<Option<_>>()?;
                let r = match f {
                    Abs => x[0].abs(),
                    Acos => x[0].acos(),
                    Acosh => x[0].acosh(),
                    Asin => x[0].asin(),
                    Asinh => x[0].asinh(),
                    Atan => x[0].atan(),
                    Atanh => x[0].atanh(),
                    Atan2 => x[0].atan2(x[1]),
                    Ceil => x[0].ceil(),
                    Clamp => x[0].max(x[1]).min(x[2]),
                    Cos => x[0].cos(),
                    Cosh => x[0].cosh(),
                    Degrees => x[0].to_degrees(),
                    Exp => x[0].exp(),
                    Exp2 => x[0].exp2(),
                    Floor => x[0].floor(),
                    Fma => x[0].mul_add(x[1], x[2]),
                    Fract => x[0] - x[0].floor(),
                    InverseSqrt => 1.0 / x[0].sqrt(),
                    Log => x[0].ln(),
                    Log2 => x[0].log2(),
                    Max => x[0].max(x[1]),
                    Min => x[0].min(x[1]),
                    Mix => x[0] * (1.0 - x[2]) + x[1] * x[2],
                    Pow => x[0].powf(x[1]),
                    Radians => x[0].to_radians(),
                    Round => x[0].round_ties_even(),
                    Saturate => x[0].clamp(0.0, 1.0),
                    Sign => {
                        if x[0] > 0.0 {
                            1.0
                        } else if x[0] < 0.0 {
                            -1.0
                        } else {
                            0.0
                        }
                    }
                    Sin => x[0].sin(),
                    Sinh => x[0].sinh(),
                    Smoothstep => {
                        let t = ((x[2] - x[0]) / (x[1] - x[0])).clamp(0.0, 1.0);
                        t * t * (3.0 - 2.0 * t)
                    }
                    Sqrt => x[0].sqrt(),
                    Step => {
                        if x[1] >= x[0] {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    Tan => x[0].tan(),
                    Tanh => x[0].tanh(),
                    Trunc => x[0].trunc(),
                    _ => return None,
                };
                if !r.is_finite() {
                    return None;
                }
                let r = if scalar == Scalar::F32 {
                    r as f32 as f64
                } else {
                    r
                };
                ConstValue::Float(r)
            }
            (false, Abs | Min | Max | Clamp | Sign) => {
                let x: Vec<i64> = parts.iter().map(|p| p.as_int()).collect::<Option<_>>()?;
                ConstValue::Int(match f {
                    Abs if scalar == Scalar::I32 => (x[0] as i32).wrapping_abs() as i64,
                    Abs => x[0].abs(),
                    Min => x[0].min(x[1]),
                    Max => x[0].max(x[1]),
                    Clamp => x[0].max(x[1]).min(x[2]),
                    _ => x[0].signum(),
                })
            }
            _ => return None,
        };
        if matches!(f, Length | Distance | Dot | Cross | Normalize) {
            return None;
        }
        out.push(value);
    }
    Some(if is_vector {
        ConstValue::Composite(out)
    } else {
        out.pop()?
    })
}

#[cfg(test)]
mod tests {
    use crate::compile;
    use crate::types::*;

    fn fails(source: &str, message: &str) {
        match compile(source) {
            Ok(_) => panic!("expected an error containing {message:?}"),
            Err(e) => assert!(
                e.message.contains(message),
                "{:?} does not contain {message:?}",
                e.message
            ),
        }
    }

    #[test]
    fn abstract_constants_fold() {
        let module =
            compile("const a = 1 + 2 * 3; const b: f32 = a / 2.0; const c = vec2(1, 2.5) * 2;")
                .unwrap();
        assert!(module.functions.is_empty());
    }

    #[test]
    fn out_of_order_declarations() {
        let module =
            compile("fn f() -> S { return S(N); } struct S { x: array<f32, N> } const N = 4;");
        // `S(N)` passes an integer where an array is expected.
        assert!(module.is_err());
        compile(
            "fn f() -> S { return S(array<f32, N>()); } struct S { x: array<f32, N> } const N = 4;",
        )
        .unwrap();
    }

    #[test]
    fn type_errors() {
        fails("fn f() { let x: f32 = 1u; }", "expected f32, found u32");
        fails("fn f() { let x = 1i + 1u; }", "cannot apply `+`");
        fails("fn f() { let x = 1; x = 2; }", "cannot assign");
        fails("fn f() { let x = y; }", "unresolved identifier `y`");
        fails("const x: i32 = 3000000000;", "does not fit");
        fails(
            "fn f() { let v = vec3f(1.0, 2.0); }",
            "expected 3 components",
        );
        fails(
            "fn f() { let a = array<f32, 2>(); let b = a[2]; }",
            "out of bounds",
        );
        fails("fn f() { break; }", "outside a loop");
        fails("fn f() { f(); }", "recursion");
    }

    #[test]
    fn entry_points_and_bindings() {
        let source = r#"
            struct Uniforms { color: vec4f, scale: f32 }
            @group(0) @binding(0) var<uniform> u: Uniforms;
            @group(0) @binding(1) var<storage, read_write> data: array<u32>;
            @group(1) @binding(0) var t: texture_2d<f32>;
            @group(1) @binding(1) var s: sampler;

            struct VertexOut { @builtin(position) position: vec4f, @location(0) uv: vec2f }

            @vertex fn vs(@builtin(vertex_index) i: u32) -> VertexOut {
                var out: VertexOut;
                out.position = vec4f(f32(i), 0.0, 0.0, 1.0) * u.scale;
                out.uv = vec2f(0.5);
                return out;
            }

            @fragment fn fs(in: VertexOut) -> @location(0) vec4f { return u.color * in.uv.x; }

            @compute @workgroup_size(64) fn cs(@builtin(global_invocation_id) id: vec3u) {
                data[id.x] = arrayLength(&data);
            }
        "#;
        let module = compile(source).unwrap();
        assert_eq!(module.entry_points.len(), 3);
        let vs = module.entry_point("vs").unwrap();
        assert_eq!(vs.outputs.len(), 2);
        assert_eq!(module.bindings_for(vs).count(), 1);
        let cs = module.entry_point("cs").unwrap();
        assert_eq!(cs.workgroup_size, [64, 1, 1]);
        assert_eq!(module.bindings.len(), 4);
        assert_eq!(module.structs[0].size, 32);
        assert!(matches!(
            module.bindings[1].kind,
            crate::ResourceKind::StorageBuffer {
                read_only: false,
                min_size: 4
            }
        ));
        let _ = Scalar::F32;
    }

    #[test]
    fn vertex_needs_position() {
        fails(
            "@vertex fn vs() -> @location(0) vec4f { return vec4f(); }",
            "must output @builtin(position)",
        );
    }
}
