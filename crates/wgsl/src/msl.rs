//! MSL back end for Metal.
//!
//! The output targets MSL 2.3 (macOS 11). Metal has no module-scope variables, so every module
//! global a function touches, directly or through calls, becomes an extra reference parameter,
//! and each entry point declares its resources as arguments with explicit `[[buffer(n)]]`,
//! `[[texture(n)]]` and `[[sampler(n)]]` indices, reported in [`MslEntryPoint::resources`].
//!
//! Host-shareable structs are laid out with WGSL's offsets: `vec3` members become
//! `packed_float3` and gaps become explicit padding, so buffers read the same bytes as on
//! Vulkan. Fixed-size arrays are wrapped in structs to make them values. Runtime-sized arrays
//! take their length from a sizes buffer that the GPU layer fills with each binding's size.
//!
//! The undefined behaviour WGSL defines away is emulated as in the SPIR-V back end: integer
//! division by zero, shift amounts, out-of-bounds texture loads and signed overflow. Array
//! indices are also clamped, which Vulkan's robust buffer access does for the SPIR-V output.

use crate::ast::{BinaryOp, UnaryOp};
use crate::consts::ConstValue;
use crate::ir::*;
use crate::types::*;
use crate::{Error, Span};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

type Result<T> = std::result::Result<T, Error>;

/// Options for [`Module::to_msl`].
#[derive(Debug, Clone, Default)]
pub struct MslOptions {
    /// Writes a `[[point_size]]` of 1 from vertex entry points, which Metal needs to draw point
    /// lists. Set it when compiling for a `point-list` pipeline.
    pub point_size: bool,
}

/// The MSL source for a whole module and how its entry points bind resources.
#[derive(Debug, Clone)]
pub struct MslOutput {
    pub source: String,
    pub entry_points: Vec<MslEntryPoint>,
    /// The storage bindings that hold runtime-sized arrays, as `(group, binding)`. Element `i`
    /// of an entry point's sizes buffer is the bound byte size of `buffer_sizes[i]`.
    pub buffer_sizes: Vec<(u32, u32)>,
}

impl MslOutput {
    pub fn entry_point(&self, name: &str) -> Option<&MslEntryPoint> {
        self.entry_points.iter().find(|e| e.name == name)
    }
}

#[derive(Debug, Clone)]
pub struct MslEntryPoint {
    /// The WGSL name.
    pub name: String,
    /// The MSL function to look up in the compiled library.
    pub function: String,
    pub stage: Stage,
    pub workgroup_size: [u32; 3],
    /// Where each resource binding the entry point uses goes in Metal's argument tables.
    pub resources: Vec<MslResource>,
    /// The buffer index of the `uint` array of buffer sizes, when the entry point needs it.
    pub sizes_buffer: Option<u32>,
    /// The first buffer index left free, where a vertex entry point's vertex buffers go.
    pub vertex_buffer_base: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MslResource {
    pub group: u32,
    pub binding: u32,
    pub slot: MslSlot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MslSlot {
    Buffer(u32),
    Texture(u32),
    Sampler(u32),
}

impl Module {
    /// Lowers the whole module, every entry point included, to MSL source.
    pub fn to_msl(&self, options: &MslOptions) -> Result<MslOutput> {
        Writer::new(self, options).write()
    }
}

fn unsupported<T>(what: &str) -> Result<T> {
    Err(Error::new(
        format!("{what} is not supported by the MSL back end yet"),
        Span::default(),
    ))
}

/// A user identifier. Every one gains a trailing `_`, so none collides with an MSL keyword or
/// library name, nor with the writer's own names, which never end in `_`.
fn ident(name: &str) -> String {
    format!("{name}_")
}

fn scalar_name(s: Scalar) -> Result<&'static str> {
    Ok(match s.concretize() {
        Scalar::Bool => "bool",
        Scalar::I32 => "int",
        Scalar::U32 => "uint",
        Scalar::F32 => "float",
        Scalar::F16 => return unsupported("f16"),
        _ => unreachable!(),
    })
}

fn float_literal(value: f64) -> String {
    let text = format!("{:?}f", value as f32);
    if value.is_sign_negative() {
        format!("({text})")
    } else {
        text
    }
}

fn int_literal(value: i64, scalar: Scalar) -> String {
    match scalar {
        Scalar::U32 => format!("{}u", value as u32),
        _ => {
            let v = value as i32;
            if v == i32::MIN {
                "(-2147483647 - 1)".into()
            } else if v < 0 {
                format!("({v})")
            } else {
                v.to_string()
            }
        }
    }
}

fn space_qualifier(space: AddressSpace, access: Access) -> &'static str {
    match space {
        AddressSpace::Function | AddressSpace::Private => "thread",
        AddressSpace::Workgroup => "threadgroup",
        AddressSpace::Uniform => "constant",
        AddressSpace::Storage if access == Access::Read => "const device",
        AddressSpace::Storage => "device",
        AddressSpace::Handle => "",
    }
}

fn is_runtime_array(ty: &Type) -> bool {
    matches!(ty, Type::Array { count: None, .. })
}

fn member_io(member: &StructMember) -> IoBinding {
    match (member.builtin, member.location) {
        (Some(b), _) => IoBinding::Builtin(b),
        (None, Some(location)) => IoBinding::Location {
            location,
            interpolation: member.interpolation,
        },
        _ => unreachable!("checked entry-point struct"),
    }
}

/// Whether evaluating `e` can have side effects, which only user function calls do.
fn has_call(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Call(..) => true,
        ExprKind::Const(_) | ExprKind::Local(_) | ExprKind::Param(_) | ExprKind::Global(_) => false,
        ExprKind::Load(x)
        | ExprKind::Unary(_, x)
        | ExprKind::Splat(x)
        | ExprKind::Convert(x)
        | ExprKind::Bitcast(x)
        | ExprKind::Member(x, _)
        | ExprKind::Swizzle(x, _)
        | ExprKind::AddrOf(x)
        | ExprKind::Deref(x) => has_call(x),
        ExprKind::Binary(_, a, b) | ExprKind::Index(a, b) => has_call(a) || has_call(b),
        ExprKind::Construct(parts) | ExprKind::Builtin(_, parts) => parts.iter().any(has_call),
    }
}

/// The MSL attribute for a fragment input's interpolation.
fn interpolation_attribute(ty: &Type, interpolation: Option<Interpolation>) -> &'static str {
    let integer = ty.scalar().is_some_and(|s| s.is_integer());
    match interpolation {
        _ if integer => ", flat",
        Some(Interpolation::Flat) => ", flat",
        Some(Interpolation::Linear(Sampling::Center)) => ", center_no_perspective",
        Some(Interpolation::Linear(Sampling::Centroid)) => ", centroid_no_perspective",
        Some(Interpolation::Linear(Sampling::Sample)) => ", sample_no_perspective",
        Some(Interpolation::Perspective(Sampling::Centroid)) => ", centroid_perspective",
        Some(Interpolation::Perspective(Sampling::Sample)) => ", sample_perspective",
        Some(Interpolation::Perspective(Sampling::Center)) | None => "",
    }
}

#[derive(Default)]
struct FnState {
    function: usize,
    indent: usize,
    loops: usize,
}

/// What an entry point's interface collects while its wrapper is written.
#[derive(Default)]
struct Interface {
    /// `[[stage_in]]` struct members.
    inputs: Vec<String>,
    /// Builtin arguments, by builtin, as (name, declaration).
    builtins: Vec<(Builtin, String, String)>,
}

