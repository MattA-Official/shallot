//! The checked, typed module: what the back ends consume and what reflection reports.

use crate::ast::{BinaryOp, UnaryOp};
use crate::consts::ConstValue;
use crate::types::{
    Access, AddressSpace, Builtin, Interpolation, Scalar, StructType, TextureDim, Type,
};

#[derive(Debug, Clone, Default)]
pub struct Module {
    pub enables: Vec<String>,
    pub structs: Vec<StructType>,
    pub globals: Vec<Global>,
    pub functions: Vec<Function>,
    pub entry_points: Vec<EntryPoint>,
    pub bindings: Vec<Binding>,
}

impl Module {
    pub fn entry_point(&self, name: &str) -> Option<&EntryPoint> {
        self.entry_points.iter().find(|e| e.name == name)
    }

    /// The resource bindings an entry point uses, directly or through called functions.
    pub fn bindings_for(&self, entry: &EntryPoint) -> impl Iterator<Item = &Binding> + '_ {
        let used = entry.globals.clone();
        self.bindings
            .iter()
            .filter(move |b| used.contains(&b.global))
    }
}

#[derive(Debug, Clone)]
pub struct Global {
    pub name: String,
    pub space: AddressSpace,
    pub access: Access,
    pub ty: Type,
    pub group_binding: Option<(u32, u32)>,
    pub init: Option<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    Vertex,
    Fragment,
    Compute,
}

/// Where an entry-point parameter, return value or struct member connects to the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoBinding {
    Builtin(Builtin),
    Location {
        location: u32,
        interpolation: Option<Interpolation>,
    },
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub io: Option<IoBinding>,
}

#[derive(Debug, Clone)]
pub struct Local {
    pub name: String,
    pub ty: Type,
    /// `var` locals are memory; `let` locals are values.
    pub mutable: bool,
}

#[derive(Debug, Clone)]
pub struct Function {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Type,
    pub ret_io: Option<IoBinding>,
    pub locals: Vec<Local>,
    pub body: Vec<Stmt>,
    /// Module globals referenced directly in the body.
    pub globals: Vec<usize>,
    /// Functions called directly in the body.
    pub calls: Vec<usize>,
}

