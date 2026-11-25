use crate::ast::{
    AssignTarget, BinaryOp, CallNamedExpr, ChainedCmpExpr, Expr, FnParam, ImportItem, ImportSource,
    MatchArm, Pattern, PipeAssignExpr, Program, SafePipeExpr, SandboxExpr, Stmt, StructField,
    StructInitExpr, StructMethod, TaggedTemplateExpr, UnaryOp, WithExpr,
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

    pub fn parse_program_recover(&mut self) -> (Program, Vec<String>) {
        let mut stmts = Vec::new();
        let mut tail = None;
        let mut errors = Vec::new();
        const MAX_RECOVER_ERRORS: usize = 25;

        while !self.is_eof() {
            let prev_pos = self.pos;
            if self.check(&TokenKind::Semicolon) {
                self.advance();
                continue;
            }
            if self.is_stmt_start() {
                match self.parse_stmt() {
                    Ok(stmt) => stmts.push(stmt),
                    Err(e) => {
                        errors.push(e);
                        if errors.len() >= MAX_RECOVER_ERRORS {
                            break;
                        }
                        self.synchronize();
                    }
                }
                if !self.is_eof() && self.pos == prev_pos {
                    self.advance();
                }
                continue;
            }
            match self.parse_expr() {
                Ok(expr) => {
                    if self.match_kind(&TokenKind::Semicolon) {
                        stmts.push(Stmt::Expr(expr));
                    } else if self.is_eof() {
                        tail = Some(Box::new(expr));
                        break;
                    } else if is_block_expr(&expr) {
                        stmts.push(Stmt::Expr(expr));
                    } else {
                        let span = self.current_span();
                        errors.push(format!(
                            "[{}:{}] Expected ';' or 'EOF' after expression",
                            span.line, span.col
                        ));
                        if errors.len() >= MAX_RECOVER_ERRORS {
                            break;
                        }
                        self.synchronize();
                    }
                }
                Err(e) => {
                    errors.push(e);
                    if errors.len() >= MAX_RECOVER_ERRORS {
                        break;
                    }
                    self.synchronize();
                }
            }
            if !self.is_eof() && self.pos == prev_pos {
                self.advance();
            }
        }
        (Program { stmts, tail }, errors)
    }

    fn synchronize(&mut self) {
        while !self.is_eof() {
            match self.peek_kind() {
                TokenKind::Semicolon => {
                    self.advance();
                    return;
                }
                TokenKind::RBrace => {
                    self.advance();
                    return;
                }
                TokenKind::Match | TokenKind::If | TokenKind::Try => {
                    return;
                }
                _ if self.is_stmt_start() => {
                    return;
                }
                _ => {
                    self.advance();
                }
            }
        }
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

    fn is_needs_stmt(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Ident(s) if s == "needs")
            && matches!(self.peek_next_kind(), Some(TokenKind::Ident(_)))
    }

    fn is_defer_stmt(&self) -> bool {
        if !matches!(self.peek_kind(), TokenKind::Ident(s) if s == "defer") {
            return false;
        }
        matches!(
            self.peek_next_kind(),
            Some(
                TokenKind::LBrace
                    | TokenKind::Ident(_)
                    | TokenKind::If
                    | TokenKind::Try
                    | TokenKind::Match
            )
        )
    }

    fn looks_like_sandbox(&self) -> bool {
        if matches!(self.peek_next_kind(), Some(TokenKind::LBrace)) {
            return true;
        }
        if !matches!(self.peek_next_kind(), Some(TokenKind::LParen)) {
            return false;
        }
        let mut depth = 0usize;
        let mut i = self.pos + 1;
        while let Some(t) = self.tokens.get(i) {
            match t.kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                    if depth == 0 {
                        return matches!(
                            self.tokens.get(i + 1).map(|n| &n.kind),
                            Some(TokenKind::LBrace)
                        );
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    fn looks_like_control_block(&self, allow_as: bool) -> bool {
        let Some(first) = self.tokens.get(self.pos + 1).map(|t| &t.kind) else {
            return false;
        };
        match first {
            TokenKind::Int(_)
            | TokenKind::Float(_)
            | TokenKind::Str(_)
            | TokenKind::Bytes(_)
            | TokenKind::Bool(_)
            | TokenKind::Unit
            | TokenKind::Ident(_)
            | TokenKind::LParen
            | TokenKind::LBracket
            | TokenKind::Hash
            | TokenKind::If
            | TokenKind::Try
            | TokenKind::Match => {}
            _ => return false,
        }
        let starts_with_lparen = matches!(first, TokenKind::LParen);
        let mut pdepth = 0usize;
        let mut bdepth = 0usize;
        let mut dict_depth = 0usize;
        let mut i = self.pos + 1;
        while i < self.tokens.len() {
            let prev_is_hash =
                i > self.pos + 1 && matches!(self.tokens[i - 1].kind, TokenKind::Hash);
            match &self.tokens[i].kind {
                TokenKind::LParen => pdepth += 1,
                TokenKind::RParen => {
                    if pdepth == 0 {
                        return false;
                    }
                    pdepth -= 1;
                    if starts_with_lparen && pdepth == 0 && bdepth == 0 && dict_depth == 0 {
                        let next_ok = match self.tokens.get(i + 1).map(|t| &t.kind) {
                            Some(TokenKind::LBrace) if self.allow_struct_literal => true,
                            Some(TokenKind::As) if allow_as => true,
                            _ => false,
                        };
                        if next_ok {
                            return true;
                        }
                        if matches!(
                            self.tokens.get(i + 1).map(|t| &t.kind),
                            Some(
                                TokenKind::Semicolon
                                    | TokenKind::RBrace
                                    | TokenKind::Comma
                                    | TokenKind::Eof
                            )
                        ) {
                            return false;
                        }
                    }
                }
                TokenKind::LBracket => bdepth += 1,
                TokenKind::RBracket => {
                    if bdepth == 0 {
                        return false;
                    }
                    bdepth -= 1;
                }
                TokenKind::LBrace => {
                    if prev_is_hash || dict_depth > 0 {
                        dict_depth += 1;
                    } else if pdepth == 0 && bdepth == 0 {
                        return self.allow_struct_literal;
                    }
                }
                TokenKind::RBrace if pdepth == 0 && bdepth == 0 => {
                    if dict_depth > 0 {
                        dict_depth -= 1;
                    } else {
                        return false;
                    }
                }
                TokenKind::As if allow_as && pdepth == 0 && bdepth == 0 && dict_depth == 0 => {
                    return matches!(
                        self.tokens.get(i + 1).map(|t| &t.kind),
                        Some(TokenKind::Ident(_))
                    ) && matches!(
                        self.tokens.get(i + 2).map(|t| &t.kind),
                        Some(TokenKind::LBrace)
                    );
                }
                TokenKind::Eq
                | TokenKind::Ne
                | TokenKind::Lt
                | TokenKind::Le
                | TokenKind::Gt
                | TokenKind::Ge
                | TokenKind::And
                | TokenKind::Or
                | TokenKind::Semicolon
                | TokenKind::Assign
                | TokenKind::PlusAssign
                | TokenKind::MinusAssign
                | TokenKind::StarAssign
                | TokenKind::SlashAssign
                | TokenKind::PipeAssign
                | TokenKind::Eof
                    if pdepth == 0 && bdepth == 0 && dict_depth == 0 =>
                {
                    return false;
                }
                _ => {}
            }
            i += 1;
        }
        false
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
            TokenKind::Ident(s) if s == "needs" => self.is_needs_stmt(),
            TokenKind::Ident(s) if s == "defer" => self.is_defer_stmt(),
            TokenKind::Ident(s) if s == "every" => self.looks_like_control_block(false),
            TokenKind::Ident(s) if s == "on" => {
                matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(c)) if c == "change"
                ) && matches!(
                    self.tokens.get(self.pos + 2).map(|t| &t.kind),
                    Some(TokenKind::LParen)
                )
            }
            TokenKind::Ident(s) if s == "spec" => {
                matches!(self.peek_next_kind(), Some(TokenKind::Str(_)))
                    && matches!(
                        self.tokens.get(self.pos + 2).map(|t| &t.kind),
                        Some(TokenKind::LBrace)
                    )
            }
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
            TokenKind::Ident(s) if s == "needs" => {
                self.advance();
                let mut caps = Vec::new();
                loop {
                    let cap_name = self.expect_ident("Expected capability name after 'needs'")?;
                    let mut args = Vec::new();
                    if self.match_kind(&TokenKind::LParen) {
                        if !self.check(&TokenKind::RParen) {
                            loop {
                                match self.peek_kind().clone() {
                                    TokenKind::Str(v) => {
                                        self.advance();
                                        args.push(v);
                                    }
                                    _ => {
                                        let sp = self.current_span();
                                        return Err(format!(
                                            "[{}:{}] Capability argument in 'needs' must be a string literal",
                                            sp.line, sp.col
                                        ));
                                    }
                                }
                                if !self.match_kind(&TokenKind::Comma) {
                                    break;
                                }
                                if self.check(&TokenKind::RParen) {
                                    break;
                                }
                            }
                        }
                        self.expect(
                            &TokenKind::RParen,
                            "Expected ')' after capability arguments in 'needs'",
                        )?;
                    }
                    caps.push((cap_name, args));
                    if !self.match_kind(&TokenKind::Comma) {
                        break;
                    }
                    if self.check(&TokenKind::Semicolon) {
                        break;
                    }
                }
                self.expect(&TokenKind::Semicolon, "Expected ';' after 'needs' contract")?;
                Ok(Stmt::Needs { caps, span })
            }
            TokenKind::Ident(s) if s == "defer" => {
                self.advance();
                let body = if self.check(&TokenKind::LBrace) {
                    let b = self.parse_block()?;
                    let _ = self.match_kind(&TokenKind::Semicolon);
                    b
                } else {
                    let e = self.parse_expr()?;
                    self.expect(
                        &TokenKind::Semicolon,
                        "Expected ';' after 'defer' expression",
                    )?;
                    e
                };
                Ok(Stmt::Defer { body, span })
            }
            TokenKind::Ident(s) if s == "every" => {
                self.advance();
                let interval = self.parse_expr_no_struct()?;
                let body = self.parse_block()?;
                Ok(Stmt::Every {
                    interval,
                    body,
                    span,
                })
            }
            TokenKind::Ident(s)
                if s == "on"
                    && matches!(
                        self.tokens.get(self.pos + 1).map(|t| &t.kind),
                        Some(TokenKind::Ident(c)) if c == "change"
                    )
                    && matches!(
                        self.tokens.get(self.pos + 2).map(|t| &t.kind),
                        Some(TokenKind::LParen)
                    ) =>
            {
                self.advance();
                self.advance();
                self.expect(&TokenKind::LParen, "Expected '(' after 'on change'")?;
                let sig_expr = self.parse_expr()?;
                self.expect(
                    &TokenKind::RParen,
                    "Expected ')' after signal in 'on change'",
                )?;
                let body = self.parse_block()?;
                let _ = self.match_kind(&TokenKind::Semicolon);
                let cb = Expr::Fn {
                    params: vec![
                        FnParam {
                            name: "_val".to_string(),
                            default: None,
                            is_variadic: false,
                        },
                        FnParam {
                            name: "_old".to_string(),
                            default: None,
                            is_variadic: false,
                        },
                    ],
                    body: Box::new(body),
                    span,
                };
                Ok(Stmt::Expr(Expr::Call {
                    callee: Box::new(Expr::GetAttr {
                        object: Box::new(sig_expr),
                        name: "watch".to_string(),
                        span,
                    }),
                    args: vec![cb],
                    span,
                }))
            }
            TokenKind::Ident(s) if s == "spec" => {
                self.advance();
                let name = match self.peek_kind().clone() {
                    TokenKind::Str(v) => {
                        self.advance();
                        v
                    }
                    other => {
                        return Err(format!(
                            "[{}:{}] Expected string name after 'spec', got {:?}",
                            span.line, span.col, other
                        ));
                    }
                };
                let body = self.parse_block()?;
                Ok(Stmt::Spec { name, body, span })
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
        if self.depth >= MAX_PARSE_DEPTH {
            let span = self.current_span();
            return Err(format!(
                "[{}:{}] Maximum expression nesting depth ({}) exceeded",
                span.line, span.col, MAX_PARSE_DEPTH
            ));
        }
        self.depth += 1;
        let res = self.parse_assign();
        self.depth -= 1;
        res
    }

    fn expr_to_assign_target(expr: &Expr, span: Span) -> Result<AssignTarget, String> {
        match expr {
            Expr::Ident(name, _) => Ok(AssignTarget::Var(name.clone())),
            Expr::Index { object, index, .. } => Ok(AssignTarget::Index {
                object: object.clone(),
                index: index.clone(),
            }),
            Expr::GetAttr { object, name, .. } => Ok(AssignTarget::Attr {
                object: object.clone(),
                name: name.clone(),
            }),
            _ => Err(format!(
                "[{}:{}] Invalid assignment target",
                span.line, span.col
            )),
        }
    }

    #[inline(never)]
    fn parse_pipe_assign_suffix(&mut self, expr: Expr) -> Result<Expr, String> {
        let span = expr.span();
        let target = Self::expr_to_assign_target(&expr, span)?;
        let param = "$pa".to_string();
        let mut stages = Vec::new();
        let start = self.pos;
        while self.match_kind(&TokenKind::PipeAssign) {
            self.check_chain(start)?;
            let rhs = self.parse_term()?;
            stages.push(Self::apply_pipeline_rhs(
                Expr::Ident(param.clone(), rhs.span()),
                rhs,
            ));
        }
        Ok(Expr::PipeAssign(Box::new(PipeAssignExpr {
            target: Box::new(target),
            param,
            stages,
            span,
        })))
    }

    #[inline(never)]
    fn parse_recover_suffix(&mut self, mut expr: Expr) -> Result<Expr, String> {
        let start = self.pos;
        while self.match_kind(&TokenKind::Recover) {
            self.check_chain(start)?;
            let span = expr.span();
            let catch_var = if matches!(self.peek_kind(), TokenKind::Ident(_)) {
                Some(self.expect_ident("Expected error variable after 'recover'")?)
            } else {
                None
            };
            self.expect(&TokenKind::FatArrow, "Expected '=>' after 'recover'")?;
            let fallback = self.parse_coalesce()?;
            expr = Expr::TryCatch {
                try_body: Box::new(expr),
                catch_var,
                catch_body: Box::new(fallback),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_assign(&mut self) -> Result<Expr, String> {
        let mut expr = self.parse_coalesce()?;
        if self.check(&TokenKind::Recover) {
            expr = self.parse_recover_suffix(expr)?;
        }
        if self.check(&TokenKind::PipeAssign) {
            return self.parse_pipe_assign_suffix(expr);
        }
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
            let target = Self::expr_to_assign_target(&expr, span)?;
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

    fn count_placeholders_stmt(stmt: &Stmt) -> (usize, bool) {
        match stmt {
            Stmt::Let { name, value, .. } => (Self::count_placeholders(value), name == "_"),
            Stmt::LetList { names, value, .. } => (
                Self::count_placeholders(value),
                names.iter().any(|n| n == "_"),
            ),
            Stmt::LetDict { fields, value, .. } => (
                Self::count_placeholders(value),
                fields.iter().any(|f| f == "_"),
            ),
            Stmt::FnDecl {
                name, params, body, ..
            } => {
                let c = if params.iter().any(|p| p.name == "_") {
                    0
                } else {
                    Self::count_placeholders(body)
                };
                (c, name == "_")
            }
            Stmt::While { cond, body, .. } => (
                Self::count_placeholders(cond) + Self::count_placeholders(body),
                false,
            ),
            Stmt::For {
                var, iter, body, ..
            } => {
                let mut c = Self::count_placeholders(iter);
                if var != "_" {
                    c += Self::count_placeholders(body);
                }
                (c, false)
            }
            Stmt::ForPair {
                var1,
                var2,
                iter,
                body,
                ..
            } => {
                let mut c = Self::count_placeholders(iter);
                if var1 != "_" && var2 != "_" {
                    c += Self::count_placeholders(body);
                }
                (c, false)
            }
            Stmt::Return { value, .. } => {
                (value.as_ref().map_or(0, Self::count_placeholders), false)
            }
            Stmt::Defer { body, .. } => (Self::count_placeholders(body), false),
            Stmt::Every { interval, body, .. } => (
                Self::count_placeholders(interval) + Self::count_placeholders(body),
                false,
            ),
            Stmt::Expr(e) => (Self::count_placeholders(e), false),
            _ => (0, false),
        }
    }

    fn count_placeholders(expr: &Expr) -> usize {
        match expr {
            Expr::Ident(name, _) if name == "_" => 1,
            Expr::Unary { expr: inner, .. } | Expr::Throw { value: inner, .. } => {
                Self::count_placeholders(inner)
            }
            Expr::Binary { left, right, .. } | Expr::NullCoalesce { left, right, .. } => {
                Self::count_placeholders(left) + Self::count_placeholders(right)
            }
            Expr::Assign { target, value, .. } => {
                let tc = match target.as_ref() {
                    AssignTarget::Var(_) => 0,
                    AssignTarget::Index { object, index } => {
                        Self::count_placeholders(object) + Self::count_placeholders(index)
                    }
                    AssignTarget::Attr { object, .. } => Self::count_placeholders(object),
                };
                tc + Self::count_placeholders(value)
            }
            Expr::Block { stmts, tail, .. } => {
                let mut total = 0usize;
                let mut shadowed = false;
                for s in stmts {
                    let (c, sh) = Self::count_placeholders_stmt(s);
                    total += c;
                    if sh {
                        shadowed = true;
                        break;
                    }
                }
                if !shadowed {
                    if let Some(t) = tail {
                        total += Self::count_placeholders(t);
                    }
                }
                total
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                Self::count_placeholders(cond)
                    + Self::count_placeholders(then_branch)
                    + else_branch.as_deref().map_or(0, Self::count_placeholders)
            }
            Expr::Match { subject, arms, .. } => {
                Self::count_placeholders(subject)
                    + arms
                        .iter()
                        .map(|a| {
                            a.guard.as_ref().map_or(0, Self::count_placeholders)
                                + Self::count_placeholders(&a.body)
                        })
                        .sum::<usize>()
            }
            Expr::Fn { params, body, .. } => {
                let def_count: usize = params
                    .iter()
                    .filter_map(|p| p.default.as_ref())
                    .map(Self::count_placeholders)
                    .sum();
                if params.iter().any(|p| p.name == "_") {
                    def_count
                } else {
                    def_count + Self::count_placeholders(body)
                }
            }
            Expr::List(items, _) => items.iter().map(Self::count_placeholders).sum(),
            Expr::Dict(entries, _) => entries
                .iter()
                .map(|(k, v)| Self::count_placeholders(k) + Self::count_placeholders(v))
                .sum(),
            Expr::Call { callee, args, .. } => {
                Self::count_placeholders(callee)
                    + args.iter().map(Self::count_placeholders).sum::<usize>()
            }
            Expr::CallNamed(c) => {
                Self::count_placeholders(&c.callee)
                    + c.pos_args
                        .iter()
                        .map(Self::count_placeholders)
                        .sum::<usize>()
                    + c.named_args
                        .iter()
                        .map(|(_, v)| Self::count_placeholders(v))
                        .sum::<usize>()
            }
            Expr::StructInit(s) => {
                Self::count_placeholders(&s.target)
                    + s.spread.as_deref().map_or(0, Self::count_placeholders)
                    + s.fields
                        .iter()
                        .map(|(_, v)| Self::count_placeholders(v))
                        .sum::<usize>()
            }
            Expr::ChainedCmp(c) => {
                Self::count_placeholders(&c.first)
                    + c.comparisons
                        .iter()
                        .map(|(_, rhs)| Self::count_placeholders(rhs))
                        .sum::<usize>()
            }
            Expr::Index { object, index, .. } => {
                Self::count_placeholders(object) + Self::count_placeholders(index)
            }
            Expr::Slice {
                object, start, end, ..
            } => {
                Self::count_placeholders(object)
                    + start.as_deref().map_or(0, Self::count_placeholders)
                    + end.as_deref().map_or(0, Self::count_placeholders)
            }
            Expr::ListComp {
                expr,
                var,
                iter,
                cond,
                ..
            } => {
                let mut c = Self::count_placeholders(iter);
                if var != "_" {
                    c += Self::count_placeholders(expr)
                        + cond.as_deref().map_or(0, Self::count_placeholders);
                }
                c
            }
            Expr::GetAttr { object, .. } | Expr::OptGetAttr { object, .. } => {
                Self::count_placeholders(object)
            }
            Expr::TryCatch {
                try_body,
                catch_var,
                catch_body,
                ..
            } => {
                Self::count_placeholders(try_body)
                    + if catch_var.as_deref() == Some("_") {
                        0
                    } else {
                        Self::count_placeholders(catch_body)
                    }
            }
            Expr::SafePipe(p) => {
                Self::count_placeholders(&p.left)
                    + if p.param == "_" {
                        0
                    } else {
                        Self::count_placeholders(&p.apply)
                    }
            }
            Expr::MapPipe { left, mapper, .. } => {
                Self::count_placeholders(left) + Self::count_placeholders(mapper)
            }
            Expr::PipeAssign(p) => {
                let tc = match p.target.as_ref() {
                    AssignTarget::Var(_) => 0,
                    AssignTarget::Index { object, index } => {
                        Self::count_placeholders(object) + Self::count_placeholders(index)
                    }
                    AssignTarget::Attr { object, .. } => Self::count_placeholders(object),
                };
                tc + if p.param == "_" {
                    0
                } else {
                    p.stages.iter().map(Self::count_placeholders).sum()
                }
            }
            Expr::TaggedTemplate(t) => t.exprs.iter().map(Self::count_placeholders).sum(),
            Expr::With(w) => {
                Self::count_placeholders(&w.resource)
                    + if w.alias.as_deref() == Some("_") {
                        0
                    } else {
                        Self::count_placeholders(&w.body)
                    }
            }
            Expr::Sandbox(s) => {
                s.policy
                    .iter()
                    .map(|(_, v)| Self::count_placeholders(v))
                    .sum::<usize>()
                    + Self::count_placeholders(&s.body)
            }
            Expr::After { delay, body, .. } => {
                Self::count_placeholders(delay) + Self::count_placeholders(body)
            }
            _ => 0,
        }
    }

    fn replace_placeholders_stmt(stmt: Stmt, replacement: &Expr) -> (Stmt, bool) {
        match stmt {
            Stmt::Let {
                name,
                name_span,
                value,
                is_pub,
                span,
            } => {
                let sh = name == "_";
                (
                    Stmt::Let {
                        name,
                        name_span,
                        value: Self::replace_placeholders(value, replacement),
                        is_pub,
                        span,
                    },
                    sh,
                )
            }
            Stmt::LetList {
                names,
                name_spans,
                value,
                is_pub,
                span,
            } => {
                let sh = names.iter().any(|n| n == "_");
                (
                    Stmt::LetList {
                        names,
                        name_spans,
                        value: Self::replace_placeholders(value, replacement),
                        is_pub,
                        span,
                    },
                    sh,
                )
            }
            Stmt::LetDict {
                fields,
                field_spans,
                value,
                is_pub,
                span,
            } => {
                let sh = fields.iter().any(|f| f == "_");
                (
                    Stmt::LetDict {
                        fields,
                        field_spans,
                        value: Self::replace_placeholders(value, replacement),
                        is_pub,
                        span,
                    },
                    sh,
                )
            }
            Stmt::FnDecl {
                name,
                params,
                body,
                is_pub,
                span,
            } => {
                let sh = name == "_";
                let new_body = if params.iter().any(|p| p.name == "_") {
                    body
                } else {
                    Self::replace_placeholders(body, replacement)
                };
                (
                    Stmt::FnDecl {
                        name,
                        params,
                        body: new_body,
                        is_pub,
                        span,
                    },
                    sh,
                )
            }
            Stmt::While { cond, body, span } => (
                Stmt::While {
                    cond: Self::replace_placeholders(cond, replacement),
                    body: Self::replace_placeholders(body, replacement),
                    span,
                },
                false,
            ),
            Stmt::For {
                var,
                iter,
                body,
                span,
            } => {
                let new_iter = Self::replace_placeholders(iter, replacement);
                let new_body = if var == "_" {
                    body
                } else {
                    Self::replace_placeholders(body, replacement)
                };
                (
                    Stmt::For {
                        var,
                        iter: new_iter,
                        body: new_body,
                        span,
                    },
                    false,
                )
            }
            Stmt::ForPair {
                var1,
                var2,
                iter,
                body,
                span,
            } => {
                let new_iter = Self::replace_placeholders(iter, replacement);
                let new_body = if var1 == "_" || var2 == "_" {
                    body
                } else {
                    Self::replace_placeholders(body, replacement)
                };
                (
                    Stmt::ForPair {
                        var1,
                        var2,
                        iter: new_iter,
                        body: new_body,
                        span,
                    },
                    false,
                )
            }
            Stmt::Return { value, span } => (
                Stmt::Return {
                    value: value.map(|v| Self::replace_placeholders(v, replacement)),
                    span,
                },
                false,
            ),
            Stmt::Defer { body, span } => (
                Stmt::Defer {
                    body: Self::replace_placeholders(body, replacement),
                    span,
                },
                false,
            ),
            Stmt::Every {
                interval,
                body,
                span,
            } => (
                Stmt::Every {
                    interval: Self::replace_placeholders(interval, replacement),
                    body: Self::replace_placeholders(body, replacement),
                    span,
                },
                false,
            ),
            Stmt::Expr(e) => (
                Stmt::Expr(Self::replace_placeholders(e, replacement)),
                false,
            ),
            other => (other, false),
        }
    }

    fn replace_placeholders(expr: Expr, replacement: &Expr) -> Expr {
        match expr {
            Expr::Ident(ref name, _) if name == "_" => replacement.clone(),
            Expr::Unary {
                op,
                expr: inner,
                span,
            } => Expr::Unary {
                op,
                expr: Box::new(Self::replace_placeholders(*inner, replacement)),
                span,
            },
            Expr::Throw { value: inner, span } => Expr::Throw {
                value: Box::new(Self::replace_placeholders(*inner, replacement)),
                span,
            },
            Expr::Binary {
                left,
                op,
                right,
                span,
            } => Expr::Binary {
                left: Box::new(Self::replace_placeholders(*left, replacement)),
                op,
                right: Box::new(Self::replace_placeholders(*right, replacement)),
                span,
            },
            Expr::NullCoalesce { left, right, span } => Expr::NullCoalesce {
                left: Box::new(Self::replace_placeholders(*left, replacement)),
                right: Box::new(Self::replace_placeholders(*right, replacement)),
                span,
            },
            Expr::Assign {
                target,
                op,
                value,
                span,
            } => {
                let new_target = match *target {
                    AssignTarget::Var(v) => AssignTarget::Var(v),
                    AssignTarget::Index { object, index } => AssignTarget::Index {
                        object: Box::new(Self::replace_placeholders(*object, replacement)),
                        index: Box::new(Self::replace_placeholders(*index, replacement)),
                    },
                    AssignTarget::Attr { object, name } => AssignTarget::Attr {
                        object: Box::new(Self::replace_placeholders(*object, replacement)),
                        name,
                    },
                };
                Expr::Assign {
                    target: Box::new(new_target),
                    op,
                    value: Box::new(Self::replace_placeholders(*value, replacement)),
                    span,
                }
            }
            Expr::Block { stmts, tail, span } => {
                let mut new_stmts = Vec::with_capacity(stmts.len());
                let mut shadowed = false;
                let mut iter = stmts.into_iter();
                while let Some(s) = iter.next() {
                    let (ns, sh) = Self::replace_placeholders_stmt(s, replacement);
                    new_stmts.push(ns);
                    if sh {
                        shadowed = true;
                        new_stmts.extend(iter);
                        break;
                    }
                }
                let new_tail = if shadowed {
                    tail
                } else {
                    tail.map(|t| Box::new(Self::replace_placeholders(*t, replacement)))
                };
                Expr::Block {
                    stmts: new_stmts,
                    tail: new_tail,
                    span,
                }
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                span,
            } => Expr::If {
                cond: Box::new(Self::replace_placeholders(*cond, replacement)),
                then_branch: Box::new(Self::replace_placeholders(*then_branch, replacement)),
                else_branch: else_branch
                    .map(|e| Box::new(Self::replace_placeholders(*e, replacement))),
                span,
            },
            Expr::Match {
                subject,
                arms,
                span,
            } => Expr::Match {
                subject: Box::new(Self::replace_placeholders(*subject, replacement)),
                arms: arms
                    .into_iter()
                    .map(|a| MatchArm {
                        pattern: a.pattern,
                        guard: a.guard.map(|g| Self::replace_placeholders(g, replacement)),
                        body: Self::replace_placeholders(a.body, replacement),
                        span: a.span,
                    })
                    .collect(),
                span,
            },
            Expr::Fn { params, body, span } => {
                let has_shadow = params.iter().any(|p| p.name == "_");
                let new_params = params
                    .into_iter()
                    .map(|p| FnParam {
                        name: p.name,
                        default: p
                            .default
                            .map(|d| Self::replace_placeholders(d, replacement)),
                        is_variadic: p.is_variadic,
                    })
                    .collect();
                let new_body = if has_shadow {
                    body
                } else {
                    Box::new(Self::replace_placeholders(*body, replacement))
                };
                Expr::Fn {
                    params: new_params,
                    body: new_body,
                    span,
                }
            }
            Expr::List(items, span) => Expr::List(
                items
                    .into_iter()
                    .map(|item| Self::replace_placeholders(item, replacement))
                    .collect(),
                span,
            ),
            Expr::Dict(entries, span) => Expr::Dict(
                entries
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            Self::replace_placeholders(k, replacement),
                            Self::replace_placeholders(v, replacement),
                        )
                    })
                    .collect(),
                span,
            ),
            Expr::Call { callee, args, span } => Expr::Call {
                callee: Box::new(Self::replace_placeholders(*callee, replacement)),
                args: args
                    .into_iter()
                    .map(|a| Self::replace_placeholders(a, replacement))
                    .collect(),
                span,
            },
            Expr::CallNamed(mut c) => {
                c.callee = Box::new(Self::replace_placeholders(*c.callee, replacement));
                c.pos_args = c
                    .pos_args
                    .into_iter()
                    .map(|a| Self::replace_placeholders(a, replacement))
                    .collect();
                c.named_args = c
                    .named_args
                    .into_iter()
                    .map(|(k, v)| (k, Self::replace_placeholders(v, replacement)))
                    .collect();
                Expr::CallNamed(c)
            }
            Expr::StructInit(mut s) => {
                s.target = Box::new(Self::replace_placeholders(*s.target, replacement));
                s.spread = s
                    .spread
                    .map(|sp| Box::new(Self::replace_placeholders(*sp, replacement)));
                s.fields = s
                    .fields
                    .into_iter()
                    .map(|(k, v)| (k, Self::replace_placeholders(v, replacement)))
                    .collect();
                Expr::StructInit(s)
            }
            Expr::ChainedCmp(mut c) => {
                c.first = Box::new(Self::replace_placeholders(*c.first, replacement));
                c.comparisons = c
                    .comparisons
                    .into_iter()
                    .map(|(op, rhs)| (op, Self::replace_placeholders(rhs, replacement)))
                    .collect();
                Expr::ChainedCmp(c)
            }
            Expr::Index {
                object,
                index,
                span,
            } => Expr::Index {
                object: Box::new(Self::replace_placeholders(*object, replacement)),
                index: Box::new(Self::replace_placeholders(*index, replacement)),
                span,
            },
            Expr::Slice {
                object,
                start,
                end,
                inclusive,
                span,
            } => Expr::Slice {
                object: Box::new(Self::replace_placeholders(*object, replacement)),
                start: start.map(|s| Box::new(Self::replace_placeholders(*s, replacement))),
                end: end.map(|e| Box::new(Self::replace_placeholders(*e, replacement))),
                inclusive,
                span,
            },
            Expr::ListComp {
                expr,
                var,
                iter,
                cond,
                span,
            } => {
                let new_iter = Box::new(Self::replace_placeholders(*iter, replacement));
                let (new_expr, new_cond) = if var == "_" {
                    (expr, cond)
                } else {
                    (
                        Box::new(Self::replace_placeholders(*expr, replacement)),
                        cond.map(|c| Box::new(Self::replace_placeholders(*c, replacement))),
                    )
                };
                Expr::ListComp {
                    expr: new_expr,
                    var,
                    iter: new_iter,
                    cond: new_cond,
                    span,
                }
            }
            Expr::GetAttr { object, name, span } => Expr::GetAttr {
                object: Box::new(Self::replace_placeholders(*object, replacement)),
                name,
                span,
            },
            Expr::OptGetAttr { object, name, span } => Expr::OptGetAttr {
                object: Box::new(Self::replace_placeholders(*object, replacement)),
                name,
                span,
            },
            Expr::TryCatch {
                try_body,
                catch_var,
                catch_body,
                span,
            } => Expr::TryCatch {
                try_body: Box::new(Self::replace_placeholders(*try_body, replacement)),
                catch_body: if catch_var.as_deref() == Some("_") {
                    catch_body
                } else {
                    Box::new(Self::replace_placeholders(*catch_body, replacement))
                },
                catch_var,
                span,
            },
            Expr::SafePipe(mut p) => {
                p.left = Box::new(Self::replace_placeholders(*p.left, replacement));
                if p.param != "_" {
                    p.apply = Box::new(Self::replace_placeholders(*p.apply, replacement));
                }
                Expr::SafePipe(p)
            }
            Expr::MapPipe { left, mapper, span } => Expr::MapPipe {
                left: Box::new(Self::replace_placeholders(*left, replacement)),
                mapper: Box::new(Self::replace_placeholders(*mapper, replacement)),
                span,
            },
            Expr::PipeAssign(mut p) => {
                p.target = Box::new(match *p.target {
                    AssignTarget::Var(v) => AssignTarget::Var(v),
                    AssignTarget::Index { object, index } => AssignTarget::Index {
                        object: Box::new(Self::replace_placeholders(*object, replacement)),
                        index: Box::new(Self::replace_placeholders(*index, replacement)),
                    },
                    AssignTarget::Attr { object, name } => AssignTarget::Attr {
                        object: Box::new(Self::replace_placeholders(*object, replacement)),
                        name,
                    },
                });
                if p.param != "_" {
                    p.stages = p
                        .stages
                        .into_iter()
                        .map(|s| Self::replace_placeholders(s, replacement))
                        .collect();
                }
                Expr::PipeAssign(p)
            }
            Expr::TaggedTemplate(mut t) => {
                t.exprs = t
                    .exprs
                    .into_iter()
                    .map(|e| Self::replace_placeholders(e, replacement))
                    .collect();
                Expr::TaggedTemplate(t)
            }
            Expr::With(mut w) => {
                w.resource = Self::replace_placeholders(w.resource, replacement);
                if w.alias.as_deref() != Some("_") {
                    w.body = Self::replace_placeholders(w.body, replacement);
                }
                Expr::With(w)
            }
            Expr::Sandbox(mut s) => {
                s.policy = s
                    .policy
                    .into_iter()
                    .map(|(k, v)| (k, Self::replace_placeholders(v, replacement)))
                    .collect();
                s.body = Self::replace_placeholders(s.body, replacement);
                Expr::Sandbox(s)
            }
            Expr::After { delay, body, span } => Expr::After {
                delay: Box::new(Self::replace_placeholders(*delay, replacement)),
                body: Box::new(Self::replace_placeholders(*body, replacement)),
                span,
            },
            other => other,
        }
    }

    fn apply_pipeline_rhs(lhs: Expr, rhs: Expr) -> Expr {
        let span = rhs.span();
        let ph_count = Self::count_placeholders(&rhs);
        if ph_count == 1 {
            return Self::replace_placeholders(rhs, &lhs);
        }
        if ph_count > 1 {
            let tmp_name = "$pipe_val".to_string();
            let tmp_ident = Expr::Ident(tmp_name.clone(), span);
            let replaced = Self::replace_placeholders(rhs, &tmp_ident);
            return Expr::Block {
                stmts: vec![Stmt::Let {
                    name: tmp_name,
                    name_span: span,
                    value: lhs,
                    is_pub: false,
                    span,
                }],
                tail: Some(Box::new(replaced)),
                span,
            };
        }
        match rhs {
            Expr::Call {
                callee,
                mut args,
                span: call_span,
            } => {
                args.insert(0, lhs);
                Expr::Call {
                    callee,
                    args,
                    span: call_span,
                }
            }
            Expr::CallNamed(mut c) => {
                c.pos_args.insert(0, lhs);
                Expr::CallNamed(c)
            }
            other => Expr::Call {
                callee: Box::new(other),
                args: vec![lhs],
                span,
            },
        }
    }

    fn parse_pipe(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        let mut expr = self.parse_term()?;
        loop {
            let kind = if self.match_kind(&TokenKind::Pipe) {
                0
            } else if self.match_kind(&TokenKind::PipeSafe) {
                1
            } else if self.match_kind(&TokenKind::PipeMap) {
                2
            } else {
                break;
            };
            self.check_chain(start)?;
            let right = self.parse_term()?;
            let span = right.span();
            expr = match kind {
                0 => Self::apply_pipeline_rhs(expr, right),
                1 => {
                    let param = "$sp".to_string();
                    let apply = Self::apply_pipeline_rhs(Expr::Ident(param.clone(), span), right);
                    Expr::SafePipe(Box::new(SafePipeExpr {
                        left: Box::new(expr),
                        param,
                        apply: Box::new(apply),
                        span,
                    }))
                }
                2 => {
                    let needs_wrapper = Self::count_placeholders(&right) > 0
                        || matches!(&right, Expr::Call { .. } | Expr::CallNamed(_));
                    let mapper = if needs_wrapper {
                        let param = "$mp".to_string();
                        let body =
                            Self::apply_pipeline_rhs(Expr::Ident(param.clone(), span), right);
                        Expr::Fn {
                            params: vec![FnParam {
                                name: param,
                                default: None,
                                is_variadic: false,
                            }],
                            body: Box::new(body),
                            span,
                        }
                    } else {
                        right
                    };
                    Expr::MapPipe {
                        left: Box::new(expr),
                        mapper: Box::new(mapper),
                        span,
                    }
                }
                _ => unreachable!(),
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

    fn looks_like_nonempty_struct_literal(&self) -> bool {
        if !matches!(self.peek_kind(), TokenKind::LBrace) {
            return false;
        }
        match self.tokens.get(self.pos + 1).map(|t| &t.kind) {
            Some(TokenKind::DotDot) => true,
            Some(TokenKind::Ident(_)) => matches!(
                self.tokens.get(self.pos + 2).map(|t| &t.kind),
                Some(TokenKind::Colon)
            ),
            _ => false,
        }
    }

    fn looks_like_struct_literal(&self) -> bool {
        if !matches!(self.peek_kind(), TokenKind::LBrace) {
            return false;
        }
        matches!(
            self.tokens.get(self.pos + 1).map(|t| &t.kind),
            Some(TokenKind::RBrace)
        ) || self.looks_like_nonempty_struct_literal()
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
            } else if matches!(expr, Expr::Ident(..) | Expr::GetAttr { .. })
                && ((self.allow_struct_literal && self.looks_like_struct_literal())
                    || self.looks_like_nonempty_struct_literal())
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

    #[inline(never)]
    fn parse_tagged_template(&mut self, tag: String, span: Span) -> Result<Expr, String> {
        match self.peek_kind().clone() {
            TokenKind::Str(s) => {
                self.advance();
                Ok(Expr::TaggedTemplate(Box::new(TaggedTemplateExpr {
                    tag,
                    parts: vec![s],
                    exprs: Vec::new(),
                    expr_labels: Vec::new(),
                    span,
                })))
            }
            TokenKind::InterpStart(first) => {
                self.advance();
                let start = self.pos;
                let mut parts = vec![first];
                let mut exprs = Vec::new();
                let mut expr_labels = Vec::new();
                loop {
                    self.check_chain(start)?;
                    let expr = self.parse_expr()?;
                    let mut label = match &expr {
                        Expr::Ident(name, _) => name.clone(),
                        Expr::GetAttr { name, .. } => name.clone(),
                        _ => String::new(),
                    };
                    let expr_val = if let TokenKind::DebugEq(eq_label) = self.peek_kind().clone() {
                        self.advance();
                        if label.is_empty() {
                            label = eq_label.clone();
                        }
                        let expr_span = expr.span();
                        let str_call = Expr::Call {
                            callee: Box::new(Expr::Ident("str".to_string(), expr_span)),
                            args: vec![expr],
                            span: expr_span,
                        };
                        Expr::Binary {
                            left: Box::new(Expr::Str(format!("{}=", eq_label), expr_span)),
                            op: BinaryOp::Add,
                            right: Box::new(str_call),
                            span: expr_span,
                        }
                    } else {
                        expr
                    };
                    exprs.push(expr_val);
                    expr_labels.push(label);
                    match self.peek_kind().clone() {
                        TokenKind::InterpMid(mid) => {
                            self.advance();
                            parts.push(mid);
                        }
                        TokenKind::InterpEnd(end) => {
                            self.advance();
                            parts.push(end);
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
                Ok(Expr::TaggedTemplate(Box::new(TaggedTemplateExpr {
                    tag,
                    parts,
                    exprs,
                    expr_labels,
                    span,
                })))
            }
            other => Err(format!(
                "[{}:{}] Expected string literal after template tag '{}', got {:?}",
                span.line, span.col, tag, other
            )),
        }
    }

    #[inline(never)]
    fn parse_interp(&mut self, first: String, span: Span) -> Result<Expr, String> {
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
            let seg = if let TokenKind::DebugEq(label) = self.peek_kind().clone() {
                self.advance();
                Expr::Binary {
                    left: Box::new(Expr::Str(format!("{}=", label), expr_span)),
                    op: BinaryOp::Add,
                    right: Box::new(str_call),
                    span: expr_span,
                }
            } else {
                str_call
            };
            acc = Expr::Binary {
                left: Box::new(acc),
                op: BinaryOp::Add,
                right: Box::new(seg),
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

    #[inline(never)]
    fn parse_sandbox_expr(&mut self, span: Span) -> Result<Expr, String> {
        let mut policy = Vec::new();
        if self.match_kind(&TokenKind::LParen) {
            if !self.check(&TokenKind::RParen) {
                loop {
                    let k = self.expect_ident("Expected policy key inside 'sandbox(...)'")?;
                    self.expect(&TokenKind::Colon, "Expected ':' after sandbox policy key")?;
                    let v = self.parse_expr()?;
                    policy.push((k, v));
                    if !self.match_kind(&TokenKind::Comma) {
                        break;
                    }
                    if self.check(&TokenKind::RParen) {
                        break;
                    }
                }
            }
            self.expect(&TokenKind::RParen, "Expected ')' after sandbox options")?;
        }
        let body = self.parse_block()?;
        Ok(Expr::Sandbox(Box::new(SandboxExpr { policy, body, span })))
    }

    #[inline(never)]
    fn parse_with_expr(&mut self, span: Span) -> Result<Expr, String> {
        let resource = self.parse_expr_no_struct()?;
        let alias = if self.match_kind(&TokenKind::As) {
            Some(self.expect_ident("Expected variable name after 'as' in 'with'")?)
        } else {
            None
        };
        let body = self.parse_block()?;
        Ok(Expr::With(Box::new(WithExpr {
            resource,
            alias,
            body,
            span,
        })))
    }

    #[inline(never)]
    fn parse_after_expr(&mut self, span: Span) -> Result<Expr, String> {
        let delay = self.parse_expr_no_struct()?;
        let body = self.parse_block()?;
        Ok(Expr::After {
            delay: Box::new(delay),
            body: Box::new(body),
            span,
        })
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
            TokenKind::TaggedTemplate(tag) => {
                self.advance();
                self.parse_tagged_template(tag, span)
            }
            TokenKind::InterpStart(first) => {
                self.advance();
                self.parse_interp(first, span)
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
                if name == "sandbox" && self.looks_like_sandbox() {
                    self.advance();
                    return self.parse_sandbox_expr(span);
                }
                if name == "with" && self.looks_like_control_block(true) {
                    self.advance();
                    return self.parse_with_expr(span);
                }
                if name == "after" && self.looks_like_control_block(false) {
                    self.advance();
                    return self.parse_after_expr(span);
                }
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
        if self.depth >= MAX_PARSE_DEPTH {
            let span = self.current_span();
            return Err(format!(
                "[{}:{}] Maximum block nesting depth ({}) exceeded",
                span.line, span.col, MAX_PARSE_DEPTH
            ));
        }
        self.depth += 1;
        let res = (|| -> Result<Expr, String> {
            let span = self.current_span();
            self.expect(&TokenKind::LBrace, "Expected '{'")?;
            let prev = self.allow_struct_literal;
            self.allow_struct_literal = true;
            let list_res = self.parse_stmt_list(TokenKind::RBrace);
            self.allow_struct_literal = prev;
            let (stmts, tail) = list_res?;
            self.expect(&TokenKind::RBrace, "Expected '}'")?;
            Ok(Expr::Block { stmts, tail, span })
        })();
        self.depth -= 1;
        res
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
            let mut name_spans = Vec::new();
            if !self.check(&TokenKind::RBracket) {
                loop {
                    let item_span = self.current_span();
                    names.push(self.expect_ident("Expected variable name in list destructuring")?);
                    name_spans.push(item_span);
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
                name_spans,
                value,
                is_pub,
                span,
            })
        } else if self.match_kind(&TokenKind::Hash) {
            self.expect(&TokenKind::LBrace, "Expected '{' after '#'")?;
            let mut fields = Vec::new();
            let mut field_spans = Vec::new();
            if !self.check(&TokenKind::RBrace) {
                loop {
                    let field_span = self.current_span();
                    fields.push(self.expect_ident("Expected field name in dict destructuring")?);
                    field_spans.push(field_span);
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
                field_spans,
                value,
                is_pub,
                span,
            })
        } else {
            let name_span = self.current_span();
            let name = self.expect_ident("Expected variable name after 'let'")?;
            self.expect(&TokenKind::Assign, "Expected '=' after variable name")?;
            let value = self.parse_expr()?;
            self.expect(&TokenKind::Semicolon, "Expected ';' after 'let' statement")?;
            Ok(Stmt::Let {
                name,
                name_span,
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
            | Expr::With(_)
            | Expr::Sandbox(_)
            | Expr::After { .. }
    )
}