impl Interface {
    fn builtin(&mut self, b: Builtin) -> String {
        if let Some((_, name, _)) = self.builtins.iter().find(|(x, ..)| *x == b) {
            return name.clone();
        }
        let (ty, name, attribute) = match b {
            Builtin::VertexIndex => ("uint", "_vertex_index", "vertex_id"),
            Builtin::InstanceIndex => ("uint", "_instance_index", "instance_id"),
            Builtin::Position => ("float4", "_position", "position"),
            Builtin::FrontFacing => ("bool", "_front_facing", "front_facing"),
            Builtin::SampleIndex => ("uint", "_sample_index", "sample_id"),
            Builtin::SampleMask => ("uint", "_sample_mask", "sample_mask"),
            Builtin::LocalInvocationId => ("uint3", "_local_id", "thread_position_in_threadgroup"),
            Builtin::LocalInvocationIndex => {
                ("uint", "_local_index", "thread_index_in_threadgroup")
            }
            Builtin::GlobalInvocationId => ("uint3", "_global_id", "thread_position_in_grid"),
            Builtin::WorkgroupId => ("uint3", "_workgroup_id", "threadgroup_position_in_grid"),
            Builtin::NumWorkgroups => ("uint3", "_num_workgroups", "threadgroups_per_grid"),
            Builtin::FragDepth => unreachable!("frag_depth is an output"),
        };
        self.builtins
            .push((b, name.to_string(), format!("{ty} {name} [[{attribute}]]")));
        name.to_string()
    }
}

struct Writer<'m> {
    module: &'m Module,
    options: &'m MslOptions,
    /// Type definitions in dependency order.
    types: String,
    type_names: HashMap<Type, String>,
    arrays: usize,
    /// Helper functions by key, each with its name and definition.
    helpers: BTreeMap<String, (String, String)>,
    /// Entry-point interface structs.
    io_types: String,
    prototypes: String,
    functions: String,
    entries: String,
    /// Every module global each function uses, through calls included, in index order.
    fn_globals: Vec<Vec<usize>>,
    /// Storage globals holding runtime-sized arrays, in sizes-buffer order.
    runtime_globals: Vec<usize>,
    f: FnState,
}

impl<'m> Writer<'m> {
    fn new(module: &'m Module, options: &'m MslOptions) -> Self {
        let mut fn_globals = vec![None; module.functions.len()];
        fn visit(module: &Module, f: usize, out: &mut Vec<Option<Vec<usize>>>) -> Vec<usize> {
            if let Some(done) = &out[f] {
                return done.clone();
            }
            let mut globals = module.functions[f].globals.clone();
            for &callee in &module.functions[f].calls {
                globals.extend(visit(module, callee, out));
            }
            globals.sort_unstable();
            globals.dedup();
            out[f] = Some(globals.clone());
            globals
        }
        for f in 0..module.functions.len() {
            visit(module, f, &mut fn_globals);
        }
        let runtime_globals = module
            .globals
            .iter()
            .enumerate()
            .filter(|(_, g)| {
                g.space == AddressSpace::Storage
                    && match &g.ty {
                        Type::Array { count: None, .. } => true,
                        Type::Struct(i) => module.structs[*i]
                            .members
                            .last()
                            .is_some_and(|m| is_runtime_array(&m.ty)),
                        _ => false,
                    }
            })
            .map(|(i, _)| i)
            .collect();
        Writer {
            module,
            options,
            types: String::new(),
            type_names: HashMap::new(),
            arrays: 0,
            helpers: BTreeMap::new(),
            io_types: String::new(),
            prototypes: String::new(),
            functions: String::new(),
            entries: String::new(),
            fn_globals: fn_globals.into_iter().map(Option::unwrap).collect(),
            runtime_globals,
            f: FnState::default(),
        }
    }

    fn write(mut self) -> Result<MslOutput> {
        let module = self.module;
        for i in 0..module.structs.len() {
            if Type::Struct(i).is_constructible(&module.structs)
                || Type::Struct(i).is_host_shareable(&module.structs)
            {
                self.ty(&Type::Struct(i))?;
            }
        }
        for f in 0..module.functions.len() {
            self.function(f)?;
        }
        let mut entry_points = Vec::new();
        for entry in &module.entry_points {
            entry_points.push(self.entry_point(entry)?);
        }
        let buffer_sizes = self
            .runtime_globals
            .iter()
            .map(|&g| {
                module.globals[g]
                    .group_binding
                    .expect("checked storage binding")
            })
            .collect();

        let mut source = String::from(
            "// Generated by shallot-wgsl.\n#include <metal_stdlib>\nusing namespace metal;\n\n",
        );
        source.push_str(&self.types);
        for (_, definition) in self.helpers.values() {
            source.push_str(definition);
            source.push('\n');
        }
        source.push_str(&self.prototypes);
        source.push('\n');
        source.push_str(&self.functions);
        source.push_str(&self.io_types);
        source.push_str(&self.entries);
        Ok(MslOutput {
            source,
            entry_points,
            buffer_sizes,
        })
    }

    // ---------------------------------------------------------------------------------------
    // Types

    fn ty(&mut self, ty: &Type) -> Result<String> {
        let ty = ty.concretize();
        if let Some(name) = self.type_names.get(&ty) {
            return Ok(name.clone());
        }
        let name = match &ty {
            Type::Void => "void".into(),
            Type::Scalar(s) => scalar_name(*s)?.into(),
            Type::Vector(n, s) => format!("{}{n}", scalar_name(*s)?),
            Type::Matrix {
                columns,
                rows,
                scalar,
            } => {
                if *scalar != Scalar::F32 {
                    return unsupported("f16 matrices");
                }
                format!("float{columns}x{rows}")
            }
            Type::Array {
                element,
                count: Some(n),
            } => {
                let element = self.ty(element)?;
                let name = format!("_array{}", self.arrays);
                self.arrays += 1;
                writeln!(self.types, "struct {name} {{\n    {element} e[{n}];\n}};\n").unwrap();
                name
            }
            // Runtime-sized arrays are only declared through pointers and trailing members,
            // which name the element type.
            Type::Array {
                element,
                count: None,
            } => return self.ty(element),
            Type::Struct(i) => self.define_struct(*i)?,
            Type::Atomic(_) => return unsupported("atomics"),
            Type::Pointer {
                space,
                pointee,
                access,
            } => {
                let pointee = self.ty(pointee)?;
                format!("{} {pointee}*", space_qualifier(*space, *access))
            }
            Type::Sampler { .. } => "sampler".into(),
            Type::Texture { dim, arrayed, kind } => texture_type(*dim, *arrayed, kind)?,
        };
        self.type_names.insert(ty, name.clone());
        Ok(name)
    }

    fn define_struct(&mut self, i: usize) -> Result<String> {
        let module = self.module;
        let st = &module.structs[i];
        let name = ident(&st.name);
        let shareable = Type::Struct(i).is_host_shareable(&module.structs);
        let mut body = String::new();
        let mut position = 0;
        for (m, member) in st.members.iter().enumerate() {
            if shareable && member.offset > position {
                writeln!(body, "    char _pad{m}[{}];", member.offset - position).unwrap();
                position = member.offset;
            }
            let member_name = ident(&member.name);
            match &member.ty {
                Type::Vector(3, s) if shareable => {
                    writeln!(body, "    packed_{}3 {member_name};", scalar_name(*s)?).unwrap();
                    position += 3 * s.size();
                }
                Type::Array {
                    element,
                    count: None,
                } => {
                    let element = self.ty(element)?;
                    writeln!(body, "    {element} {member_name}[1];").unwrap();
                    position = st.size;
                }
                ty => {
                    let ty_name = self.ty(ty)?;
                    writeln!(body, "    {ty_name} {member_name};").unwrap();
                    position += align_and_size(ty, &module.structs).1;
                }
            }
        }
        if shareable && st.size > position {
            writeln!(body, "    char _pad_end[{}];", st.size - position).unwrap();
        }
        writeln!(self.types, "struct {name} {{\n{body}}};\n").unwrap();
        Ok(name)
    }

    /// `S_{...}` for a struct value, through a generated constructor that assigns members by
    /// name, so padding members do not shift the arguments.
    fn construct_struct(&mut self, i: usize, parts: Vec<String>) -> Result<String> {
        let module = self.module;
        let st = &module.structs[i];
        let name = self.ty(&Type::Struct(i))?;
        let helper = format!("_make_{name}");
        if !self.helpers.contains_key(&helper) {
            let mut params = Vec::new();
            let mut body = format!("    {name} r = {{}};\n");
            for (m, member) in st.members.iter().enumerate() {
                params.push(format!("{} a{m}", self.ty(&member.ty)?));
                writeln!(body, "    r.{} = a{m};", ident(&member.name)).unwrap();
            }
            let definition = format!(
                "{name} {helper}({}) {{\n{body}    return r;\n}}\n",
                params.join(", ")
            );
            self.helpers
                .insert(helper.clone(), (helper.clone(), definition));
        }
        Ok(format!("{helper}({})", parts.join(", ")))
    }

