//! Recursive-descent WGSL parser.

use crate::ast::*;
use crate::lexer::{tokenize, Token};
use crate::{Error, Span};

pub fn parse(source: &str) -> Result<Module, Error> {
    let tokens = tokenize(source)?;
    Parser { tokens, pos: 0 }.module()
}

/// Predeclared names that take a template list, so `<` after them opens one.
const TEMPLATED: &[&str] = &[
    "vec2",
    "vec3",
    "vec4",
    "mat2x2",
    "mat2x3",
    "mat2x4",
    "mat3x2",
    "mat3x3",
    "mat3x4",
    "mat4x2",
    "mat4x3",
    "mat4x4",
    "array",
    "ptr",
    "atomic",
    "bitcast",
    "binding_array",
    "texture_1d",
    "texture_2d",
    "texture_2d_array",
    "texture_3d",
    "texture_cube",
    "texture_cube_array",
    "texture_multisampled_2d",
    "texture_depth_multisampled_2d",
    "texture_storage_1d",
    "texture_storage_2d",
    "texture_storage_2d_array",
    "texture_storage_3d",
];

struct Parser {
    tokens: Vec<(Token, Span)>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        &self.tokens[self.pos].0
    }

    fn peek_at(&self, offset: usize) -> &Token {
        let i = (self.pos + offset).min(self.tokens.len() - 1);
        &self.tokens[i].0
    }

    fn span(&self) -> Span {
        self.tokens[self.pos].1
    }

    fn prev_end(&self) -> usize {
        if self.pos == 0 {
            0
        } else {
            self.tokens[self.pos - 1].1.end
        }
    }

    fn since(&self, start: usize) -> Span {
        Span::new(start, self.prev_end().max(start))
    }

    fn advance(&mut self) -> (Token, Span) {
        let token = self.tokens[self.pos].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        token
    }

    fn is_punct(&self, p: &str) -> bool {
        matches!(self.peek(), Token::Punct(q) if *q == p)
    }

    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Token::Ident(name) if name == w)
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        if self.is_punct(p) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, w: &str) -> bool {
        if self.is_word(w) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T, Error> {
        Err(Error::new(message, self.span()))
    }

    fn describe(&self) -> String {
        match self.peek() {
            Token::Ident(name) => format!("`{name}`"),
            Token::Int(..) | Token::Float(..) => "a number".into(),
            Token::Punct(p) => format!("`{p}`"),
            Token::Eof => "the end of the source".into(),
        }
    }

    fn expect_punct(&mut self, p: &str) -> Result<Span, Error> {
        if self.is_punct(p) {
            Ok(self.advance().1)
        } else {
            self.error(format!("expected `{p}`, found {}", self.describe()))
        }
    }

    /// Consumes a `>` that closes a template list, splitting `>>`, `>=` and `>>=`.
    fn expect_template_close(&mut self) -> Result<(), Error> {
        let split = match self.peek() {
            Token::Punct(">") => {
                self.advance();
                return Ok(());
            }
            Token::Punct(">>") => ">",
            Token::Punct(">=") => "=",
            Token::Punct(">>=") => ">=",
            _ => return self.error(format!("expected `>`, found {}", self.describe())),
        };
        let span = &mut self.tokens[self.pos].1;
        span.start += 1;
        self.tokens[self.pos].0 = Token::Punct(split);
        Ok(())
    }

    fn ident_name(&mut self) -> Result<(String, Span), Error> {
        match self.peek().clone() {
            Token::Ident(name) => {
                let span = self.advance().1;
                Ok((name, span))
            }
            _ => self.error(format!("expected an identifier, found {}", self.describe())),
        }
    }

    fn module(mut self) -> Result<Module, Error> {
        let mut module = Module::default();
        loop {
            if self.eat_word("enable") || self.eat_word("requires") {
                loop {
                    module.enables.push(self.ident_name()?.0);
                    if !self.eat_punct(",") || self.is_punct(";") {
                        break;
                    }
                }
                self.expect_punct(";")?;
            } else if self.is_word("diagnostic") {
                while !self.eat_punct(";") {
                    if matches!(self.peek(), Token::Eof) {
                        return self.error("unterminated diagnostic directive");
                    }
                    self.advance();
                }
            } else {
                break;
            }
        }
        while !matches!(self.peek(), Token::Eof) {
            if self.eat_punct(";") {
                continue;
            }
            module.decls.push(self.global_decl()?);
        }
        Ok(module)
    }

    fn attributes(&mut self) -> Result<Vec<Attribute>, Error> {
        let mut attrs = Vec::new();
        while self.is_punct("@") {
            let start = self.advance().1.start;
            let (name, _) = self.ident_name()?;
            let mut args = Vec::new();
            if self.eat_punct("(") {
                while !self.is_punct(")") {
                    args.push(self.expr()?);
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct(")")?;
            }
            attrs.push(Attribute {
                name,
                args,
                span: self.since(start),
            });
        }
        Ok(attrs)
    }

    fn global_decl(&mut self) -> Result<Decl, Error> {
        let attrs = self.attributes()?;
        let start = self.span().start;
        if self.eat_word("struct") {
            let (name, _) = self.ident_name()?;
            self.expect_punct("{")?;
            let mut members = Vec::new();
            while !self.is_punct("}") {
                let attrs = self.attributes()?;
                let member_start = self.span().start;
                let (member, _) = self.ident_name()?;
                self.expect_punct(":")?;
                let ty = self.type_ident()?;
                members.push(StructMember {
                    attrs,
                    name: member,
                    ty,
                    span: self.since(member_start),
                });
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct("}")?;
            self.eat_punct(";");
            return Ok(Decl::Struct(StructDecl {
                name,
                members,
                span: self.since(start),
            }));
        }
        if self.eat_word("fn") {
            return self.fn_decl(attrs, start).map(Decl::Fn);
        }
        if self.is_word("var") {
            let var = self.var_decl(attrs)?;
            self.expect_punct(";")?;
            return Ok(Decl::Var(var));
        }
        if self.eat_word("const") {
            let decl = self.value_decl(attrs, start)?;
            self.expect_punct(";")?;
            return Ok(Decl::Const(decl));
        }
        if self.eat_word("override") {
            let decl = self.value_decl(attrs, start)?;
            self.expect_punct(";")?;
            return Ok(Decl::Override(decl));
        }
        if self.eat_word("alias") {
            let (name, _) = self.ident_name()?;
            self.expect_punct("=")?;
            let ty = self.type_ident()?;
            self.expect_punct(";")?;
            return Ok(Decl::Alias {
                name,
                ty,
                span: self.since(start),
            });
        }
        if self.eat_word("const_assert") {
            let expr = self.expr()?;
            self.expect_punct(";")?;
            return Ok(Decl::ConstAssert(expr));
        }
        self.error(format!("expected a declaration, found {}", self.describe()))
    }

    fn fn_decl(&mut self, attrs: Vec<Attribute>, start: usize) -> Result<FnDecl, Error> {
        let (name, _) = self.ident_name()?;
        self.expect_punct("(")?;
        let mut params = Vec::new();
        while !self.is_punct(")") {
            let attrs = self.attributes()?;
            let param_start = self.span().start;
            let (param, _) = self.ident_name()?;
            self.expect_punct(":")?;
            let ty = self.type_ident()?;
            params.push(Param {
                attrs,
                name: param,
                ty,
                span: self.since(param_start),
            });
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(")")?;
        let ret = if self.eat_punct("->") {
            let attrs = self.attributes()?;
            Some((attrs, self.type_ident()?))
        } else {
            None
        };
        let body = self.block()?;
        Ok(FnDecl {
            attrs,
            name,
            params,
            ret,
            body,
            span: self.since(start),
        })
    }

    fn var_decl(&mut self, attrs: Vec<Attribute>) -> Result<VarDecl, Error> {
        let start = self.span().start;
        self.advance(); // `var`
        let mut space = None;
        let mut access = None;
        if self.eat_punct("<") {
            if !self.is_punct(">") {
                space = Some(self.ident_name()?.0);
                if self.eat_punct(",") {
                    access = Some(self.ident_name()?.0);
                }
            }
            self.expect_template_close()?;
        }
        let (name, _) = self.ident_name()?;
        let ty = if self.eat_punct(":") {
            Some(self.type_ident()?)
        } else {
            None
        };
        let init = if self.eat_punct("=") {
            Some(self.expr()?)
        } else {
            None
        };
        Ok(VarDecl {
            attrs,
            name,
            space,
            access,
            ty,
            init,
            span: self.since(start),
        })
    }

    fn value_decl(&mut self, attrs: Vec<Attribute>, start: usize) -> Result<ValueDecl, Error> {
        let (name, _) = self.ident_name()?;
        let ty = if self.eat_punct(":") {
            Some(self.type_ident()?)
        } else {
            None
        };
        let init = if self.eat_punct("=") {
            Some(self.expr()?)
        } else {
            None
        };
        Ok(ValueDecl {
            attrs,
            name,
            ty,
            init,
            span: self.since(start),
        })
    }

    fn type_ident(&mut self) -> Result<Ident, Error> {
        let (name, span) = self.ident_name()?;
        self.templated(name, span)
    }

    fn templated(&mut self, name: String, span: Span) -> Result<Ident, Error> {
        let mut template = Vec::new();
        if TEMPLATED.contains(&name.as_str()) && self.is_punct("<") {
            self.advance();
            while !self.is_punct(">") && !self.is_punct(">>") && !self.is_punct(">=") {
                template.push(self.template_arg()?);
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_template_close()?;
        }
        Ok(Ident {
            name,
            template,
            span: self.since(span.start),
        })
    }

    /// Template arguments stop below relational and shift operators, so `>` closes the list.
    fn template_arg(&mut self) -> Result<Expr, Error> {
        self.binary(ADDITIVE)
    }

    fn block(&mut self) -> Result<Block, Error> {
        let start = self.expect_punct("{")?.start;
        let mut stmts = Vec::new();
        while !self.is_punct("}") {
            if matches!(self.peek(), Token::Eof) {
                return self.error("unterminated block");
            }
            if self.eat_punct(";") {
                continue;
            }
            stmts.push(self.stmt()?);
        }
        self.expect_punct("}")?;
        Ok(Block {
            stmts,
            span: self.since(start),
        })
    }

    fn stmt(&mut self) -> Result<Stmt, Error> {
        // Statement attributes (such as `@diagnostic`) are accepted and ignored.
        let _ = self.attributes()?;
        let start = self.span().start;
        let kind = if self.is_punct("{") {
            StmtKind::Block(self.block()?)
        } else if self.is_word("if") {
            return self.if_stmt();
        } else if self.eat_word("switch") {
            let selector = self.expr()?;
            let _ = self.attributes()?;
            self.expect_punct("{")?;
            let mut cases = Vec::new();
            while !self.eat_punct("}") {
                let _ = self.attributes()?;
                let mut selectors = Vec::new();
                if self.eat_word("default") {
                    selectors.push(None);
                } else if self.eat_word("case") {
                    loop {
                        if self.eat_word("default") {
                            selectors.push(None);
                        } else {
                            selectors.push(Some(self.expr()?));
                        }
                        if !self.eat_punct(",") || self.is_punct(":") || self.is_punct("{") {
                            break;
                        }
                    }
                } else {
                    return self.error(format!(
                        "expected `case` or `default`, found {}",
                        self.describe()
                    ));
                }
                self.eat_punct(":");
                let body = self.block()?;
                cases.push(SwitchCase { selectors, body });
            }
            StmtKind::Switch { selector, cases }
        } else if self.eat_word("loop") {
            let _ = self.attributes()?;
            self.expect_punct("{")?;
            let block_start = self.prev_end();
            let mut stmts = Vec::new();
            let mut continuing = None;
            while !self.is_punct("}") {
                if self.eat_word("continuing") {
                    continuing = Some(self.continuing()?);
                    break;
                }
                if matches!(self.peek(), Token::Eof) {
                    return self.error("unterminated loop");
                }
                if self.eat_punct(";") {
                    continue;
                }
                stmts.push(self.stmt()?);
            }
            self.expect_punct("}")?;
            StmtKind::Loop {
                body: Block {
                    stmts,
                    span: self.since(block_start),
                },
                continuing,
            }
        } else if self.eat_word("for") {
            self.expect_punct("(")?;
            let init = if self.is_punct(";") {
                None
            } else {
                Some(Box::new(self.simple_stmt()?))
            };
            self.expect_punct(";")?;
            let cond = if self.is_punct(";") {
                None
            } else {
                Some(self.expr()?)
            };
            self.expect_punct(";")?;
            let update = if self.is_punct(")") {
                None
            } else {
                Some(Box::new(self.simple_stmt()?))
            };
            self.expect_punct(")")?;
            let body = self.block()?;
            StmtKind::For {
                init,
                cond,
                update,
                body,
            }
        } else if self.eat_word("while") {
            let cond = self.expr()?;
            let body = self.block()?;
            StmtKind::While { cond, body }
        } else {
            let kind = if self.eat_word("return") {
                if self.is_punct(";") {
                    StmtKind::Return(None)
                } else {
                    StmtKind::Return(Some(self.expr()?))
                }
            } else if self.eat_word("break") {
                if self.is_word("if") {
                    return self
                        .error("`break if` is only allowed at the end of a continuing block");
                }
                StmtKind::Break
            } else if self.eat_word("continue") {
                StmtKind::Continue
            } else if self.eat_word("discard") {
                StmtKind::Discard
            } else if self.eat_word("const_assert") {
                StmtKind::ConstAssert(self.expr()?)
            } else {
                self.simple_stmt()?.kind
            };
            self.expect_punct(";")?;
            kind
        };
        Ok(Stmt {
            kind,
            span: self.since(start),
        })
    }

    fn if_stmt(&mut self) -> Result<Stmt, Error> {
        let start = self.span().start;
        self.advance(); // `if`
        let cond = self.expr()?;
        let then = self.block()?;
        let otherwise = if self.eat_word("else") {
            if self.is_word("if") {
                Some(Box::new(self.if_stmt()?))
            } else {
                let block = self.block()?;
                let span = block.span;
                Some(Box::new(Stmt {
                    kind: StmtKind::Block(block),
                    span,
                }))
            }
        } else {
            None
        };
        Ok(Stmt {
            kind: StmtKind::If {
                cond,
                then,
                otherwise,
            },
            span: self.since(start),
        })
    }

    fn continuing(&mut self) -> Result<Continuing, Error> {
        let start = self.expect_punct("{")?.start;
        let mut stmts = Vec::new();
        let mut break_if = None;
        while !self.is_punct("}") {
            if matches!(self.peek(), Token::Eof) {
                return self.error("unterminated continuing block");
            }
            if self.is_word("break") && matches!(self.peek_at(1), Token::Ident(w) if w == "if") {
                self.advance();
                self.advance();
                break_if = Some(self.expr()?);
                self.expect_punct(";")?;
                break;
            }
            if self.eat_punct(";") {
                continue;
            }
            stmts.push(self.stmt()?);
        }
        self.expect_punct("}")?;
        Ok(Continuing {
            body: Block {
                stmts,
                span: self.since(start),
            },
            break_if,
        })
    }

    /// Declarations, assignments, increments and calls: the statements allowed in `for` headers.
    fn simple_stmt(&mut self) -> Result<Stmt, Error> {
        let start = self.span().start;
        let kind = if self.is_word("var") {
            StmtKind::Var(self.var_decl(Vec::new())?)
        } else if self.eat_word("let") {
            StmtKind::Let(self.value_decl(Vec::new(), start)?)
        } else if self.eat_word("const") {
            StmtKind::Const(self.value_decl(Vec::new(), start)?)
        } else if self.is_word("_") && matches!(self.peek_at(1), Token::Punct("=")) {
            self.advance();
            self.advance();
            StmtKind::Phony(self.expr()?)
        } else {
            let lhs = self.expr()?;
            if self.eat_punct("=") {
                StmtKind::Assign {
                    lhs,
                    op: None,
                    rhs: self.expr()?,
                }
            } else if self.eat_punct("++") {
                StmtKind::Increment(lhs)
            } else if self.eat_punct("--") {
                StmtKind::Decrement(lhs)
            } else if let Some(op) = self.compound_op() {
                self.advance();
                StmtKind::Assign {
                    lhs,
                    op: Some(op),
                    rhs: self.expr()?,
                }
            } else if matches!(lhs.kind, ExprKind::Call(..)) {
                StmtKind::Call(lhs)
            } else {
                return self.error(format!(
                    "expected an assignment or call, found {}",
                    self.describe()
                ));
            }
        };
        Ok(Stmt {
            kind,
            span: self.since(start),
        })
    }

    fn compound_op(&self) -> Option<BinaryOp> {
        Some(match self.peek() {
            Token::Punct("+=") => BinaryOp::Add,
            Token::Punct("-=") => BinaryOp::Sub,
            Token::Punct("*=") => BinaryOp::Mul,
            Token::Punct("/=") => BinaryOp::Div,
            Token::Punct("%=") => BinaryOp::Rem,
            Token::Punct("&=") => BinaryOp::BitAnd,
            Token::Punct("|=") => BinaryOp::BitOr,
            Token::Punct("^=") => BinaryOp::BitXor,
            Token::Punct("<<=") => BinaryOp::Shl,
            Token::Punct(">>=") => BinaryOp::Shr,
            _ => return None,
        })
    }

    pub fn expr(&mut self) -> Result<Expr, Error> {
        self.binary(0)
    }

    fn binary(&mut self, min_prec: u8) -> Result<Expr, Error> {
        let mut lhs = self.unary()?;
        while let Some((op, prec)) = self.binary_op() {
            if prec < min_prec {
                break;
            }
            self.advance();
            let rhs = self.binary(prec + 1)?;
            let span = Span::new(lhs.span.start, rhs.span.end);
            lhs = Expr {
                kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
                span,
            };
        }
        Ok(lhs)
    }

    fn binary_op(&self) -> Option<(BinaryOp, u8)> {
        use BinaryOp::*;
        let Token::Punct(p) = self.peek() else {
            return None;
        };
        Some(match *p {
            "||" => (Or, 1),
            "&&" => (And, 2),
            "|" => (BitOr, 3),
            "^" => (BitXor, 4),
            "&" => (BitAnd, 5),
            "==" => (Eq, 6),
            "!=" => (Ne, 6),
            "<" => (Lt, 7),
            "<=" => (Le, 7),
            ">" => (Gt, 7),
            ">=" => (Ge, 7),
            "<<" => (Shl, 8),
            ">>" => (Shr, 8),
            "+" => (Add, ADDITIVE),
            "-" => (Sub, ADDITIVE),
            "*" => (Mul, 10),
            "/" => (Div, 10),
            "%" => (Rem, 10),
            _ => return None,
        })
    }

    fn unary(&mut self) -> Result<Expr, Error> {
        let start = self.span().start;
        let op = match self.peek() {
            Token::Punct("-") => Some(UnaryOp::Neg),
            Token::Punct("!") => Some(UnaryOp::Not),
            Token::Punct("~") => Some(UnaryOp::BitNot),
            Token::Punct("&") => Some(UnaryOp::AddrOf),
            Token::Punct("*") => Some(UnaryOp::Deref),
            _ => None,
        };
        if let Some(op) = op {
            self.advance();
            let operand = self.unary()?;
            return Ok(Expr {
                kind: ExprKind::Unary(op, Box::new(operand)),
                span: self.since(start),
            });
        }
        // `&&x` and `--x` lex as one token but mean two unary operators here.
        if self.is_punct("&&") || self.is_punct("--") {
            let (inner, outer) = if self.is_punct("&&") {
                (UnaryOp::AddrOf, UnaryOp::AddrOf)
            } else {
                (UnaryOp::Neg, UnaryOp::Neg)
            };
            self.advance();
            let operand = self.unary()?;
            let span = self.since(start);
            let inner = Expr {
                kind: ExprKind::Unary(inner, Box::new(operand)),
                span,
            };
            return Ok(Expr {
                kind: ExprKind::Unary(outer, Box::new(inner)),
                span,
            });
        }
        self.postfix()
    }

    fn postfix(&mut self) -> Result<Expr, Error> {
        let start = self.span().start;
        let mut expr = self.primary()?;
        loop {
            if self.eat_punct("[") {
                let index = self.expr()?;
                self.expect_punct("]")?;
                expr = Expr {
                    kind: ExprKind::Index(Box::new(expr), Box::new(index)),
                    span: self.since(start),
                };
            } else if self.eat_punct(".") {
                let (member, member_span) = self.ident_name()?;
                expr = Expr {
                    kind: ExprKind::Member(Box::new(expr), member, member_span),
                    span: self.since(start),
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        let start = self.span().start;
        match self.peek().clone() {
            Token::Int(value, suffix) => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Int(value, suffix),
                    span: self.since(start),
                })
            }
            Token::Float(value, suffix) => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Float(value, suffix),
                    span: self.since(start),
                })
            }
            Token::Punct("(") => {
                self.advance();
                let mut inner = self.expr()?;
                self.expect_punct(")")?;
                inner.span = self.since(start);
                Ok(inner)
            }
            Token::Ident(name) if name == "true" || name == "false" => {
                self.advance();
                Ok(Expr {
                    kind: ExprKind::Bool(name == "true"),
                    span: self.since(start),
                })
            }
            Token::Ident(_) => {
                let (name, span) = self.ident_name()?;
                let ident = self.templated(name, span)?;
                if self.eat_punct("(") {
                    let mut args = Vec::new();
                    while !self.is_punct(")") {
                        args.push(self.expr()?);
                        if !self.eat_punct(",") {
                            break;
                        }
                    }
                    self.expect_punct(")")?;
                    Ok(Expr {
                        kind: ExprKind::Call(ident, args),
                        span: self.since(start),
                    })
                } else {
                    Ok(Expr {
                        kind: ExprKind::Ident(ident),
                        span: self.since(start),
                    })
                }
            }
            _ => self.error(format!("expected an expression, found {}", self.describe())),
        }
    }
}

const ADDITIVE: u8 = 9;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_templates() {
        let module = parse("var<private> a: array<vec2<f32>, 3>;").unwrap();
        let Decl::Var(var) = &module.decls[0] else {
            panic!()
        };
        let ty = var.ty.as_ref().unwrap();
        assert_eq!(ty.name, "array");
        assert_eq!(ty.template.len(), 2);
        let ExprKind::Ident(inner) = &ty.template[0].kind else {
            panic!()
        };
        assert_eq!(inner.name, "vec2");
    }

    #[test]
    fn precedence() {
        let module = parse("const x = 1 + 2 * 3 == 7 && true;").unwrap();
        let Decl::Const(decl) = &module.decls[0] else {
            panic!()
        };
        let ExprKind::Binary(BinaryOp::And, lhs, _) = &decl.init.as_ref().unwrap().kind else {
            panic!()
        };
        assert!(matches!(lhs.kind, ExprKind::Binary(BinaryOp::Eq, ..)));
    }

    #[test]
    fn statements() {
        let source = r#"
            @fragment fn main(@builtin(position) p: vec4f) -> @location(0) vec4f {
                var total = 0.0;
                for (var i = 0; i < 4; i++) { total += f32(i); }
                loop { if total > 1.0 { break; } continuing { total += 1.0; break if total > 9.0; } }
                switch i32(total) { case 1, 2: { discard; } default { } }
                while total > 0.0 { total -= 1.0; }
                _ = p;
                return vec4f(total);
            }
        "#;
        let module = parse(source).unwrap();
        let Decl::Fn(f) = &module.decls[0] else {
            panic!()
        };
        assert_eq!(f.body.stmts.len(), 7);
    }

    #[test]
    fn reports_positions() {
        let err = parse("fn f() {\n  let x = ;\n}").unwrap_err();
        assert_eq!(err.span.start, 19);
    }
}
