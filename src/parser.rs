use crate::ast::{
    AssignTarget, BinaryOp, CallNamedExpr, ChainedCmpExpr, Expr, FnParam, ImportItem, ImportSource,
    MatchArm, Pattern, Program, Stmt, StructField, StructInitExpr, StructMethod, UnaryOp,
};
use crate::lexer::{Span, Token, TokenKind};

const MAX_PARSE_DEPTH: usize = 64;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    depth: usize,
    allow_struct_literal: bool,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self {
            tokens,
            pos: 0,
            depth: 0,
            allow_struct_literal: true,
        }
    }

    pub fn parse_program(&mut self) -> Result<Program, String> {
        let (stmts, tail) = self.parse_stmt_list(TokenKind::Eof)?;
        Ok(Program { stmts, tail })
    }

    fn parse_stmt_list(
        &mut self,
        terminator: TokenKind,
    ) -> Result<(Vec<Stmt>, Option<Box<Expr>>), String> {
        let mut stmts = Vec::new();
        let mut tail = None;

        while !self.check(&terminator) && !self.is_eof() {
            if self.check(&TokenKind::Semicolon) {
                self.advance();
                continue;
            }

            if self.is_stmt_start() {
                stmts.push(self.parse_stmt()?);
                continue;
            }

            let expr = self.parse_expr()?;
            if self.match_kind(&TokenKind::Semicolon) {
                stmts.push(Stmt::Expr(expr));
            } else if self.check(&terminator) || self.is_eof() {
                tail = Some(Box::new(expr));
                break;
            } else if is_block_expr(&expr) {
                stmts.push(Stmt::Expr(expr));
            } else {
                let span = self.current_span();
                return Err(format!(
                    "[{}:{}] Expected ';' or '{}' after expression",
                    span.line,
                    span.col,
                    if terminator == TokenKind::Eof {
                        "EOF"
                    } else {
                        "}"
                    }
                ));
            }
        }

        Ok((stmts, tail))
    }

    fn is_stmt_start(&self) -> bool {
        match self.peek_kind() {
            TokenKind::Let
            | TokenKind::Struct
            | TokenKind::Import
            | TokenKind::From
            | TokenKind::Pub
            | TokenKind::While
            | TokenKind::Loop
            | TokenKind::For
            | TokenKind::Return
            | TokenKind::Break
            | TokenKind::Continue => true,
            TokenKind::Fn => matches!(self.peek_next_kind(), Some(TokenKind::Ident(_))),
            _ => false,
        }
    }

    fn parse_stmt(&mut self) -> Result<Stmt, String> {
        let span = self.current_span();
        match self.peek_kind().clone() {
            TokenKind::Import => {
                self.advance();
                let (source, default_alias) = self.parse_import_source(span, "import")?;
                let alias = if self.match_kind(&TokenKind::As) {
                    self.expect_ident("Expected alias name after 'as'")?
                } else {
                    default_alias
                };
                self.expect(
                    &TokenKind::Semicolon,
                    "Expected ';' after 'import' statement",
                )?;
                Ok(Stmt::Import {
                    source,
                    alias,
                    span,
                })
            }
            TokenKind::From => {
                self.advance();
                let (source, _) = self.parse_import_source(span, "from")?;
                self.expect(
                    &TokenKind::Import,
                    "Expected 'import' after module source in 'from' statement",
                )?;
                let mut items = Vec::new();
                loop {
                    let name = self.expect_ident("Expected exported symbol name after 'import'")?;
                    let alias = if self.match_kind(&TokenKind::As) {
                        self.expect_ident("Expected alias name after 'as'")?
                    } else {
                        name.clone()
                    };
                    items.push(ImportItem { name, alias });
                    if !self.match_kind(&TokenKind::Comma) {
                        break;
                    }
                    if self.check(&TokenKind::Semicolon) {
                        break;
                    }
                }
                self.expect(
                    &TokenKind::Semicolon,
                    "Expected ';' after 'from ... import' statement",
                )?;
                Ok(Stmt::ImportFrom {
                    source,
                    items,
                    span,
                })
            }
            TokenKind::Pub => {
                self.advance();
                match self.peek_kind() {
                    TokenKind::Let => {
                        self.advance();
                        self.parse_let_stmt(span, true)
                    }
                    TokenKind::Fn => {
                        self.advance();
                        self.parse_fn_decl(span, true)
                    }
                    TokenKind::Struct => {
                        self.advance();
                        self.parse_struct_decl(span, true)
                    }
                    _ => Err(format!(
                        "[{}:{}] Expected 'let', 'fn', or 'struct' after 'pub'",
                        span.line, span.col
                    )),
                }
            }
            TokenKind::Let => {
                self.advance();
                self.parse_let_stmt(span, false)
            }
            TokenKind::Fn => {
                self.advance();
                self.parse_fn_decl(span, false)
            }
            TokenKind::Struct => {
                self.advance();
                self.parse_struct_decl(span, false)
            }
            TokenKind::While => {
                self.advance();
                let cond = self.parse_expr_no_struct()?;
                let body = self.parse_block()?;
                Ok(Stmt::While { cond, body, span })
            }
            TokenKind::Loop => {
                self.advance();
                let body = self.parse_block()?;
                Ok(Stmt::While {
                    cond: Expr::Bool(true, span),
                    body,
                    span,
                })
            }
            TokenKind::For => {
                self.advance();
                let var1 = self.expect_ident("Expected loop variable name after 'for'")?;
                if self.match_kind(&TokenKind::Comma) {
                    let var2 = self.expect_ident("Expected second loop variable name")?;
                    self.expect(&TokenKind::In, "Expected 'in' after loop variables")?;
                    let iter = self.parse_expr_no_struct()?;
                    let body = self.parse_block()?;
                    Ok(Stmt::ForPair {
                        var1,
                        var2,
                        iter,
                        body,
                        span,
                    })
                } else {
                    self.expect(&TokenKind::In, "Expected 'in' after loop variable")?;
                    let iter = self.parse_expr_no_struct()?;
                    let body = self.parse_block()?;
                    Ok(Stmt::For {
                        var: var1,
                        iter,
                        body,
                        span,
                    })
                }
            }
            TokenKind::Return => {
                self.advance();
                let value = if self.check(&TokenKind::Semicolon) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                self.expect(&TokenKind::Semicolon, "Expected ';' after 'return'")?;
                Ok(Stmt::Return { value, span })
            }
            TokenKind::Break => {
                self.advance();
                self.expect(&TokenKind::Semicolon, "Expected ';' after 'break'")?;
                Ok(Stmt::Break(span))
            }
            TokenKind::Continue => {
                self.advance();
                self.expect(&TokenKind::Semicolon, "Expected ';' after 'continue'")?;
                Ok(Stmt::Continue(span))
            }
            _ => unreachable!(),
        }
    }

    fn parse_expr_no_struct(&mut self) -> Result<Expr, String> {
        let prev = self.allow_struct_literal;
        self.allow_struct_literal = false;
        let res = self.parse_expr();
        self.allow_struct_literal = prev;
        res
    }

    fn parse_expr(&mut self) -> Result<Expr, String> {
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            let span = self.current_span();
            return Err(format!(
                "[{}:{}] Maximum expression nesting depth ({}) exceeded",
                span.line, span.col, MAX_PARSE_DEPTH
            ));
        }
        let res = self.parse_assign();
        self.depth -= 1;
        res
    }

    fn parse_assign(&mut self) -> Result<Expr, String> {
        let expr = self.parse_coalesce()?;
        let compound_op = if self.match_kind(&TokenKind::PlusAssign) {
            Some(BinaryOp::Add)
        } else if self.match_kind(&TokenKind::MinusAssign) {
            Some(BinaryOp::Sub)
        } else if self.match_kind(&TokenKind::StarAssign) {
            Some(BinaryOp::Mul)
        } else if self.match_kind(&TokenKind::SlashAssign) {
            Some(BinaryOp::Div)
        } else {
            None
        };
        if compound_op.is_some() || self.match_kind(&TokenKind::Assign) {
            let span = expr.span();
            let rhs = self.parse_expr()?;
            let target = match &expr {
                Expr::Ident(name, _) => AssignTarget::Var(name.clone()),
                Expr::Index { object, index, .. } => AssignTarget::Index {
                    object: object.clone(),
                    index: index.clone(),
                },
                Expr::GetAttr { object, name, .. } => AssignTarget::Attr {
                    object: object.clone(),
                    name: name.clone(),
                },
                _ => {
                    return Err(format!(
                        "[{}:{}] Invalid assignment target",
                        span.line, span.col
                    ));
                }
            };
            return Ok(Expr::Assign {
                target: Box::new(target),
                op: compound_op,
                value: Box::new(rhs),
                span,
            });
        }
        Ok(expr)
    }

    fn parse_coalesce(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        let mut expr = self.parse_or()?;
        while self.match_kind(&TokenKind::DoubleQuestion) {
            self.check_chain(start)?;
            let span = expr.span();
            let right = self.parse_or()?;
            expr = Expr::NullCoalesce {
                left: Box::new(expr),
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_or(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        let mut expr = self.parse_and()?;
        while self.match_kind(&TokenKind::Or) {
            self.check_chain(start)?;
            let span = expr.span();
            let right = self.parse_and()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op: BinaryOp::Or,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_and(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        let mut expr = self.parse_equality()?;
        while self.match_kind(&TokenKind::And) {
            self.check_chain(start)?;
            let span = expr.span();
            let right = self.parse_equality()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op: BinaryOp::And,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_equality(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        let mut expr = self.parse_comparison()?;
        loop {
            self.check_chain(start)?;
            let op = if self.match_kind(&TokenKind::Eq) {
                BinaryOp::Eq
            } else if self.match_kind(&TokenKind::Ne) {
                BinaryOp::Ne
            } else {
                break;
            };
            let span = expr.span();
            let right = self.parse_comparison()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_comparison(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        let first = self.parse_pipe()?;
        let mut comparisons = Vec::new();
        loop {
            self.check_chain(start)?;
            let op = if self.match_kind(&TokenKind::Lt) {
                BinaryOp::Lt
            } else if self.match_kind(&TokenKind::Le) {
                BinaryOp::Le
            } else if self.match_kind(&TokenKind::Gt) {
                BinaryOp::Gt
            } else if self.match_kind(&TokenKind::Ge) {
                BinaryOp::Ge
            } else if self.match_kind(&TokenKind::In) {
                BinaryOp::In
            } else if matches!(self.peek_kind(), TokenKind::Ident(s) if s == "not")
                && matches!(self.peek_next_kind(), Some(TokenKind::In))
            {
                self.advance();
                self.advance();
                BinaryOp::NotIn
            } else {
                break;
            };
            let right = self.parse_pipe()?;
            comparisons.push((op, right));
        }
        if comparisons.is_empty() {
            Ok(first)
        } else if comparisons.len() == 1 {
            let span = first.span();
            let (op, right) = comparisons.into_iter().next().unwrap();
            Ok(Expr::Binary {
                left: Box::new(first),
                op,
                right: Box::new(right),
                span,
            })
        } else {
            let span = first.span();
            Ok(Expr::ChainedCmp(Box::new(ChainedCmpExpr {
                first: Box::new(first),
                comparisons,
                span,
            })))
        }
    }

    fn parse_pipe(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        let mut expr = self.parse_term()?;
        while self.match_kind(&TokenKind::Pipe) {
            self.check_chain(start)?;
            let right = self.parse_term()?;
            let span = right.span();
            expr = match right {
                Expr::Call {
                    callee,
                    mut args,
                    span: call_span,
                } => {
                    args.insert(0, expr);
                    Expr::Call {
                        callee,
                        args,
                        span: call_span,
                    }
                }
                Expr::CallNamed(mut c) => {
                    c.pos_args.insert(0, expr);
                    Expr::CallNamed(c)
                }
                other => Expr::Call {
                    callee: Box::new(other),
                    args: vec![expr],
                    span,
                },
            };
        }
        Ok(expr)
    }

    fn parse_term(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        let mut expr = self.parse_factor()?;
        loop {
            self.check_chain(start)?;
            let op = if self.match_kind(&TokenKind::Plus) {
                BinaryOp::Add
            } else if self.match_kind(&TokenKind::Minus) {
                BinaryOp::Sub
            } else {
                break;
            };
            let span = expr.span();
            let right = self.parse_factor()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_factor(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        let mut expr = self.parse_unary()?;
        loop {
            self.check_chain(start)?;
            let op = if self.match_kind(&TokenKind::Star) {
                BinaryOp::Mul
            } else if self.match_kind(&TokenKind::Slash) {
                BinaryOp::Div
            } else if self.match_kind(&TokenKind::Percent) {
                BinaryOp::Mod
            } else {
                break;
            };
            let span = expr.span();
            let right = self.parse_unary()?;
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        let span = self.current_span();
        if self.match_kind(&TokenKind::Minus) {
            let expr = self.parse_nested_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Neg,
                expr: Box::new(expr),
                span,
            });
        }
        if self.match_kind(&TokenKind::Bang) {
            let expr = self.parse_nested_unary()?;
            return Ok(Expr::Unary {
                op: UnaryOp::Not,
                expr: Box::new(expr),
                span,
            });
        }
        if self.match_kind(&TokenKind::Throw) {
            if self.depth >= MAX_PARSE_DEPTH {
                return Err("Maximum expression nesting depth exceeded".to_string());
            }
            self.depth += 1;
            let value = self.parse_expr();
            self.depth -= 1;
            return Ok(Expr::Throw {
                value: Box::new(value?),
                span,
            });
        }
        self.parse_postfix()
    }

    fn parse_nested_unary(&mut self) -> Result<Expr, String> {
        if self.depth >= MAX_PARSE_DEPTH {
            return Err("Maximum expression nesting depth exceeded".to_string());
        }
        self.depth += 1;
        let result = self.parse_unary();
        self.depth -= 1;
        result
    }

    fn looks_like_struct_literal(&self) -> bool {
        if !matches!(self.peek_kind(), TokenKind::LBrace) {
            return false;
        }
        match self.tokens.get(self.pos + 1).map(|t| &t.kind) {
            Some(TokenKind::RBrace) | Some(TokenKind::DotDot) => true,
            Some(TokenKind::Ident(_)) => matches!(
                self.tokens.get(self.pos + 2).map(|t| &t.kind),
                Some(TokenKind::Colon)
            ),
            _ => false,
        }
    }

    #[inline(never)]
    fn parse_call_suffix(&mut self, expr: Expr) -> Result<Expr, String> {
        let prev_allow = self.allow_struct_literal;
        self.allow_struct_literal = true;
        let span = expr.span();
        let mut pos_args = Vec::new();
        let mut named_args: Vec<(String, Expr)> = Vec::new();
        let res = (|| -> Result<Expr, String> {
            if !self.check(&TokenKind::RParen) {
                loop {
                    let arg_span = self.current_span();
                    if matches!(self.peek_kind(), TokenKind::Ident(_))
                        && matches!(self.peek_next_kind(), Some(TokenKind::Colon))
                    {
                        let name = self.expect_ident("Expected argument name")?;
                        self.advance();
                        if named_args.iter().any(|(k, _)| k == &name) {
                            return Err(format!(
                                "[{}:{}] Duplicate keyword argument '{}'",
                                arg_span.line, arg_span.col, name
                            ));
                        }
                        let val = self.parse_expr()?;
                        named_args.push((name, val));
                    } else {
                        if !named_args.is_empty() {
                            return Err(format!(
                                "[{}:{}] Positional argument cannot follow keyword argument",
                                arg_span.line, arg_span.col
                            ));
                        }
                        pos_args.push(self.parse_expr()?);
                    }
                    if !self.match_kind(&TokenKind::Comma) {
                        break;
                    }
                    if self.check(&TokenKind::RParen) {
                        break;
                    }
                }
            }
            self.expect(&TokenKind::RParen, "Expected ')' after arguments")?;
            if named_args.is_empty() {
                Ok(Expr::Call {
                    callee: Box::new(expr),
                    args: pos_args,
                    span,
                })
            } else {
                Ok(Expr::CallNamed(Box::new(CallNamedExpr {
                    callee: Box::new(expr),
                    pos_args,
                    named_args,
                    span,
                })))
            }
        })();
        self.allow_struct_literal = prev_allow;
        res
    }

    #[inline(never)]
    fn parse_struct_init_suffix(&mut self, expr: Expr) -> Result<Expr, String> {
        let prev_allow = self.allow_struct_literal;
        self.allow_struct_literal = true;
        let span = expr.span();
        self.advance();
        let res = (|| -> Result<Expr, String> {
            let mut fields: Vec<(String, Expr)> = Vec::new();
            let mut spread: Option<Box<Expr>> = None;
            while !self.check(&TokenKind::RBrace) && !self.is_eof() {
                let fspan = self.current_span();
                if self.match_kind(&TokenKind::DotDot) {
                    if spread.is_some() {
                        return Err(format!(
                            "[{}:{}] Only one '..spread' is allowed in struct literal",
                            fspan.line, fspan.col
                        ));
                    }
                    spread = Some(Box::new(self.parse_expr()?));
                } else {
                    let fname = self.expect_ident("Expected field name in struct literal")?;
                    self.expect(&TokenKind::Colon, "Expected ':' after field name")?;
                    if fields.iter().any(|(k, _)| k == &fname) {
                        return Err(format!(
                            "[{}:{}] Duplicate field '{}' in struct literal",
                            fspan.line, fspan.col, fname
                        ));
                    }
                    let val = self.parse_expr()?;
                    fields.push((fname, val));
                }
                if !self.match_kind(&TokenKind::Comma) {
                    break;
                }
            }
            self.expect(&TokenKind::RBrace, "Expected '}' after struct literal")?;
            Ok(Expr::StructInit(Box::new(StructInitExpr {
                target: Box::new(expr),
                fields,
                spread,
                span,
            })))
        })();
        self.allow_struct_literal = prev_allow;
        res
    }

    fn parse_postfix(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        let mut expr = self.parse_primary()?;
        loop {
            self.check_chain(start)?;
            if self.match_kind(&TokenKind::LParen) {
                expr = self.parse_call_suffix(expr)?;
            } else if self.allow_struct_literal
                && matches!(expr, Expr::Ident(..) | Expr::GetAttr { .. })
                && self.looks_like_struct_literal()
            {
                expr = self.parse_struct_init_suffix(expr)?;
            } else if self.match_kind(&TokenKind::LBracket) {
                let span = expr.span();
                if self.check(&TokenKind::DotDot) || self.check(&TokenKind::DotDotEq) {
                    let inclusive = self.match_kind(&TokenKind::DotDotEq);
                    if !inclusive {
                        self.advance();
                    }
                    let end = if self.check(&TokenKind::RBracket) {
                        None
                    } else {
                        Some(Box::new(self.parse_expr()?))
                    };
                    self.expect(&TokenKind::RBracket, "Expected ']' after slice")?;
                    expr = Expr::Slice {
                        object: Box::new(expr),
                        start: None,
                        end,
                        inclusive,
                        span,
                    };
                } else {
                    let first = self.parse_expr()?;
                    if self.check(&TokenKind::DotDot) || self.check(&TokenKind::DotDotEq) {
                        let inclusive = self.match_kind(&TokenKind::DotDotEq);
                        if !inclusive {
                            self.advance();
                        }
                        let end = if self.check(&TokenKind::RBracket) {
                            None
                        } else {
                            Some(Box::new(self.parse_expr()?))
                        };
                        self.expect(&TokenKind::RBracket, "Expected ']' after slice")?;
                        expr = Expr::Slice {
                            object: Box::new(expr),
                            start: Some(Box::new(first)),
                            end,
                            inclusive,
                            span,
                        };
                    } else {
                        self.expect(&TokenKind::RBracket, "Expected ']' after index")?;
                        expr = Expr::Index {
                            object: Box::new(expr),
                            index: Box::new(first),
                            span,
                        };
                    }
                }
            } else if self.match_kind(&TokenKind::Dot) {
                let span = expr.span();
                let name = self.expect_attr_name("Expected property or method name after '.'")?;
                expr = Expr::GetAttr {
                    object: Box::new(expr),
                    name,
                    span,
                };
            } else if self.match_kind(&TokenKind::QuestionDot) {
                let span = expr.span();
                let name = self.expect_attr_name("Expected property or method name after '?.'")?;
                expr = Expr::OptGetAttr {
                    object: Box::new(expr),
                    name,
                    span,
                };
            } else {
                break;
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        let span = self.current_span();
        match self.peek_kind().clone() {
            TokenKind::Int(v) => {
                self.advance();
                Ok(Expr::Int(v, span))
            }
            TokenKind::Float(v) => {
                self.advance();
                Ok(Expr::Float(v, span))
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Expr::Str(s, span))
            }
            TokenKind::Bytes(b) => {
                self.advance();
                Ok(Expr::Bytes(b, span))
            }
            TokenKind::InterpStart(first) => {
                self.advance();
                let start = self.pos;
                let mut acc = Expr::Str(first, span);
                loop {
                    self.check_chain(start)?;
                    let expr = self.parse_expr()?;
                    let expr_span = expr.span();
                    let str_call = Expr::Call {
                        callee: Box::new(Expr::Ident("str".to_string(), expr_span)),
                        args: vec![expr],
                        span: expr_span,
                    };
                    acc = Expr::Binary {
                        left: Box::new(acc),
                        op: BinaryOp::Add,
                        right: Box::new(str_call),
                        span,
                    };
                    match self.peek_kind().clone() {
                        TokenKind::InterpMid(mid) => {
                            self.advance();
                            if !mid.is_empty() {
                                acc = Expr::Binary {
                                    left: Box::new(acc),
                                    op: BinaryOp::Add,
                                    right: Box::new(Expr::Str(mid, span)),
                                    span,
                                };
                            }
                        }
                        TokenKind::InterpEnd(end) => {
                            self.advance();
                            if !end.is_empty() {
                                acc = Expr::Binary {
                                    left: Box::new(acc),
                                    op: BinaryOp::Add,
                                    right: Box::new(Expr::Str(end, span)),
                                    span,
                                };
                            }
                            break;
                        }
                        other => {
                            return Err(format!(
                                "[{}:{}] Expected string interpolation continuation, got {:?}",
                                span.line, span.col, other
                            ));
                        }
                    }
                }
                Ok(acc)
            }
            TokenKind::Bool(b) => {
                self.advance();
                Ok(Expr::Bool(b, span))
            }
            TokenKind::Unit => {
                self.advance();
                Ok(Expr::Unit(span))
            }
            TokenKind::Ident(name) => {
                self.advance();
                Ok(Expr::Ident(name, span))
            }
            TokenKind::LParen => {
                self.advance();
                if self.match_kind(&TokenKind::RParen) {
                    return Ok(Expr::Unit(span));
                }
                let prev = self.allow_struct_literal;
                self.allow_struct_literal = true;
                let expr = self.parse_expr();
                self.allow_struct_literal = prev;
                let expr = expr?;
                self.expect(&TokenKind::RParen, "Expected ')' after expression")?;
                Ok(expr)
            }
            TokenKind::LBracket => self.parse_bracket_expr(span),
            TokenKind::Hash => self.parse_dict_expr(span),
            TokenKind::LBrace => self.parse_block(),
            TokenKind::If => self.parse_if(),
            TokenKind::Try => self.parse_try(),
            TokenKind::Match => self.parse_match(),
            TokenKind::Fn => {
                self.advance();
                let params = self.parse_param_list()?;
                let body = self.parse_block()?;
                Ok(Expr::Fn {
                    params,
                    body: Box::new(body),
                    span,
                })
            }
            TokenKind::Bar => {
                self.advance();
                let mut params = Vec::new();
                if !self.check(&TokenKind::Bar) {
                    loop {
                        params.push(FnParam {
                            name: self.expect_ident("Expected parameter name")?,
                            default: None,
                            is_variadic: false,
                        });
                        if !self.match_kind(&TokenKind::Comma) {
                            break;
                        }
                        if self.check(&TokenKind::Bar) {
                            break;
                        }
                    }
                }
                self.expect(&TokenKind::Bar, "Expected '|' after lambda parameters")?;
                let body = self.parse_expr()?;
                Ok(Expr::Fn {
                    params,
                    body: Box::new(body),
                    span,
                })
            }
            TokenKind::Or => {
                self.advance();
                let body = self.parse_expr()?;
                Ok(Expr::Fn {
                    params: Vec::new(),
                    body: Box::new(body),
                    span,
                })
            }
            other => Err(format!(
                "[{}:{}] Unexpected token {:?}",
                span.line, span.col, other
            )),
        }
    }

    #[inline(never)]
    fn parse_bracket_expr(&mut self, span: Span) -> Result<Expr, String> {
        self.advance();
        if self.check(&TokenKind::RBracket) {
            self.advance();
            return Ok(Expr::List(Vec::new(), span));
        }
        let prev = self.allow_struct_literal;
        self.allow_struct_literal = true;
        let res = self.parse_bracket_inner(span);
        self.allow_struct_literal = prev;
        res
    }

    fn parse_bracket_inner(&mut self, span: Span) -> Result<Expr, String> {
        let first = self.parse_expr()?;
        if self.match_kind(&TokenKind::For) {
            let var = self.expect_ident("Expected variable name after 'for' in comprehension")?;
            self.expect(&TokenKind::In, "Expected 'in' in list comprehension")?;
            let iter = self.parse_expr()?;
            let cond = if self.match_kind(&TokenKind::If) {
                Some(Box::new(self.parse_expr()?))
            } else {
                None
            };
            self.expect(
                &TokenKind::RBracket,
                "Expected ']' after list comprehension",
            )?;
            return Ok(Expr::ListComp {
                expr: Box::new(first),
                var,
                iter: Box::new(iter),
                cond,
                span,
            });
        }
        let mut items = vec![first];
        while self.match_kind(&TokenKind::Comma) {
            if self.check(&TokenKind::RBracket) {
                break;
            }
            items.push(self.parse_expr()?);
        }
        self.expect(&TokenKind::RBracket, "Expected ']' after list elements")?;
        Ok(Expr::List(items, span))
    }

    #[inline(never)]
    fn parse_dict_expr(&mut self, span: Span) -> Result<Expr, String> {
        self.advance();
        self.expect(&TokenKind::LBrace, "Expected '{' after '#'")?;
        let prev = self.allow_struct_literal;
        self.allow_struct_literal = true;
        let mut entries = Vec::new();
        let res = (|| -> Result<Expr, String> {
            if !self.check(&TokenKind::RBrace) {
                loop {
                    let key_span = self.current_span();
                    let key = match self.peek_kind().clone() {
                        TokenKind::Ident(name) => {
                            self.advance();
                            Expr::Str(name, key_span)
                        }
                        _ => self.parse_expr()?,
                    };
                    self.expect(&TokenKind::Colon, "Expected ':' after dictionary key")?;
                    let val = self.parse_expr()?;
                    entries.push((key, val));
                    if !self.match_kind(&TokenKind::Comma) {
                        break;
                    }
                    if self.check(&TokenKind::RBrace) {
                        break;
                    }
                }
            }
            self.expect(&TokenKind::RBrace, "Expected '}' after dictionary entries")?;
            Ok(Expr::Dict(entries, span))
        })();
        self.allow_struct_literal = prev;
        res
    }

    fn parse_block(&mut self) -> Result<Expr, String> {
        self.depth += 1;
        if self.depth > MAX_PARSE_DEPTH {
            let span = self.current_span();
            return Err(format!(
                "[{}:{}] Maximum block nesting depth ({}) exceeded",
                span.line, span.col, MAX_PARSE_DEPTH
            ));
        }
        let span = self.current_span();
        self.expect(&TokenKind::LBrace, "Expected '{'")?;
        let prev = self.allow_struct_literal;
        self.allow_struct_literal = true;
        let res = self.parse_stmt_list(TokenKind::RBrace);
        self.allow_struct_literal = prev;
        let (stmts, tail) = res?;
        self.expect(&TokenKind::RBrace, "Expected '}'")?;
        self.depth -= 1;
        Ok(Expr::Block { stmts, tail, span })
    }

    fn check_chain(&self, start: usize) -> Result<(), String> {
        if self.pos.saturating_sub(start) > 512 {
            Err(
                "Expression token/chain limit exceeded (split into smaller expressions)"
                    .to_string(),
            )
        } else {
            Ok(())
        }
    }

    fn parse_if(&mut self) -> Result<Expr, String> {
        let span = self.current_span();
        self.expect(&TokenKind::If, "Expected 'if'")?;
        let cond = self.parse_expr_no_struct()?;
        let then_branch = self.parse_block()?;
        let else_branch = if self.match_kind(&TokenKind::Else) {
            if self.check(&TokenKind::If) {
                Some(Box::new(self.parse_expr()?))
            } else {
                Some(Box::new(self.parse_block()?))
            }
        } else {
            None
        };
        Ok(Expr::If {
            cond: Box::new(cond),
            then_branch: Box::new(then_branch),
            else_branch,
            span,
        })
    }

    fn parse_try(&mut self) -> Result<Expr, String> {
        let span = self.current_span();
        self.expect(&TokenKind::Try, "Expected 'try'")?;
        let try_body = self.parse_block()?;
        self.expect(&TokenKind::Catch, "Expected 'catch' after 'try' block")?;
        let catch_var = if self.match_kind(&TokenKind::LParen) {
            let name = self.expect_ident("Expected error variable name in catch")?;
            self.expect(&TokenKind::RParen, "Expected ')' after catch variable")?;
            Some(name)
        } else if matches!(self.peek_kind(), TokenKind::Ident(_)) {
            Some(self.expect_ident("Expected error variable name in catch")?)
        } else {
            None
        };
        let catch_body = self.parse_block()?;
        Ok(Expr::TryCatch {
            try_body: Box::new(try_body),
            catch_var,
            catch_body: Box::new(catch_body),
            span,
        })
    }

    fn parse_match(&mut self) -> Result<Expr, String> {
        let span = self.current_span();
        self.expect(&TokenKind::Match, "Expected 'match'")?;
        let subject = self.parse_expr_no_struct()?;
        self.expect(&TokenKind::LBrace, "Expected '{' after match subject")?;
        let mut arms = Vec::new();

        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            let arm_span = self.current_span();
            let pattern = self.parse_pattern()?;
            let guard = if self.match_kind(&TokenKind::If) {
                Some(self.parse_expr()?)
            } else {
                None
            };
            self.expect(&TokenKind::FatArrow, "Expected '=>' after match pattern")?;
            let body = self.parse_expr()?;
            let needs_comma = !is_block_expr(&body);
            arms.push(MatchArm {
                pattern,
                guard,
                body,
                span: arm_span,
            });
            if self.match_kind(&TokenKind::Comma) {
                continue;
            }
            if !needs_comma {
                continue;
            }
            if !self.check(&TokenKind::RBrace) {
                let s = self.current_span();
                return Err(format!(
                    "[{}:{}] Expected ',' or '}}' after match arm",
                    s.line, s.col
                ));
            }
        }

        self.expect(&TokenKind::RBrace, "Expected '}' after match arms")?;
        Ok(Expr::Match {
            subject: Box::new(subject),
            arms,
            span,
        })
    }

    fn parse_pattern(&mut self) -> Result<Pattern, String> {
        let span = self.current_span();
        match self.peek_kind().clone() {
            TokenKind::Minus => {
                self.advance();
                match self.peek_kind().clone() {
                    TokenKind::Int(v) => {
                        self.advance();
                        self.maybe_range_pattern(-v)
                    }
                    TokenKind::Float(v) => {
                        self.advance();
                        Ok(Pattern::Float(-v))
                    }
                    _ => Err(format!(
                        "[{}:{}] Expected number after '-' in pattern",
                        span.line, span.col
                    )),
                }
            }
            TokenKind::Int(v) => {
                self.advance();
                self.maybe_range_pattern(v)
            }
            TokenKind::Float(v) => {
                self.advance();
                Ok(Pattern::Float(v))
            }
            TokenKind::Bool(b) => {
                self.advance();
                Ok(Pattern::Bool(b))
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Pattern::Str(s))
            }
            TokenKind::Unit => {
                self.advance();
                Ok(Pattern::Unit)
            }
            TokenKind::LParen => {
                self.advance();
                self.expect(&TokenKind::RParen, "Expected ')' for unit pattern")?;
                Ok(Pattern::Unit)
            }
            TokenKind::Ident(name) => {
                self.advance();
                if name == "_" {
                    Ok(Pattern::Wildcard)
                } else {
                    Ok(Pattern::Ident(name))
                }
            }
            other => Err(format!(
                "[{}:{}] Invalid pattern token {:?}",
                span.line, span.col, other
            )),
        }
    }

    fn maybe_range_pattern(&mut self, start: i64) -> Result<Pattern, String> {
        let inclusive = if self.match_kind(&TokenKind::DotDotEq) {
            true
        } else if self.match_kind(&TokenKind::DotDot) {
            false
        } else {
            return Ok(Pattern::Int(start));
        };
        let span = self.current_span();
        let neg = self.match_kind(&TokenKind::Minus);
        let end = match self.peek_kind().clone() {
            TokenKind::Int(v) => {
                self.advance();
                if neg { -v } else { v }
            }
            _ => {
                return Err(format!(
                    "[{}:{}] Expected integer end bound in match range",
                    span.line, span.col
                ));
            }
        };
        Ok(Pattern::Range {
            start,
            end,
            inclusive,
        })
    }

    fn parse_import_source(
        &mut self,
        span: Span,
        kw: &str,
    ) -> Result<(ImportSource, String), String> {
        match self.peek_kind().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                Ok((ImportSource::Name(name.clone()), name))
            }
            TokenKind::Str(path) => {
                self.advance();
                let stem = std::path::Path::new(&path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("module")
                    .to_string();
                Ok((ImportSource::Path(path), stem))
            }
            _ => Err(format!(
                "[{}:{}] Expected module name or path string after '{}'",
                span.line, span.col, kw
            )),
        }
    }

    fn parse_let_stmt(&mut self, span: Span, is_pub: bool) -> Result<Stmt, String> {
        if self.match_kind(&TokenKind::LBracket) {
            let mut names = Vec::new();
            if !self.check(&TokenKind::RBracket) {
                loop {
                    names.push(self.expect_ident("Expected variable name in list destructuring")?);
                    if !self.match_kind(&TokenKind::Comma) {
                        break;
                    }
                    if self.check(&TokenKind::RBracket) {
                        break;
                    }
                }
            }
            self.expect(
                &TokenKind::RBracket,
                "Expected ']' after list destructuring",
            )?;
            self.expect(
                &TokenKind::Assign,
                "Expected '=' after destructuring pattern",
            )?;
            let value = self.parse_expr()?;
            self.expect(&TokenKind::Semicolon, "Expected ';' after 'let' statement")?;
            Ok(Stmt::LetList {
                names,
                value,
                is_pub,
                span,
            })
        } else if self.match_kind(&TokenKind::Hash) {
            self.expect(&TokenKind::LBrace, "Expected '{' after '#'")?;
            let mut fields = Vec::new();
            if !self.check(&TokenKind::RBrace) {
                loop {
                    fields.push(self.expect_ident("Expected field name in dict destructuring")?);
                    if !self.match_kind(&TokenKind::Comma) {
                        break;
                    }
                    if self.check(&TokenKind::RBrace) {
                        break;
                    }
                }
            }
            self.expect(&TokenKind::RBrace, "Expected '}' after dict destructuring")?;
            self.expect(
                &TokenKind::Assign,
                "Expected '=' after destructuring pattern",
            )?;
            let value = self.parse_expr()?;
            self.expect(&TokenKind::Semicolon, "Expected ';' after 'let' statement")?;
            Ok(Stmt::LetDict {
                fields,
                value,
                is_pub,
                span,
            })
        } else {
            let name = self.expect_ident("Expected variable name after 'let'")?;
            self.expect(&TokenKind::Assign, "Expected '=' after variable name")?;
            let value = self.parse_expr()?;
            self.expect(&TokenKind::Semicolon, "Expected ';' after 'let' statement")?;
            Ok(Stmt::Let {
                name,
                value,
                is_pub,
                span,
            })
        }
    }

    fn parse_struct_decl(&mut self, span: Span, is_pub: bool) -> Result<Stmt, String> {
        let name = self.expect_ident("Expected struct name")?;
        self.expect(&TokenKind::LBrace, "Expected '{' after struct name")?;
        let mut fields: Vec<StructField> = Vec::new();
        let mut methods: Vec<StructMethod> = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.is_eof() {
            while self.match_kind(&TokenKind::Comma) || self.match_kind(&TokenKind::Semicolon) {}
            if self.check(&TokenKind::RBrace) {
                break;
            }
            let mspan = self.current_span();
            if self.match_kind(&TokenKind::Fn) {
                let mname = self.expect_ident("Expected method name in struct")?;
                if methods.iter().any(|m| m.name == mname) || fields.iter().any(|f| f.name == mname)
                {
                    return Err(format!(
                        "[{}:{}] Duplicate member '{}' in struct '{}'",
                        mspan.line, mspan.col, mname, name
                    ));
                }
                let params = self.parse_param_list()?;
                let body = self.parse_block()?;
                methods.push(StructMethod {
                    name: mname,
                    params,
                    body,
                    span: mspan,
                });
            } else {
                let fname = self.expect_ident("Expected field or 'fn' method in struct")?;
                if fields.iter().any(|f| f.name == fname) || methods.iter().any(|m| m.name == fname)
                {
                    return Err(format!(
                        "[{}:{}] Duplicate member '{}' in struct '{}'",
                        mspan.line, mspan.col, fname, name
                    ));
                }
                let default = if self.match_kind(&TokenKind::Assign) {
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                fields.push(StructField {
                    name: fname,
                    default,
                });
                let _ =
                    self.match_kind(&TokenKind::Comma) || self.match_kind(&TokenKind::Semicolon);
            }
        }
        self.expect(&TokenKind::RBrace, "Expected '}' after struct body")?;
        Ok(Stmt::StructDecl {
            name,
            is_pub,
            fields,
            methods,
            span,
        })
    }

    fn parse_fn_decl(&mut self, span: Span, is_pub: bool) -> Result<Stmt, String> {
        let name = self.expect_ident("Expected function name")?;
        let params = self.parse_param_list()?;
        let body = self.parse_block()?;
        Ok(Stmt::FnDecl {
            name,
            params,
            body,
            is_pub,
            span,
        })
    }

    fn parse_param_list(&mut self) -> Result<Vec<FnParam>, String> {
        self.expect(&TokenKind::LParen, "Expected '(' for parameter list")?;
        let mut params: Vec<FnParam> = Vec::new();
        let mut seen_default = false;
        let mut seen_variadic = false;
        if !self.check(&TokenKind::RParen) {
            loop {
                let pspan = self.current_span();
                if seen_variadic {
                    return Err(format!(
                        "[{}:{}] Variadic '*args' parameter must be last",
                        pspan.line, pspan.col
                    ));
                }
                let is_variadic = self.match_kind(&TokenKind::Star);
                let name = self.expect_ident("Expected parameter name")?;
                if params.iter().any(|p| p.name == name) {
                    return Err(format!(
                        "[{}:{}] Duplicate parameter '{}'",
                        pspan.line, pspan.col, name
                    ));
                }
                let default = if self.match_kind(&TokenKind::Assign) {
                    if is_variadic {
                        return Err(format!(
                            "[{}:{}] Variadic parameter '*{}' cannot have a default value",
                            pspan.line, pspan.col, name
                        ));
                    }
                    seen_default = true;
                    Some(self.parse_expr()?)
                } else {
                    if !is_variadic && seen_default {
                        return Err(format!(
                            "[{}:{}] Required parameter '{}' cannot follow a default parameter",
                            pspan.line, pspan.col, name
                        ));
                    }
                    None
                };
                if is_variadic {
                    seen_variadic = true;
                }
                params.push(FnParam {
                    name,
                    default,
                    is_variadic,
                });
                if !self.match_kind(&TokenKind::Comma) {
                    break;
                }
                if self.check(&TokenKind::RParen) {
                    break;
                }
            }
        }
        self.expect(&TokenKind::RParen, "Expected ')' after parameter list")?;
        Ok(params)
    }

    fn expect_ident(&mut self, msg: &str) -> Result<String, String> {
        let span = self.current_span();
        match self.peek_kind().clone() {
            TokenKind::Ident(name) => {
                self.advance();
                Ok(name)
            }
            _ => Err(format!("[{}:{}] {}", span.line, span.col, msg)),
        }
    }

    fn expect_attr_name(&mut self, msg: &str) -> Result<String, String> {
        let span = self.current_span();
        let name = match self.peek_kind().clone() {
            TokenKind::Ident(s) => s,
            TokenKind::Struct => "struct".to_string(),
            TokenKind::Match => "match".to_string(),
            TokenKind::In => "in".to_string(),
            TokenKind::For => "for".to_string(),
            TokenKind::Try => "try".to_string(),
            TokenKind::Catch => "catch".to_string(),
            TokenKind::Loop => "loop".to_string(),
            TokenKind::As => "as".to_string(),
            _ => return Err(format!("[{}:{}] {}", span.line, span.col, msg)),
        };
        self.advance();
        Ok(name)
    }

    fn expect(&mut self, kind: &TokenKind, msg: &str) -> Result<(), String> {
        if self.check(kind) {
            self.advance();
            Ok(())
        } else {
            let span = self.current_span();
            Err(format!("[{}:{}] {}", span.line, span.col, msg))
        }
    }

    fn match_kind(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn check(&self, kind: &TokenKind) -> bool {
        self.peek_kind() == kind
    }

    fn is_eof(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Eof)
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn peek_next_kind(&self) -> Option<&TokenKind> {
        self.tokens.get(self.pos + 1).map(|t| &t.kind)
    }

    fn current_span(&self) -> Span {
        self.tokens[self.pos].span
    }

    fn advance(&mut self) {
        if !self.is_eof() {
            self.pos += 1;
        }
    }
}

fn is_block_expr(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::Block { .. }
            | Expr::If { .. }
            | Expr::TryCatch { .. }
            | Expr::Match { .. }
            | Expr::Fn { .. }
    )
}