    /// Registers a helper function under `key` and returns its name.
    fn helper(
        &mut self,
        key: String,
        name: String,
        definition: impl FnOnce(&str) -> String,
    ) -> String {
        if let Some((name, _)) = self.helpers.get(&key) {
            return name.clone();
        }
        let text = definition(&name);
        self.helpers.insert(key, (name.clone(), text));
        name
    }

    /// Registers a helper whose name is unique per key: `{base}{n}`.
    fn numbered_helper(
        &mut self,
        base: &str,
        key: String,
        definition: impl FnOnce(&str) -> String,
    ) -> String {
        if let Some((name, _)) = self.helpers.get(&key) {
            return name.clone();
        }
        let name = format!("{base}{}", self.helpers.len());
        self.helper(key, name, definition)
    }

    // ---------------------------------------------------------------------------------------
    // Functions

    /// Declares global `g` as a parameter of a function, by reference or, for handles, by value.
    fn global_param(&mut self, g: usize) -> Result<String> {
        let global = &self.module.globals[g];
        let name = ident(&global.name);
        Ok(match global.space {
            AddressSpace::Uniform => format!("constant {}& {name}", self.ty(&global.ty)?),
            AddressSpace::Storage => {
                let qualifier = space_qualifier(global.space, global.access);
                if is_runtime_array(&global.ty) {
                    format!("{qualifier} {}* {name}", self.ty(&global.ty)?)
                } else {
                    format!("{qualifier} {}& {name}", self.ty(&global.ty)?)
                }
            }
            AddressSpace::Private => format!("thread {}& {name}", self.ty(&global.ty)?),
            AddressSpace::Workgroup => format!("threadgroup {}& {name}", self.ty(&global.ty)?),
            AddressSpace::Handle => format!("{} {name}", self.ty(&global.ty)?),
            AddressSpace::Function => unreachable!("function-space global"),
        })
    }

    fn needs_sizes(&self, globals: &[usize]) -> bool {
        globals.iter().any(|g| self.runtime_globals.contains(g))
    }

    fn function(&mut self, index: usize) -> Result<()> {
        let module = self.module;
        let function = &module.functions[index];
        let ret = self.ty(&function.ret)?;
        let mut params = Vec::new();
        for p in &function.params {
            params.push(format!("{} {}", self.ty(&p.ty)?, ident(&p.name)));
        }
        let globals = self.fn_globals[index].clone();
        for &g in &globals {
            params.push(self.global_param(g)?);
        }
        if self.needs_sizes(&globals) {
            params.push("constant uint* _buffer_sizes".into());
        }
        let signature = format!("{ret} {}({})", ident(&function.name), params.join(", "));
        writeln!(self.prototypes, "{signature};").unwrap();

        self.f = FnState {
            function: index,
            indent: 1,
            loops: 0,
        };
        let mut body = String::new();
        for (i, local) in function.locals.iter().enumerate() {
            let ty = self.ty(&local.ty)?;
            writeln!(body, "    {ty} {};", local_name(local, i)).unwrap();
        }
        self.block(&function.body, &mut body)?;
        writeln!(self.functions, "{signature} {{\n{body}}}\n").unwrap();
        Ok(())
    }

    fn line(&self, out: &mut String, text: &str) {
        for _ in 0..self.f.indent {
            out.push_str("    ");
        }
        out.push_str(text);
        out.push('\n');
    }

    fn block(&mut self, stmts: &[Stmt], out: &mut String) -> Result<()> {
        for stmt in stmts {
            self.stmt(stmt, out)?;
        }
        Ok(())
    }

    fn nested(&mut self, stmts: &[Stmt], out: &mut String) -> Result<()> {
        self.f.indent += 1;
        let result = self.block(stmts, out);
        self.f.indent -= 1;
        result
    }

    fn local(&self, i: usize) -> String {
        local_name(&self.module.functions[self.f.function].locals[i], i)
    }

