//! The untyped syntax tree produced by the parser.

use crate::lexer::{FloatSuffix, IntSuffix};
use crate::Span;

#[derive(Debug, Clone, Default)]
pub struct Module {
    /// `enable` and `requires` directive names.
    pub enables: Vec<String>,
    pub decls: Vec<Decl>,
}

#[derive(Debug, Clone)]
pub enum Decl {
    Struct(StructDecl),
    Fn(FnDecl),
    Var(VarDecl),
    Const(ValueDecl),
    Override(ValueDecl),
    Alias { name: String, ty: Ident, span: Span },
    ConstAssert(Expr),
}

#[derive(Debug, Clone)]
pub struct Attribute {
    pub name: String,
    pub args: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub members: Vec<StructMember>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StructMember {
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub ty: Ident,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Option<(Vec<Attribute>, Ident)>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub ty: Ident,
    pub span: Span,
}

/// `var<space, access> name: T = init;`, at module or function scope.
#[derive(Debug, Clone)]
pub struct VarDecl {
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub space: Option<String>,
    pub access: Option<String>,
    pub ty: Option<Ident>,
    pub init: Option<Expr>,
    pub span: Span,
}

/// `const`, `override` and `let` declarations.
#[derive(Debug, Clone)]
pub struct ValueDecl {
    pub attrs: Vec<Attribute>,
    pub name: String,
    pub ty: Option<Ident>,
    pub init: Option<Expr>,
    pub span: Span,
}

/// A possibly templated identifier, such as `x`, `vec3<f32>` or `array<u32, 4>`.
/// Types are written this way too.
#[derive(Debug, Clone)]
pub struct Ident {
    pub name: String,
    pub template: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Ident(Ident),
    Bool(bool),
    Int(i64, IntSuffix),
    Float(f64, FloatSuffix),
    Unary(UnaryOp, Box<Expr>),
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    Call(Ident, Vec<Expr>),
    Index(Box<Expr>, Box<Expr>),
    Member(Box<Expr>, String, Span),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
    AddrOf,
    Deref,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    And,
    Or,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl BinaryOp {
    pub fn symbol(self) -> &'static str {
        use BinaryOp::*;
        match self {
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
            And => "&&",
            Or => "||",
            Eq => "==",
            Ne => "!=",
            Lt => "<",
            Le => "<=",
            Gt => ">",
            Ge => ">=",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum StmtKind {
    Block(Block),
    Var(VarDecl),
    Let(ValueDecl),
    Const(ValueDecl),
    /// `lhs = rhs` or a compound assignment such as `lhs += rhs`.
    Assign {
        lhs: Expr,
        op: Option<BinaryOp>,
        rhs: Expr,
    },
    /// `_ = expr;`
    Phony(Expr),
    Increment(Expr),
    Decrement(Expr),
    Call(Expr),
    Return(Option<Expr>),
    If {
        cond: Expr,
        then: Block,
        otherwise: Option<Box<Stmt>>,
    },
    Switch {
        selector: Expr,
        cases: Vec<SwitchCase>,
    },
    Loop {
        body: Block,
        continuing: Option<Continuing>,
    },
    For {
        init: Option<Box<Stmt>>,
        cond: Option<Expr>,
        update: Option<Box<Stmt>>,
        body: Block,
    },
    While {
        cond: Expr,
        body: Block,
    },
    Break,
    Continue,
    Discard,
    ConstAssert(Expr),
}

#[derive(Debug, Clone)]
pub struct SwitchCase {
    /// `None` is the `default` selector.
    pub selectors: Vec<Option<Expr>>,
    pub body: Block,
}

#[derive(Debug, Clone)]
pub struct Continuing {
    pub body: Block,
    pub break_if: Option<Expr>,
}