/// An entry point's interface variable, for reflection.
#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceVar {
    pub io: IoBinding,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct EntryPoint {
    pub name: String,
    pub stage: Stage,
    pub function: usize,
    pub workgroup_size: [u32; 3],
    pub inputs: Vec<InterfaceVar>,
    pub outputs: Vec<InterfaceVar>,
    /// Every global used, through calls included.
    pub globals: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResourceKind {
    UniformBuffer {
        min_size: u64,
    },
    StorageBuffer {
        read_only: bool,
        min_size: u64,
    },
    Sampler {
        comparison: bool,
    },
    Texture {
        dim: TextureDim,
        arrayed: bool,
        sample: TextureSample,
        multisampled: bool,
    },
    StorageTexture {
        dim: TextureDim,
        arrayed: bool,
        format: String,
        access: Access,
    },
    ExternalTexture,
}

/// The texel type a sampled texture binding returns, as in WebGPU's `GPUTextureSampleType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureSample {
    Float,
    Depth,
    Sint,
    Uint,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub group: u32,
    pub binding: u32,
    pub name: String,
    pub global: usize,
    pub kind: ResourceKind,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Type,
    /// `Some` when the expression denotes memory (a WGSL reference) holding a `ty`.
    pub reference: Option<(AddressSpace, Access)>,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Const(ConstValue),
    /// A `var` local (a reference) or a `let` local (a value).
    Local(usize),
    Param(usize),
    Global(usize),
    Load(Box<Expr>),
    Unary(UnaryOp, Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    /// Builds `ty` from components: scalars and vectors for a vector, columns for a matrix,
    /// elements for an array, members for a struct. Empty means the zero value.
    Construct(Vec<Expr>),
    /// Repeats a scalar into every component of the vector `ty`.
    Splat(Box<Expr>),
    /// Value conversion between scalar, vector or matrix types of the same shape.
    Convert(Box<Expr>),
    Bitcast(Box<Expr>),
    Call(usize, Vec<Expr>),
    Builtin(BuiltinFn, Vec<Expr>),
    /// A struct member, of a reference or a value.
    Member(Box<Expr>, u32),
    /// Vector components of a value.
    Swizzle(Box<Expr>, Vec<u8>),
    /// An array, matrix or vector element, of a reference or a value.
    Index(Box<Expr>, Box<Expr>),
    AddrOf(Box<Expr>),
    Deref(Box<Expr>),
}

impl Expr {
    pub fn constant(value: ConstValue, ty: Type) -> Expr {
        Expr {
            kind: ExprKind::Const(value),
            ty,
            reference: None,
        }
    }

    pub fn as_const(&self) -> Option<&ConstValue> {
        match &self.kind {
            ExprKind::Const(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Stmt {
    /// Binds a `let` local.
    Let(usize, Expr),
    /// Initialises a `var` local (to zero when `None`).
    Var(usize, Option<Expr>),
    /// Writes `value` to the reference `target`.
    Store {
        target: Expr,
        value: Expr,
    },
    /// `target = target op value`, evaluating `target` once.
    Update {
        target: Expr,
        op: BinaryOp,
        value: Expr,
    },
    Eval(Expr),
    Return(Option<Expr>),
    If {
        cond: Expr,
        then: Vec<Stmt>,
        otherwise: Vec<Stmt>,
    },
    Switch {
        selector: Expr,
        cases: Vec<SwitchCase>,
    },
    /// `for` and `while` lower to this with an `if !cond { break; }` at the top of `body`.
    Loop {
        body: Vec<Stmt>,
        continuing: Vec<Stmt>,
        break_if: Option<Expr>,
    },
    Break,
    Continue,
    Discard,
    Block(Vec<Stmt>),
}

#[derive(Debug, Clone)]
pub struct SwitchCase {
    /// `None` is `default`.
    pub selectors: Vec<Option<i64>>,
    pub body: Vec<Stmt>,
}

/// Builtin functions the checker resolved, with argument types already unified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuiltinFn {
    // Component-wise float functions.
    Abs,
    Acos,
    Acosh,
    Asin,
    Asinh,
    Atan,
    Atanh,
    Atan2,
    Ceil,
    Clamp,
    Cos,
    Cosh,
    Degrees,
    Exp,
    Exp2,
    Floor,
    Fma,
    Fract,
    InverseSqrt,
    Log,
    Log2,
    Max,
    Min,
    Mix,
    Pow,
    Radians,
    Round,
    Saturate,
    Sign,
    Sin,
    Sinh,
    Smoothstep,
    Sqrt,
    Step,
    Tan,
    Tanh,
    Trunc,
    // Geometric and matrix functions.
    Cross,
    Determinant,
    Distance,
    Dot,
    FaceForward,
    Length,
    Normalize,
    Reflect,
    Refract,
    Transpose,
    // Logical.
    All,
    Any,
    Select,
    // Bits.
    CountOneBits,
    ReverseBits,
    FirstLeadingBit,
    FirstTrailingBit,
    ExtractBits,
    InsertBits,
    // Packing.
    Pack4x8Snorm,
    Pack4x8Unorm,
    Pack2x16Snorm,
    Pack2x16Unorm,
    Pack2x16Float,
    Unpack4x8Snorm,
    Unpack4x8Unorm,
    Unpack2x16Snorm,
    Unpack2x16Unorm,
    Unpack2x16Float,
    // Derivatives.
    Dpdx,
    Dpdy,
    Fwidth,
    // Synchronisation.
    WorkgroupBarrier,
    StorageBarrier,
    // Arrays.
    ArrayLength,
}

impl BuiltinFn {
    pub fn from_name(name: &str) -> Option<BuiltinFn> {
        use BuiltinFn::*;
        Some(match name {
            "abs" => Abs,
            "acos" => Acos,
            "acosh" => Acosh,
            "asin" => Asin,
            "asinh" => Asinh,
            "atan" => Atan,
            "atanh" => Atanh,
            "atan2" => Atan2,
            "ceil" => Ceil,
            "clamp" => Clamp,
            "cos" => Cos,
            "cosh" => Cosh,
            "degrees" => Degrees,
            "exp" => Exp,
            "exp2" => Exp2,
            "floor" => Floor,
            "fma" => Fma,
            "fract" => Fract,
            "inverseSqrt" => InverseSqrt,
            "log" => Log,
            "log2" => Log2,
            "max" => Max,
            "min" => Min,
            "mix" => Mix,
            "pow" => Pow,
            "radians" => Radians,
            "round" => Round,
            "saturate" => Saturate,
            "sign" => Sign,
            "sin" => Sin,
            "sinh" => Sinh,
            "smoothstep" => Smoothstep,
            "sqrt" => Sqrt,
            "step" => Step,
            "tan" => Tan,
            "tanh" => Tanh,
            "trunc" => Trunc,
            "cross" => Cross,
            "determinant" => Determinant,
            "distance" => Distance,
            "dot" => Dot,
            "faceForward" => FaceForward,
            "length" => Length,
            "normalize" => Normalize,
            "reflect" => Reflect,
            "refract" => Refract,
            "transpose" => Transpose,
            "all" => All,
            "any" => Any,
            "select" => Select,
            "countOneBits" => CountOneBits,
            "reverseBits" => ReverseBits,
            "firstLeadingBit" => FirstLeadingBit,
            "firstTrailingBit" => FirstTrailingBit,
            "extractBits" => ExtractBits,
            "insertBits" => InsertBits,
            "pack4x8snorm" => Pack4x8Snorm,
            "pack4x8unorm" => Pack4x8Unorm,
            "pack2x16snorm" => Pack2x16Snorm,
            "pack2x16unorm" => Pack2x16Unorm,
            "pack2x16float" => Pack2x16Float,
            "unpack4x8snorm" => Unpack4x8Snorm,
            "unpack4x8unorm" => Unpack4x8Unorm,
            "unpack2x16snorm" => Unpack2x16Snorm,
            "unpack2x16unorm" => Unpack2x16Unorm,
            "unpack2x16float" => Unpack2x16Float,
            "dpdx" => Dpdx,
            "dpdy" => Dpdy,
            "fwidth" => Fwidth,
            "workgroupBarrier" => WorkgroupBarrier,
            "storageBarrier" => StorageBarrier,
            "arrayLength" => ArrayLength,
            _ => return None,
        })
    }
}

/// Helper for code that matches on the leaf scalar of an expression's type.
pub fn scalar_of(ty: &Type) -> Scalar {
    ty.leaf_scalar().unwrap_or(Scalar::Bool)
}