    fn stmt(&mut self, stmt: &Stmt, out: &mut String) -> Result<()> {
        match stmt {
            Stmt::Let(i, value) => {
                let value = self.expr(value)?;
                self.line(out, &format!("{} = {value};", self.local(*i)));
            }
            Stmt::Var(i, init) => {
                let ty = self.module.functions[self.f.function].locals[*i].ty.clone();
                let value = match init {
                    Some(e) => self.expr(e)?,
                    None => format!("{}{{}}", self.ty(&ty)?),
                };
                self.line(out, &format!("{} = {value};", self.local(*i)));
            }
            Stmt::Store { target, value } => {
                let target = self.lvalue(target)?;
                let value = self.expr(value)?;
                self.line(out, &format!("{target} = {value};"));
            }
            Stmt::Update { target, op, value } => {
                if has_call(target) {
                    return unsupported("compound assignment to an expression with calls");
                }
                let lvalue = self.lvalue(target)?;
                let current = self.read(target, lvalue.clone())?;
                let rhs = self.expr(value)?;
                let result = self.binary(*op, &target.ty, current, &value.ty, rhs, &target.ty)?;
                self.line(out, &format!("{lvalue} = {result};"));
            }
            Stmt::Eval(e) => {
                let e = self.expr(e)?;
                self.line(out, &format!("{e};"));
            }
            Stmt::Return(value) => match value {
                Some(e) => {
                    let e = self.expr(e)?;
                    self.line(out, &format!("return {e};"));
                }
                None => self.line(out, "return;"),
            },
            Stmt::If {
                cond,
                then,
                otherwise,
            } => {
                let cond = self.expr(cond)?;
                self.line(out, &format!("if ({cond}) {{"));
                self.nested(then, out)?;
                if otherwise.is_empty() {
                    self.line(out, "}");
                } else {
                    self.line(out, "} else {");
                    self.nested(otherwise, out)?;
                    self.line(out, "}");
                }
            }
            Stmt::Switch { selector, cases } => {
                let scalar = scalar_of(&selector.ty.concretize());
                let selector = self.expr(selector)?;
                self.line(out, &format!("switch ({selector}) {{"));
                for case in cases {
                    for value in &case.selectors {
                        match value {
                            Some(v) => {
                                self.line(out, &format!("case {}:", int_literal(*v, scalar)))
                            }
                            None => self.line(out, "default:"),
                        }
                    }
                    self.line(out, "{");
                    self.nested(&case.body, out)?;
                    self.line(out, "}");
                    self.line(out, "break;");
                }
                self.line(out, "}");
            }
            Stmt::Loop {
                body,
                continuing,
                break_if,
            } => {
                if continuing.is_empty() && break_if.is_none() {
                    self.line(out, "while (true) {");
                    self.nested(body, out)?;
                    self.line(out, "}");
                } else {
                    // `continue` must run the continuing block, so it goes at the top, skipped
                    // on the first iteration. Locals are declared at function scope, so the
                    // continuing block still sees the body's.
                    let first = format!("_first{}", self.f.loops);
                    self.f.loops += 1;
                    self.line(out, &format!("bool {first} = true;"));
                    self.line(out, "while (true) {");
                    self.f.indent += 1;
                    self.line(out, &format!("if (!{first}) {{"));
                    self.nested(continuing, out)?;
                    if let Some(cond) = break_if {
                        let cond = self.expr(cond)?;
                        self.f.indent += 1;
                        self.line(out, &format!("if ({cond}) {{ break; }}"));
                        self.f.indent -= 1;
                    }
                    self.line(out, "}");
                    self.line(out, &format!("{first} = false;"));
                    self.block(body, out)?;
                    self.f.indent -= 1;
                    self.line(out, "}");
                }
            }
            Stmt::Break => self.line(out, "break;"),
            Stmt::Continue => self.line(out, "continue;"),
            Stmt::Discard => self.line(out, "discard_fragment();"),
            Stmt::Block(stmts) => {
                self.line(out, "{");
                self.nested(stmts, out)?;
                self.line(out, "}");
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // Expressions

    /// Whether `e` names a `vec3` member of a laid-out struct, stored as a packed vector.
    fn is_packed(&self, e: &Expr) -> bool {
        let ExprKind::Member(base, _) = &e.kind else {
            return false;
        };
        matches!(e.ty, Type::Vector(3, _))
            && matches!(&base.ty, Type::Struct(i) if base.ty.is_host_shareable(&self.module.structs) && *i < self.module.structs.len())
    }

    /// Reads the memory or value `text` that `e` denotes, unpacking packed vectors.
    fn read(&mut self, e: &Expr, text: String) -> Result<String> {
        Ok(if self.is_packed(e) {
            format!("{}({text})", self.ty(&e.ty)?)
        } else {
            text
        })
    }

    /// The memory a reference expression denotes, as an lvalue.
    fn lvalue(&mut self, e: &Expr) -> Result<String> {
        match &e.kind {
            ExprKind::Local(i) => Ok(self.local(*i)),
            ExprKind::Global(g) => Ok(ident(&self.module.globals[*g].name)),
            ExprKind::Member(base, m) => {
                let base_text = self.lvalue(base)?;
                Ok(format!("{base_text}.{}", self.member_name(&base.ty, *m)))
            }
            ExprKind::Index(base, index) => self.index(base, index, true),
            ExprKind::Deref(pointer) => Ok(format!("(*{})", self.expr(pointer)?)),
            _ => unreachable!("not a reference expression"),
        }
    }

    fn member_name(&self, ty: &Type, m: u32) -> String {
        let Type::Struct(i) = ty else {
            unreachable!("member of a non-struct")
        };
        ident(&self.module.structs[*i].members[m as usize].name)
    }

    /// `base[index]`, with the index clamped to the bounds unless it is a constant the checker
    /// already bounded.
    fn index(&mut self, base: &Expr, index: &Expr, reference: bool) -> Result<String> {
        let base_text = if reference {
            self.lvalue(base)?
        } else {
            self.expr(base)?
        };
        let constant = matches!(index.as_const(), Some(ConstValue::Int(_)));
        let i = self.expr(index)?;
        let (count, wrapped) = match &base.ty {
            Type::Array { count, .. } => (*count, count.is_some()),
            Type::Vector(n, _) => (Some(*n as u32), false),
            Type::Matrix { columns, .. } => (Some(*columns as u32), false),
            _ => unreachable!("checked index base"),
        };
        let i = match count {
            Some(_) if constant => i,
            Some(n) => format!("min(uint({i}), {}u)", n - 1),
            None => {
                let length = self.runtime_length(base)?;
                format!("min(uint({i}), {length} - 1u)")
            }
        };
        Ok(if wrapped {
            format!("{base_text}.e[{i}]")
        } else {
            format!("{base_text}[{i}]")
        })
    }

    /// The element count of a runtime-sized array in a storage buffer, from the sizes buffer.
    fn runtime_length(&mut self, array: &Expr) -> Result<String> {
        let module = self.module;
        let Type::Array { element, .. } = &array.ty else {
            unreachable!("runtime_length of a non-array")
        };
        let stride = array_stride(element, &module.structs);
        let (global, offset) = match &array.kind {
            ExprKind::Global(g) => (*g, 0),
            ExprKind::Member(base, m) => match (&base.kind, &base.ty) {
                (ExprKind::Global(g), Type::Struct(i)) => {
                    (*g, module.structs[*i].members[*m as usize].offset)
                }
                _ => return unsupported("runtime-sized arrays reached through pointers"),
            },
            _ => return unsupported("runtime-sized arrays reached through pointers"),
        };
        let k = self
            .runtime_globals
            .iter()
            .position(|&g| g == global)
            .expect("runtime-sized global");
        Ok(format!("((_buffer_sizes[{k}] - {offset}u) / {stride}u)"))
    }

    fn expr(&mut self, e: &Expr) -> Result<String> {
        if let Some((space, _)) = e.reference {
            let lvalue = self.lvalue(e)?;
            return Ok(if space == AddressSpace::Handle {
                lvalue
            } else {
                format!("(&{lvalue})")
            });
        }
        let module = self.module;
        Ok(match &e.kind {
            ExprKind::Const(value) => self.constant(value, &e.ty)?,
            ExprKind::Local(i) => self.local(*i),
            ExprKind::Param(i) => ident(&module.functions[self.f.function].params[*i].name),
            ExprKind::Global(_) => unreachable!("globals are references"),
            ExprKind::Load(inner) => {
                let lvalue = self.lvalue(inner)?;
                self.read(inner, lvalue)?
            }
            ExprKind::Unary(op, x) => {
                let scalar = scalar_of(&x.ty.concretize());
                let x = self.expr(x)?;
                match op {
                    UnaryOp::Neg if scalar == Scalar::I32 => {
                        let ty = self.ty(&e.ty)?;
                        let name = self.wrapping_helper("_wneg", &ty, "-")?;
                        format!("{name}({x})")
                    }
                    UnaryOp::Neg => format!("(-{x})"),
                    UnaryOp::Not => format!("(!{x})"),
                    UnaryOp::BitNot => format!("(~{x})"),
                    _ => unreachable!("lowered unary operator"),
                }
            }
            ExprKind::Binary(op, l, r) => {
                let (lt, rt) = (self.expr(l)?, self.expr(r)?);
                self.binary(*op, &l.ty, lt, &r.ty, rt, &e.ty)?
            }
            ExprKind::Construct(parts) => {
                let ty = e.ty.concretize();
                let texts: Vec<String> =
                    parts.iter().map(|p| self.expr(p)).collect::<Result<_>>()?;
                let name = self.ty(&ty)?;
                match &ty {
                    _ if texts.is_empty() => format!("{name}{{}}"),
                    Type::Struct(i) => self.construct_struct(*i, texts)?,
                    Type::Array { .. } => format!("{name}{{{{{}}}}}", texts.join(", ")),
                    _ => format!("{name}({})", texts.join(", ")),
                }
            }
            ExprKind::Splat(x) | ExprKind::Convert(x) => {
                let ty = self.ty(&e.ty)?;
                format!("{ty}({})", self.expr(x)?)
            }
            ExprKind::Bitcast(x) => {
                let ty = self.ty(&e.ty)?;
                format!("as_type<{ty}>({})", self.expr(x)?)
            }
            ExprKind::Call(f, args) => {
                let mut texts: Vec<String> =
                    args.iter().map(|a| self.expr(a)).collect::<Result<_>>()?;
                let globals = self.fn_globals[*f].clone();
                texts.extend(globals.iter().map(|&g| ident(&module.globals[g].name)));
                if self.needs_sizes(&globals) {
                    texts.push("_buffer_sizes".into());
                }
                format!(
                    "{}({})",
                    ident(&module.functions[*f].name),
                    texts.join(", ")
                )
            }
            ExprKind::Builtin(f, args) => self.builtin(*f, args, &e.ty)?,
            ExprKind::Member(base, m) => {
                let text = format!("{}.{}", self.expr(base)?, self.member_name(&base.ty, *m));
                self.read(e, text)?
            }
            ExprKind::Swizzle(base, components) => {
                let letters: String = components
                    .iter()
                    .map(|&c| b"xyzw"[c as usize] as char)
                    .collect();
                format!("{}.{letters}", self.expr(base)?)
            }
            ExprKind::Index(base, index) => self.index(base, index, false)?,
            ExprKind::AddrOf(inner) => format!("(&{})", self.lvalue(inner)?),
            ExprKind::Deref(_) => unreachable!("dereferences are references"),
        })
    }

    fn constant(&mut self, value: &ConstValue, ty: &Type) -> Result<String> {
        let ty = ty.concretize();
        Ok(match (value, &ty) {
            (ConstValue::Bool(b), _) => b.to_string(),
            (ConstValue::Int(i), Type::Scalar(s)) if s.is_float() => float_literal(*i as f64),
            (ConstValue::Int(i), Type::Scalar(s)) => int_literal(*i, *s),
            (ConstValue::Float(f), Type::Scalar(s)) if s.is_integer() => int_literal(*f as i64, *s),
            (ConstValue::Float(f), _) => float_literal(*f),
            (ConstValue::Composite(parts), Type::Vector(_, s)) => {
                let name = self.ty(&ty)?;
                let texts: Vec<String> = parts
                    .iter()
                    .map(|p| self.constant(p, &Type::Scalar(*s)))
                    .collect::<Result<_>>()?;
                format!("{name}({})", texts.join(", "))
            }
            (ConstValue::Composite(parts), Type::Matrix { rows, scalar, .. }) => {
                let name = self.ty(&ty)?;
                let column = Type::Vector(*rows, *scalar);
                let texts: Vec<String> = parts
                    .iter()
                    .map(|p| self.constant(p, &column))
                    .collect::<Result<_>>()?;
                format!("{name}({})", texts.join(", "))
            }
            (ConstValue::Composite(parts), Type::Array { element, .. }) => {
                let name = self.ty(&ty)?;
                let texts: Vec<String> = parts
                    .iter()
                    .map(|p| self.constant(p, element))
                    .collect::<Result<_>>()?;
                format!("{name}{{{{{}}}}}", texts.join(", "))
            }
            (ConstValue::Composite(parts), Type::Struct(i)) => {
                let members: Vec<Type> = self.module.structs[*i]
                    .members
                    .iter()
                    .map(|m| m.ty.clone())
                    .collect();
                let texts: Vec<String> = parts
                    .iter()
                    .zip(&members)
                    .map(|(p, t)| self.constant(p, t))
                    .collect::<Result<_>>()?;
                self.construct_struct(*i, texts)?
            }
            (ConstValue::Int(i), _) => i.to_string(),
            (ConstValue::Composite(_), _) => unreachable!("composite constant of a scalar type"),
        })
    }

    /// `_wadd`, `_wsub`, `_wmul` or `_wneg` for an `i32` type: two's-complement wrapping
    /// arithmetic, which C++ leaves undefined for signed integers.
    fn wrapping_helper(&mut self, name: &str, ty: &str, op: &str) -> Result<String> {
        let unsigned = ty.replacen("int", "uint", 1);
        let key = format!("{name}:{ty}");
        let unary = name == "_wneg";
        Ok(self.helper(key, name.into(), |name| {
            if unary {
                format!(
                    "{ty} {name}({ty} a) {{\n    return as_type<{ty}>({unsigned}(0u) - as_type<{unsigned}>(a));\n}}\n"
                )
            } else {
                format!(
                    "{ty} {name}({ty} a, {ty} b) {{\n    return as_type<{ty}>(as_type<{unsigned}>(a) {op} as_type<{unsigned}>(b));\n}}\n"
                )
            }
        }))
    }

    /// Converts a scalar operand to `result`'s vector type when the other operand is a vector.
    fn widen(&mut self, ty: &Type, text: String, result: &Type) -> Result<String> {
        Ok(match (ty, result) {
            (Type::Scalar(_), Type::Vector(..)) => format!("{}({text})", self.ty(result)?),
            _ => text,
        })
    }

    fn binary(
        &mut self,
        op: BinaryOp,
        lty: &Type,
        l: String,
        rty: &Type,
        r: String,
        result: &Type,
    ) -> Result<String> {
        use BinaryOp::*;
        let (lty, rty, result) = (lty.concretize(), rty.concretize(), result.concretize());
        let matrix = matches!(lty, Type::Matrix { .. }) || matches!(rty, Type::Matrix { .. });
        let scalar = scalar_of(&lty);
        let operator = match op {
            Add => "+",
            Sub => "-",
            Mul => "*",
            Div => "/",
            Rem => "%",
            BitAnd => "&",
            BitOr => "|",
            BitXor => "^",
            Shl => "<<",
            Shr => ">>",
            Eq => "==",
            Ne => "!=",
            Lt => "<",
            Le => "<=",
            Gt => ">",
            Ge => ">=",
            And => "&&",
            Or => "||",
        };
        if matrix {
            return Ok(format!("({l} {operator} {r})"));
        }
        Ok(match op {
            Add | Sub | Mul if scalar == Scalar::I32 => {
                let ty = self.ty(&result)?;
                let name = match op {
                    Add => "_wadd",
                    Sub => "_wsub",
                    _ => "_wmul",
                };
                let helper = self.wrapping_helper(name, &ty, operator)?;
                let l = self.widen(&lty, l, &result)?;
                let r = self.widen(&rty, r, &result)?;
                format!("{helper}({l}, {r})")
            }
            Rem if scalar.is_float() => {
                let l = self.widen(&lty, l, &result)?;
                let r = self.widen(&rty, r, &result)?;
                format!("fmod({l}, {r})")
            }
            Div | Rem if scalar.is_integer() => {
                let ty = self.ty(&result)?;
                let l = self.widen(&lty, l, &result)?;
                let r = self.widen(&rty, r, &result)?;
                let name = if op == Div { "_div" } else { "_rem" };
                let signed = scalar == Scalar::I32;
                let key = format!("{name}:{ty}");
                let helper = self.helper(key, name.into(), |name| {
                    let guard = if signed {
                        format!(
                            "    {ty} d = select(b, {ty}(1), b == {ty}(0));\n    d = select(d, {ty}(1), select({ty}(0), b, a == {ty}(-2147483647 - 1)) == {ty}(-1));\n"
                        )
                    } else {
                        format!("    {ty} d = select(b, {ty}(1u), b == {ty}(0u));\n")
                    };
                    format!("{ty} {name}({ty} a, {ty} b) {{\n{guard}    return a {operator} d;\n}}\n")
                });
                format!("{helper}({l}, {r})")
            }
            Shl | Shr => {
                let masked = format!("({r} & 31u)");
                if op == Shl && scalar == Scalar::I32 {
                    let ty = self.ty(&result)?;
                    let unsigned = ty.replacen("int", "uint", 1);
                    format!("as_type<{ty}>(as_type<{unsigned}>({l}) << {masked})")
                } else {
                    format!("({l} {operator} {masked})")
                }
            }
            BitAnd | BitOr if scalar == Scalar::Bool && matches!(lty, Type::Scalar(_)) => {
                format!("bool({l} {operator} {r})")
            }
            BitAnd | BitOr if scalar == Scalar::Bool => {
                let operator = if op == BitAnd { "&&" } else { "||" };
                format!("({l} {operator} {r})")
            }
            _ => format!("({l} {operator} {r})"),
        })
    }

    // ---------------------------------------------------------------------------------------
    // Builtin functions

    fn builtin(&mut self, f: BuiltinFn, args: &[Expr], ret: &Type) -> Result<String> {
        use BuiltinFn::*;
        if f == ArrayLength {
            return self.array_length(&args[0]);
        }
        if f.is_texture() {
            return self.texture(f, args, ret);
        }
        let ret = ret.concretize();
        let mut a = Vec::new();
        for arg in args {
            let text = self.expr(arg)?;
            // MSL's component-wise functions want every argument at the result's shape.
            let text = if matches!(
                f,
                Clamp | Max | Min | Mix | Step | Smoothstep | Fma | Pow | Atan2
            ) {
                self.widen(&arg.ty.concretize(), text, &ret)?
            } else {
                text
            };
            a.push(text);
        }
        let arg_ty = args
            .first()
            .map(|x| x.ty.concretize())
            .unwrap_or(Type::Void);
        let scalar = scalar_of(&arg_ty);
        let call = |name: &str, a: &[String]| format!("{name}({})", a.join(", "));
        Ok(match f {
            Abs if scalar == Scalar::U32 => a[0].clone(),
            Abs => call("abs", &a),
            Acos => call("acos", &a),
            Acosh => call("acosh", &a),
            Asin => call("asin", &a),
            Asinh => call("asinh", &a),
            Atan => call("atan", &a),
            Atanh => call("atanh", &a),
            Atan2 => call("atan2", &a),
            Ceil => call("ceil", &a),
            Clamp => call("clamp", &a),
            Cos => call("cos", &a),
            Cosh => call("cosh", &a),
            Degrees => format!("({} * 57.295779513082322865f)", a[0]),
            Radians => format!("({} * 0.017453292519943295769f)", a[0]),
            Exp => call("exp", &a),
            Exp2 => call("exp2", &a),
            Floor => call("floor", &a),
            Fma => call("fma", &a),
            Fract => call("fract", &a),
            InverseSqrt => call("rsqrt", &a),
            Log => call("log", &a),
            Log2 => call("log2", &a),
            Max => call("max", &a),
            Min => call("min", &a),
            Mix => call("mix", &a),
            Pow => call("pow", &a),
            Round => call("rint", &a),
            Saturate => call("saturate", &a),
            Sign if scalar.is_float() => call("sign", &a),
            Sign => {
                let ty = self.ty(&arg_ty)?;
                let name = self.helper(format!("_isign:{ty}"), "_isign".into(), |name| {
                    format!(
                        "{ty} {name}({ty} x) {{\n    return select(select({ty}(0), {ty}(-1), x < {ty}(0)), {ty}(1), x > {ty}(0));\n}}\n"
                    )
                });
                call(&name, &a)
            }
            Sin => call("sin", &a),
            Sinh => call("sinh", &a),
            Smoothstep => call("smoothstep", &a),
            Sqrt => call("sqrt", &a),
            Step => call("step", &a),
            Tan => call("tan", &a),
            Tanh => call("tanh", &a),
            Trunc => call("trunc", &a),
            Cross => call("cross", &a),
            Determinant => call("determinant", &a),
            Distance => call("distance", &a),
            Dot if scalar.is_float() => call("dot", &a),
            Dot => {
                let vector = self.ty(&arg_ty)?;
                let element = self.ty(&ret)?;
                let n = arg_ty.vector_size().unwrap();
                let unsigned = vector
                    .replacen("int", "uint", 1)
                    .replacen("uuint", "uint", 1);
                let sum: Vec<String> = (0..n)
                    .map(|c| format!("p.{}", b"xyzw"[c as usize] as char))
                    .collect();
                let name = self.helper(format!("_idot:{vector}"), "_idot".into(), |name| {
                    format!(
                        "{element} {name}({vector} a, {vector} b) {{\n    {unsigned} p = as_type<{unsigned}>(a) * as_type<{unsigned}>(b);\n    return as_type<{element}>({});\n}}\n",
                        sum.join(" + ")
                    )
                });
                call(&name, &a)
            }
            FaceForward => call("faceforward", &a),
            Length => call("length", &a),
            Normalize => call("normalize", &a),
            Reflect => call("reflect", &a),
            Refract => call("refract", &a),
            Transpose => call("transpose", &a),
            All | Any if matches!(arg_ty, Type::Scalar(_)) => a[0].clone(),
            All => call("all", &a),
            Any => call("any", &a),
            Select => call("select", &a),
            CountOneBits => call("popcount", &a),
            ReverseBits => call("reverse_bits", &a),
            FirstLeadingBit | FirstTrailingBit => {
                let ty = self.ty(&arg_ty)?;
                let signed = scalar == Scalar::I32;
                let leading = f == FirstLeadingBit;
                let name = if leading {
                    "_first_leading_bit"
                } else {
                    "_first_trailing_bit"
                };
                let key = format!("{name}:{ty}");
                let name = self.helper(key, name.into(), |name| {
                    let none = if signed {
                        format!("{ty}(-1)")
                    } else {
                        format!("{ty}(0xffffffffu)")
                    };
                    let body = match (leading, signed) {
                        (true, true) => format!(
                            "    {ty} y = select(x, ~x, x < {ty}(0));\n    return select({ty}(31) - {ty}(clz(y)), {none}, y == {ty}(0));\n"
                        ),
                        (true, false) => format!(
                            "    return select({ty}(31u) - clz(x), {none}, x == {ty}(0u));\n"
                        ),
                        (false, _) => format!(
                            "    return select({ty}(ctz(x)), {none}, x == {ty}(0));\n"
                        ),
                    };
                    format!("{ty} {name}({ty} x) {{\n{body}}}\n")
                });
                call(&name, &a)
            }
            ExtractBits => call("extract_bits", &a),
            InsertBits => call("insert_bits", &a),
            Pack4x8Snorm => call("pack_float_to_snorm4x8", &a),
            Pack4x8Unorm => call("pack_float_to_unorm4x8", &a),
            Pack2x16Snorm => call("pack_float_to_snorm2x16", &a),
            Pack2x16Unorm => call("pack_float_to_unorm2x16", &a),
            Pack2x16Float => format!("as_type<uint>(half2({}))", a[0]),
            Unpack4x8Snorm => call("unpack_snorm4x8_to_float", &a),
            Unpack4x8Unorm => call("unpack_unorm4x8_to_float", &a),
            Unpack2x16Snorm => call("unpack_snorm2x16_to_float", &a),
            Unpack2x16Unorm => call("unpack_unorm2x16_to_float", &a),
            Unpack2x16Float => format!("float2(as_type<half2>({}))", a[0]),
            Dpdx => call("dfdx", &a),
            Dpdy => call("dfdy", &a),
            Fwidth => call("fwidth", &a),
            WorkgroupBarrier => "threadgroup_barrier(mem_flags::mem_threadgroup)".into(),
            StorageBarrier => "threadgroup_barrier(mem_flags::mem_device)".into(),
            ArrayLength => unreachable!(),
            _ => return unsupported(&format!("{f:?}")),
        })
    }

    fn array_length(&mut self, pointer: &Expr) -> Result<String> {
        let ExprKind::AddrOf(target) = &pointer.kind else {
            return unsupported("arrayLength of a pointer parameter");
        };
        self.runtime_length(target)
    }

    // ---------------------------------------------------------------------------------------
    // Textures

    fn texture(&mut self, f: BuiltinFn, args: &[Expr], ret: &Type) -> Result<String> {
        use BuiltinFn::*;
        // textureGather on a colour texture leads with the component, a constant.
        let skip = (f == TextureGather && matches!(args[0].ty, Type::Scalar(_))) as usize;
        let component = match args[0].as_const() {
            Some(ConstValue::Int(c)) if skip == 1 => *c as usize,
            _ => 0,
        };
        let args = &args[skip..];
        let Type::Texture { dim, arrayed, kind } = &args[0].ty else {
            unreachable!("checked texture argument")
        };
        let (dim, arrayed) = (*dim, *arrayed);
        let texture_ty = self.ty(&args[0].ty)?;
        let a: Vec<String> = args.iter().map(|x| self.expr(x)).collect::<Result<_>>()?;
        let t = a[0].clone();
        let depth = matches!(kind, TextureKind::Depth { .. });
        let multisampled = matches!(
            kind,
            TextureKind::Sampled {
                multisampled: true,
                ..
            } | TextureKind::Depth { multisampled: true }
        );
        let storage = matches!(kind, TextureKind::Storage { .. });
        let has_levels = !storage && !multisampled && dim != TextureDim::D1;
        let ret_ty = self.ty(ret)?;

        match f {
            TextureSample
            | TextureSampleBias
            | TextureSampleLevel
            | TextureSampleGrad
            | TextureSampleCompare
            | TextureSampleCompareLevel
            | TextureGather
            | TextureGatherCompare => {
                let mut parts = vec![a[1].clone(), a[2].clone()];
                let mut next = 3;
                if arrayed {
                    parts.push(format!("uint({})", a[3]));
                    next += 1;
                }
                match f {
                    TextureSampleBias => {
                        parts.push(format!("bias({})", a[next]));
                        next += 1;
                    }
                    TextureSampleLevel => {
                        // One-dimensional textures have a single level.
                        if dim != TextureDim::D1 {
                            parts.push(format!("level(float({}))", a[next]));
                        }
                        next += 1;
                    }
                    TextureSampleGrad => {
                        let gradient = match dim {
                            TextureDim::D2 => "gradient2d",
                            TextureDim::D3 => "gradient3d",
                            TextureDim::Cube => "gradientcube",
                            TextureDim::D1 => return unsupported("textureSampleGrad on 1D"),
                        };
                        parts.push(format!("{gradient}({}, {})", a[next], a[next + 1]));
                        next += 2;
                    }
                    TextureSampleCompare | TextureGatherCompare => {
                        parts.push(a[next].clone());
                        next += 1;
                    }
                    TextureSampleCompareLevel => {
                        parts.push(a[next].clone());
                        parts.push("level(0.0f)".into());
                        next += 1;
                    }
                    _ => {}
                }
                let offset = a.get(next).cloned();
                let method = match f {
                    TextureGather => {
                        if depth {
                            parts.extend(offset);
                        } else {
                            if dim != TextureDim::Cube {
                                parts.push(offset.unwrap_or_else(|| "int2(0)".into()));
                            }
                            parts.push(format!("component::{}", b"xyzw"[component.min(3)] as char));
                        }
                        "gather"
                    }
                    TextureGatherCompare => {
                        parts.extend(offset);
                        "gather_compare"
                    }
                    TextureSampleCompare | TextureSampleCompareLevel => {
                        parts.extend(offset);
                        "sample_compare"
                    }
                    _ => {
                        parts.extend(offset);
                        "sample"
                    }
                };
                Ok(format!("{t}.{method}({})", parts.join(", ")))
            }
            TextureLoad | TextureStore => {
                let store = f == TextureStore;
                let coord_ty = self.ty(&args[1].ty)?;
                let n = dim.coordinates();
                let uint_coord = if n == 1 {
                    "uint".to_string()
                } else {
                    format!("uint{n}")
                };
                let mut params = vec![format!("{texture_ty} t"), format!("{coord_ty} c")];
                let mut call = vec![t.clone(), a[1].clone()];
                let mut next = 2;
                if arrayed {
                    params.push(format!("{} a", self.ty(&args[2].ty)?));
                    call.push(a[2].clone());
                    next += 1;
                }
                let mut body = String::new();
                let mut read_tail = Vec::new();
                let level = if store {
                    None
                } else if has_levels {
                    Some("lv")
                } else {
                    None
                };
                if !store && (has_levels || multisampled) {
                    params.push(format!("{} l", self.ty(&args[next].ty)?));
                    call.push(a[next].clone());
                    if multisampled {
                        body.push_str("    uint s = min(uint(l), t.get_num_samples() - 1u);\n");
                        read_tail.push("s".to_string());
                    } else {
                        body.push_str("    uint lv = min(uint(l), t.get_num_mip_levels() - 1u);\n");
                        read_tail.push("lv".to_string());
                    }
                }
                if store {
                    params.push(format!("{} v", self.ty(&args[next].ty)?));
                    call.push(a[next].clone());
                }
                writeln!(
                    body,
                    "    {uint_coord} size = {};",
                    size_expr("t", dim, level)
                )
                .unwrap();
                writeln!(body, "    {uint_coord} p = {uint_coord}(c);").unwrap();
                let mut coords = vec!["p".to_string()];
                if arrayed {
                    body.push_str("    uint layer = uint(a);\n");
                    coords.push("layer".into());
                }
                if store {
                    let inside = if n == 1 {
                        "p < size".to_string()
                    } else {
                        "all(p < size)".to_string()
                    };
                    let inside = if arrayed {
                        format!("{inside} && layer < t.get_array_size()")
                    } else {
                        inside
                    };
                    writeln!(
                        body,
                        "    if ({inside}) {{\n        t.write(v, {});\n    }}",
                        coords.join(", ")
                    )
                    .unwrap();
                } else {
                    body.push_str("    p = min(p, size - 1u);\n");
                    if arrayed {
                        body.push_str("    layer = min(layer, t.get_array_size() - 1u);\n");
                    }
                    coords.extend(read_tail);
                    writeln!(body, "    return t.read({});", coords.join(", ")).unwrap();
                }
                let ret = if store { "void".to_string() } else { ret_ty };
                let base = if store {
                    "_texture_store"
                } else {
                    "_texture_load"
                };
                let key = format!("{base}:{ret}:{}", params.join(","));
                let name = self.numbered_helper(base, key, |name| {
                    format!("{ret} {name}({}) {{\n{body}}}\n", params.join(", "))
                });
                Ok(format!("{name}({})", call.join(", ")))
            }
            TextureDimensions => {
                if a.len() > 1 && has_levels {
                    let level_ty = self.ty(&args[1].ty)?;
                    let key = format!("_texture_dimensions:{texture_ty}:{level_ty}");
                    let size = size_expr("t", dim, Some("lv"));
                    let name = self.numbered_helper("_texture_dimensions", key, |name| {
                        format!(
                            "{ret_ty} {name}({texture_ty} t, {level_ty} l) {{\n    uint lv = min(uint(l), t.get_num_mip_levels() - 1u);\n    return {size};\n}}\n"
                        )
                    });
                    Ok(format!("{name}({t}, {})", a[1]))
                } else {
                    Ok(size_expr(&t, dim, None))
                }
            }
            TextureNumLayers => Ok(format!("{t}.get_array_size()")),
            TextureNumLevels => Ok(format!("{t}.get_num_mip_levels()")),
            TextureNumSamples => Ok(format!("{t}.get_num_samples()")),
            _ => unreachable!(),
        }
    }

    // ---------------------------------------------------------------------------------------
    // Entry points

    fn input(
        &mut self,
        io: IoBinding,
        ty: &Type,
        stage: Stage,
        interface: &mut Interface,
    ) -> Result<String> {
        match io {
            IoBinding::Builtin(b) => Ok(interface.builtin(b)),
            IoBinding::Location {
                location,
                interpolation,
            } => {
                let ty_name = self.ty(ty)?;
                let attribute = match stage {
                    Stage::Vertex => format!(" [[attribute({location})]]"),
                    _ => format!(
                        " [[user(loc{location}){}]]",
                        interpolation_attribute(ty, interpolation)
                    ),
                };
                interface
                    .inputs
                    .push(format!("    {ty_name} _loc{location}{attribute};"));
                Ok(format!("_in._loc{location}"))
            }
        }
    }

    fn output_member(
        &mut self,
        io: IoBinding,
        ty: &Type,
        stage: Stage,
    ) -> Result<(String, String)> {
        let ty_name = self.ty(ty)?;
        Ok(match io {
            IoBinding::Builtin(Builtin::Position) => (
                "_position".into(),
                "    float4 _position [[position]];".into(),
            ),
            IoBinding::Builtin(Builtin::FragDepth) => (
                "_frag_depth".into(),
                "    float _frag_depth [[depth(any)]];".into(),
            ),
            IoBinding::Builtin(Builtin::SampleMask) => (
                "_sample_mask".into(),
                "    uint _sample_mask [[sample_mask]];".into(),
            ),
            IoBinding::Builtin(b) => return unsupported(&format!("{b:?} as an output")),
            IoBinding::Location { location, .. } => {
                let attribute = if stage == Stage::Fragment {
                    format!("color({location})")
                } else {
                    format!("user(loc{location})")
                };
                (
                    format!("_loc{location}"),
                    format!("    {ty_name} _loc{location} [[{attribute}]];"),
                )
            }
        })
    }

    fn entry_point(&mut self, entry: &EntryPoint) -> Result<MslEntryPoint> {
        let module = self.module;
        let function = &module.functions[entry.function];
        let name = &entry.name;
        let msl_name = format!("ep_{name}");
        let mut interface = Interface::default();
        let mut prologue = String::new();
        let mut args = Vec::new();

        for (p, param) in function.params.iter().enumerate() {
            match (param.io, &param.ty) {
                (Some(io), ty) => args.push(self.input(io, ty, entry.stage, &mut interface)?),
                (None, Type::Struct(i)) => {
                    let st = &module.structs[*i];
                    let ty = self.ty(&param.ty)?;
                    let temp = format!("_p{p}");
                    writeln!(prologue, "    {ty} {temp} = {{}};").unwrap();
                    for member in &st.members {
                        let value =
                            self.input(member_io(member), &member.ty, entry.stage, &mut interface)?;
                        writeln!(prologue, "    {temp}.{} = {value};", ident(&member.name))
                            .unwrap();
                    }
                    args.push(temp);
                }
                _ => unreachable!("checked entry-point parameter"),
            }
        }

        // Resources, in binding order, then the sizes buffer.
        let mut params = Vec::new();
        let mut resources = Vec::new();
        let (mut buffers, mut textures, mut samplers) = (0, 0, 0);
        let mut bound: Vec<usize> = entry
            .globals
            .iter()
            .copied()
            .filter(|&g| module.globals[g].group_binding.is_some())
            .collect();
        bound.sort_by_key(|&g| module.globals[g].group_binding);
        for g in bound {
            let global = &module.globals[g];
            let (group, binding) = global.group_binding.unwrap();
            let declaration = self.global_param(g)?;
            let (slot, attribute) = match (&global.space, &global.ty) {
                (AddressSpace::Handle, Type::Sampler { .. }) => {
                    samplers += 1;
                    (
                        MslSlot::Sampler(samplers - 1),
                        format!("sampler({})", samplers - 1),
                    )
                }
                (AddressSpace::Handle, _) => {
                    textures += 1;
                    (
                        MslSlot::Texture(textures - 1),
                        format!("texture({})", textures - 1),
                    )
                }
                _ => {
                    buffers += 1;
                    (
                        MslSlot::Buffer(buffers - 1),
                        format!("buffer({})", buffers - 1),
                    )
                }
            };
            params.push(format!("{declaration} [[{attribute}]]"));
            resources.push(MslResource {
                group,
                binding,
                slot,
            });
        }
        let sizes_buffer = if self.needs_sizes(&entry.globals) {
            params.push(format!(
                "constant uint* _buffer_sizes [[buffer({buffers})]]"
            ));
            buffers += 1;
            Some(buffers - 1)
        } else {
            None
        };

        // Private and workgroup globals live in the entry point.
        let mut workgroup = Vec::new();
        for &g in &entry.globals {
            let global = &module.globals[g];
            let ty = self.ty(&global.ty)?;
            let global_name = ident(&global.name);
            match global.space {
                AddressSpace::Private => {
                    let init = match &global.init {
                        Some(e) => self.expr(e)?,
                        None => format!("{ty}{{}}"),
                    };
                    writeln!(prologue, "    {ty} {global_name} = {init};").unwrap();
                }
                AddressSpace::Workgroup => {
                    writeln!(prologue, "    threadgroup {ty} {global_name};").unwrap();
                    workgroup.push((global_name, ty));
                }
                _ => {}
            }
        }
        if !workgroup.is_empty() {
            // WGSL zero-initialises workgroup memory.
            let index = interface.builtin(Builtin::LocalInvocationIndex);
            writeln!(prologue, "    if ({index} == 0u) {{").unwrap();
            for (global_name, ty) in &workgroup {
                writeln!(prologue, "        {global_name} = {ty}{{}};").unwrap();
            }
            prologue.push_str("    }\n    threadgroup_barrier(mem_flags::mem_threadgroup);\n");
        }

        let globals = self.fn_globals[entry.function].clone();
        args.extend(globals.iter().map(|&g| ident(&module.globals[g].name)));
        if self.needs_sizes(&globals) {
            args.push("_buffer_sizes".into());
        }
        let call = format!("{}({})", ident(&function.name), args.join(", "));

        // Outputs.
        let stage = match entry.stage {
            Stage::Vertex => "vertex",
            Stage::Fragment => "fragment",
            Stage::Compute => "kernel",
        };
        let out_struct = format!("_{name}_out");
        let mut out_members = Vec::new();
        let mut body = prologue;
        let ret = match (function.ret_io, &function.ret) {
            (_, Type::Void) => {
                writeln!(body, "    {call};").unwrap();
                "void".to_string()
            }
            (Some(io), ty) => {
                let (member, declaration) = self.output_member(io, ty, entry.stage)?;
                out_members.push(declaration);
                writeln!(
                    body,
                    "    {out_struct} _o = {{}};\n    _o.{member} = {call};"
                )
                .unwrap();
                out_struct.clone()
            }
            (None, Type::Struct(i)) => {
                let ty = self.ty(&function.ret)?;
                writeln!(body, "    {ty} _r = {call};\n    {out_struct} _o = {{}};").unwrap();
                for member in &module.structs[*i].members {
                    let (out, declaration) =
                        self.output_member(member_io(member), &member.ty, entry.stage)?;
                    out_members.push(declaration);
                    writeln!(body, "    _o.{out} = _r.{};", ident(&member.name)).unwrap();
                }
                out_struct.clone()
            }
            _ => unreachable!("checked entry-point return"),
        };
        if entry.stage == Stage::Vertex && self.options.point_size {
            out_members.push("    float _point_size [[point_size]];".into());
            body.push_str("    _o._point_size = 1.0f;\n");
        }
        if ret != "void" {
            body.push_str("    return _o;\n");
            writeln!(
                self.io_types,
                "struct {out_struct} {{\n{}\n}};\n",
                out_members.join("\n")
            )
            .unwrap();
        }

        let mut all_params = Vec::new();
        if !interface.inputs.is_empty() {
            let in_struct = format!("_{name}_in");
            writeln!(
                self.io_types,
                "struct {in_struct} {{\n{}\n}};\n",
                interface.inputs.join("\n")
            )
            .unwrap();
            all_params.push(format!("{in_struct} _in [[stage_in]]"));
        }
        all_params.extend(interface.builtins.iter().map(|(_, _, d)| d.clone()));
        all_params.extend(params);
        writeln!(
            self.entries,
            "{stage} {ret} {msl_name}({}) {{\n{body}}}\n",
            all_params.join(", ")
        )
        .unwrap();

        Ok(MslEntryPoint {
            name: name.clone(),
            function: msl_name,
            stage: entry.stage,
            workgroup_size: entry.workgroup_size,
            resources,
            sizes_buffer,
            vertex_buffer_base: buffers,
        })
    }
}

/// A local's name: `_{name}_{index}`. Writer names never have an `_` before their digits, and
/// user identifiers end in `_`, so neither collides, and no MSL keyword starts with `_`.
fn local_name(local: &Local, i: usize) -> String {
    format!("_{}_{i}", local.name)
}

/// The size of texture `t` at level `level`, or its only size.
fn size_expr(t: &str, dim: TextureDim, level: Option<&str>) -> String {
    let l = level.unwrap_or("");
    match dim {
        TextureDim::D1 => format!("{t}.get_width()"),
        TextureDim::D2 | TextureDim::Cube => {
            format!("uint2({t}.get_width({l}), {t}.get_height({l}))")
        }
        TextureDim::D3 => {
            format!("uint3({t}.get_width({l}), {t}.get_height({l}), {t}.get_depth({l}))")
        }
    }
}

fn texture_type(dim: TextureDim, arrayed: bool, kind: &TextureKind) -> Result<String> {
    let array = if arrayed { "_array" } else { "" };
    let dim_name = match dim {
        TextureDim::D1 => "1d",
        TextureDim::D2 => "2d",
        TextureDim::D3 => "3d",
        TextureDim::Cube => "cube",
    };
    Ok(match kind {
        TextureKind::Sampled {
            sampled,
            multisampled: false,
        } => format!("texture{dim_name}{array}<{}>", scalar_name(*sampled)?),
        TextureKind::Sampled {
            sampled,
            multisampled: true,
        } => format!("texture2d_ms<{}>", scalar_name(*sampled)?),
        TextureKind::Depth {
            multisampled: false,
        } => format!("depth{dim_name}{array}<float>"),
        TextureKind::Depth { multisampled: true } => "depth2d_ms<float>".into(),
        TextureKind::Storage { format, access } => {
            let scalar = texel_format_scalar(format).expect("checked texel format");
            let access = match access {
                Access::Read => "read",
                Access::Write => "write",
                Access::ReadWrite => "read_write",
            };
            format!(
                "texture{dim_name}{array}<{}, access::{access}>",
                scalar_name(scalar)?
            )
        }
        TextureKind::External => return unsupported("external textures"),
    })
}
