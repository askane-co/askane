#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    InterpStart(String),
    InterpMid(String),
    InterpEnd(String),
    TaggedTemplate(String),
    DebugEq(String),
    Bool(bool),
    Ident(String),

    Let,
    Fn,
    Struct,
    If,
    Else,
    Match,
    While,
    Loop,
    For,
    In,
    Import,
    From,
    Pub,
    As,
    Return,
    Break,
    Continue,
    Try,
    Catch,
    Throw,
    Recover,
    Unit,

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Dot,
    DotDot,
    DotDotEq,
    Colon,
    Hash,
    Semicolon,

    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Bang,
    Bar,
    Pipe,
    PipeSafe,
    PipeMap,
    PipeAssign,
    DoubleQuestion,
    QuestionDot,
    FatArrow,

    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
    interp_braces: Vec<(usize, usize, Option<usize>)>,
}

impl Lexer {
    pub fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            interp_braces: Vec::new(),
        }
    }

    pub fn tokenize(&mut self) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();

        while let Some(ch) = self.peek() {
            if ch.is_whitespace() {
                self.advance();
                continue;
            }

            if ch == '/' && self.peek_next() == Some('/') {
                while let Some(c) = self.peek() {
                    if c == '\n' {
                        break;
                    }
                    self.advance();
                }
                continue;
            }

            if ch == '/' && self.peek_next() == Some('*') {
                let start = self.span();
                self.advance();
                self.advance();
                let mut closed = false;
                while let Some(c) = self.peek() {
                    if c == '*' && self.peek_next() == Some('/') {
                        self.advance();
                        self.advance();
                        closed = true;
                        break;
                    }
                    self.advance();
                }
                if !closed {
                    return Err(format!(
                        "[{}:{}] Unterminated block comment",
                        start.line, start.col
                    ));
                }
                continue;
            }

            let span = self.span();

            if ch.is_ascii_digit() {
                tokens.push(self.lex_number(span)?);
                continue;
            }

            if ch == 'r' && self.peek_next() == Some('"') {
                self.advance(); // 'r'
                self.advance(); // '"'
                tokens.push(self.lex_raw_string(span)?);
                continue;
            }

            if ch == 'b' && self.peek_next() == Some('"') {
                self.advance(); // 'b'
                self.advance(); // '"'
                tokens.push(self.lex_byte_string(span)?);
                continue;
            }

            if ((ch == 'b' && self.peek_next() == Some('r'))
                || (ch == 'r' && self.peek_next() == Some('b')))
                && self.chars.get(self.pos + 2).copied() == Some('"')
            {
                self.advance();
                self.advance();
                self.advance();
                tokens.push(self.lex_raw_byte_string(span)?);
                continue;
            }

            if ch.is_ascii_alphabetic() || ch == '_' {
                let tok = self.lex_ident_or_keyword(span);
                if let TokenKind::Ident(tag) = &tok.kind {
                    if self.peek() == Some('"')
                        && !(self.peek_next() == Some('"')
                            && self.chars.get(self.pos + 2) == Some(&'"'))
                    {
                        tokens.push(Token {
                            kind: TokenKind::TaggedTemplate(tag.clone()),
                            span,
                        });
                        continue;
                    }
                }
                tokens.push(tok);
                continue;
            }

            if ch == '"' {
                if self.peek_next() == Some('"')
                    && self.chars.get(self.pos + 2).copied() == Some('"')
                {
                    self.advance();
                    self.advance();
                    self.advance();
                    tokens.push(self.lex_multiline_string(span)?);
                } else {
                    self.advance(); // opening quote
                    tokens.push(self.lex_string_part(span, false)?);
                }
                continue;
            }

            let kind = match ch {
                '(' => {
                    self.advance();
                    TokenKind::LParen
                }
                ')' => {
                    self.advance();
                    TokenKind::RParen
                }
                '{' => {
                    self.advance();
                    if let Some(pair) = self.interp_braces.last_mut() {
                        pair.0 += 1;
                    }
                    TokenKind::LBrace
                }
                '}' => {
                    let brace_pos = self.pos;
                    self.advance();
                    if let Some(&(depth, expr_start, last_eq)) = self.interp_braces.last() {
                        if depth == 0 {
                            self.interp_braces.pop();
                            if tokens
                                .last()
                                .is_some_and(|t| matches!(t.kind, TokenKind::Assign))
                            {
                                tokens.pop();
                                let eq_pos = last_eq.unwrap_or(brace_pos);
                                let raw_label: String =
                                    self.chars[expr_start..eq_pos].iter().collect();
                                tokens.push(Token {
                                    kind: TokenKind::DebugEq(raw_label.trim().to_string()),
                                    span,
                                });
                            }
                            tokens.push(self.lex_string_part(span, true)?);
                            continue;
                        } else if let Some(pair) = self.interp_braces.last_mut() {
                            pair.0 -= 1;
                        }
                    }
                    TokenKind::RBrace
                }
                '[' => {
                    self.advance();
                    TokenKind::LBracket
                }
                ']' => {
                    self.advance();
                    TokenKind::RBracket
                }
                ',' => {
                    self.advance();
                    TokenKind::Comma
                }
                '.' => {
                    self.advance();
                    if self.peek() == Some('.') {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            TokenKind::DotDotEq
                        } else {
                            TokenKind::DotDot
                        }
                    } else {
                        TokenKind::Dot
                    }
                }
                ':' => {
                    self.advance();
                    TokenKind::Colon
                }
                '#' => {
                    self.advance();
                    TokenKind::Hash
                }
                ';' => {
                    self.advance();
                    TokenKind::Semicolon
                }
                '+' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        TokenKind::PlusAssign
                    } else {
                        TokenKind::Plus
                    }
                }
                '-' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        TokenKind::MinusAssign
                    } else {
                        TokenKind::Minus
                    }
                }
                '*' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        TokenKind::StarAssign
                    } else {
                        TokenKind::Star
                    }
                }
                '/' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        TokenKind::SlashAssign
                    } else {
                        TokenKind::Slash
                    }
                }
                '%' => {
                    self.advance();
                    TokenKind::Percent
                }
                '=' => {
                    let eq_idx = self.pos;
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        TokenKind::Eq
                    } else if self.peek() == Some('>') {
                        self.advance();
                        TokenKind::FatArrow
                    } else {
                        if let Some((depth, _, last_eq)) = self.interp_braces.last_mut() {
                            if *depth == 0 {
                                *last_eq = Some(eq_idx);
                            }
                        }
                        TokenKind::Assign
                    }
                }
                '!' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        TokenKind::Ne
                    } else {
                        TokenKind::Bang
                    }
                }
                '<' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        TokenKind::Le
                    } else {
                        TokenKind::Lt
                    }
                }
                '>' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        TokenKind::Ge
                    } else {
                        TokenKind::Gt
                    }
                }
                '&' => {
                    self.advance();
                    if self.peek() == Some('&') {
                        self.advance();
                        TokenKind::And
                    } else {
                        return Err(format!(
                            "[{}:{}] Unexpected character '&' (did you mean '&&'?)",
                            span.line, span.col
                        ));
                    }
                }
                '|' => {
                    self.advance();
                    if self.peek() == Some('|') {
                        self.advance();
                        TokenKind::Or
                    } else if self.peek() == Some('>') {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            TokenKind::PipeAssign
                        } else {
                            TokenKind::Pipe
                        }
                    } else if self.peek() == Some('?') && self.peek_next() == Some('>') {
                        self.advance();
                        self.advance();
                        TokenKind::PipeSafe
                    } else if self.peek() == Some('*') && self.peek_next() == Some('>') {
                        self.advance();
                        self.advance();
                        TokenKind::PipeMap
                    } else {
                        TokenKind::Bar
                    }
                }
                '?' => {
                    self.advance();
                    if self.peek() == Some('?') {
                        self.advance();
                        TokenKind::DoubleQuestion
                    } else if self.peek() == Some('.') {
                        self.advance();
                        TokenKind::QuestionDot
                    } else {
                        return Err(format!(
                            "[{}:{}] Unexpected character '?' (did you mean '??' or '?.'?)",
                            span.line, span.col
                        ));
                    }
                }
                other => {
                    return Err(format!(
                        "[{}:{}] Unexpected character '{}'",
                        span.line, span.col, other
                    ));
                }
            };

            tokens.push(Token { kind, span });
        }

        tokens.push(Token {
            kind: TokenKind::Eof,
            span: self.span(),
        });

        Ok(tokens)
    }

    fn lex_number(&mut self, span: Span) -> Result<Token, String> {
        let mut raw = String::new();
        let mut is_float = false;

        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                raw.push(c);
                self.advance();
            } else if c == '_' {
                self.advance();
            } else if c == '.' && !is_float && self.peek_next().is_some_and(|n| n.is_ascii_digit())
            {
                is_float = true;
                raw.push(c);
                self.advance();
            } else {
                break;
            }
        }

        let dur_factor = match (
            self.peek(),
            self.peek_next(),
            self.chars.get(self.pos + 2).copied(),
        ) {
            (Some('m'), Some('s'), next)
                if !next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                self.advance();
                self.advance();
                Some(1.0_f64)
            }
            (Some('s'), next, _)
                if !next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                self.advance();
                Some(1_000.0_f64)
            }
            (Some('m'), next, _)
                if !next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                self.advance();
                Some(60_000.0_f64)
            }
            (Some('h'), next, _)
                if !next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                self.advance();
                Some(3_600_000.0_f64)
            }
            _ => None,
        };

        let kind = if let Some(factor) = dur_factor {
            if !is_float {
                let factor_u128 = factor as u128;
                let base_u128 = raw.parse::<u128>().map_err(|_| {
                    format!(
                        "[{}:{}] Invalid duration literal '{}'",
                        span.line, span.col, raw
                    )
                })?;
                let ms = base_u128
                    .checked_mul(factor_u128)
                    .filter(|&v| v <= i64::MAX as u128)
                    .ok_or_else(|| {
                        format!("[{}:{}] Duration literal out of range", span.line, span.col)
                    })?;
                TokenKind::Int(ms as i64)
            } else {
                let base: f64 = raw.parse().map_err(|_| {
                    format!(
                        "[{}:{}] Invalid duration literal '{}'",
                        span.line, span.col, raw
                    )
                })?;
                let ms = (base * factor).round();
                if !ms.is_finite() || ms < 0.0 || ms >= 9223372036854775808.0 {
                    return Err(format!(
                        "[{}:{}] Duration literal out of range",
                        span.line, span.col
                    ));
                }
                TokenKind::Int(ms as i64)
            }
        } else if is_float {
            let val: f64 = raw.parse().map_err(|_| {
                format!(
                    "[{}:{}] Invalid float literal '{}'",
                    span.line, span.col, raw
                )
            })?;
            TokenKind::Float(val)
        } else {
            let val: i64 = raw.parse().map_err(|_| {
                format!(
                    "[{}:{}] Invalid integer literal '{}'",
                    span.line, span.col, raw
                )
            })?;
            TokenKind::Int(val)
        };

        Ok(Token { kind, span })
    }

    fn lex_ident_or_keyword(&mut self, span: Span) -> Token {
        let mut ident = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                ident.push(c);
                self.advance();
            } else {
                break;
            }
        }

        let kind = match ident.as_str() {
            "let" => TokenKind::Let,
            "fn" => TokenKind::Fn,
            "struct" => TokenKind::Struct,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "match" => TokenKind::Match,
            "while" => TokenKind::While,
            "loop" => TokenKind::Loop,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "import" => TokenKind::Import,
            "from" => TokenKind::From,
            "pub" => TokenKind::Pub,
            "as" => TokenKind::As,
            "return" => TokenKind::Return,
            "break" => TokenKind::Break,
            "continue" => TokenKind::Continue,
            "try" => TokenKind::Try,
            "catch" => TokenKind::Catch,
            "throw" => TokenKind::Throw,
            "recover" => TokenKind::Recover,
            "true" => TokenKind::Bool(true),
            "false" => TokenKind::Bool(false),
            "unit" => TokenKind::Unit,
            _ => TokenKind::Ident(ident),
        };

        Token { kind, span }
    }

    fn lex_string_part(&mut self, span: Span, in_interp: bool) -> Result<Token, String> {
        let mut out = String::new();

        while let Some(c) = self.peek() {
            if c == '"' {
                self.advance();
                return Ok(Token {
                    kind: if in_interp {
                        TokenKind::InterpEnd(out)
                    } else {
                        TokenKind::Str(out)
                    },
                    span,
                });
            }
            if c == '$' && self.peek_next() == Some('{') {
                self.advance();
                self.advance();
                self.interp_braces.push((0, self.pos, None));
                return Ok(Token {
                    kind: if in_interp {
                        TokenKind::InterpMid(out)
                    } else {
                        TokenKind::InterpStart(out)
                    },
                    span,
                });
            }
            if c == '\\' {
                self.advance();
                let esc = self.peek().ok_or_else(|| {
                    format!("[{}:{}] Unterminated escape in string", span.line, span.col)
                })?;
                self.advance();
                match esc {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    '$' => out.push('$'),
                    '0' => out.push('\0'),
                    other => {
                        return Err(format!(
                            "[{}:{}] Unknown escape sequence '\\{}'",
                            span.line, span.col, other
                        ));
                    }
                }
            } else {
                out.push(c);
                self.advance();
            }
        }

        Err(format!(
            "[{}:{}] Unterminated string literal",
            span.line, span.col
        ))
    }

    fn lex_raw_string(&mut self, span: Span) -> Result<Token, String> {
        let mut out = String::new();
        while let Some(c) = self.peek() {
            self.advance();
            if c == '"' {
                return Ok(Token {
                    kind: TokenKind::Str(out),
                    span,
                });
            }
            out.push(c);
        }
        Err(format!(
            "[{}:{}] Unterminated raw string literal",
            span.line, span.col
        ))
    }

    fn lex_multiline_string(&mut self, span: Span) -> Result<Token, String> {
        if self.peek() == Some('\r') && self.peek_next() == Some('\n') {
            self.advance();
            self.advance();
        } else if self.peek() == Some('\n') {
            self.advance();
        }
        let mut out = String::new();
        while let Some(c) = self.peek() {
            if c == '"'
                && self.peek_next() == Some('"')
                && self.chars.get(self.pos + 2).copied() == Some('"')
            {
                self.advance();
                self.advance();
                self.advance();
                return Ok(Token {
                    kind: TokenKind::Str(out),
                    span,
                });
            }
            self.advance();
            out.push(c);
        }
        Err(format!(
            "[{}:{}] Unterminated multiline string literal",
            span.line, span.col
        ))
    }

    fn lex_byte_string(&mut self, span: Span) -> Result<Token, String> {
        let mut out = Vec::new();
        while let Some(c) = self.peek() {
            if c == '"' {
                self.advance();
                return Ok(Token {
                    kind: TokenKind::Bytes(out),
                    span,
                });
            }
            if c == '\\' {
                self.advance();
                let esc = self.peek().ok_or_else(|| {
                    format!(
                        "[{}:{}] Unterminated escape in byte literal",
                        span.line, span.col
                    )
                })?;
                self.advance();
                match esc {
                    'n' => out.push(b'\n'),
                    'r' => out.push(b'\r'),
                    't' => out.push(b'\t'),
                    '\\' => out.push(b'\\'),
                    '"' => out.push(b'"'),
                    '0' => out.push(0),
                    'x' => {
                        let h1 = self.advance().ok_or_else(|| {
                            format!(
                                "[{}:{}] Incomplete \\xHH escape in byte literal",
                                span.line, span.col
                            )
                        })?;
                        let h2 = self.advance().ok_or_else(|| {
                            format!(
                                "[{}:{}] Incomplete \\xHH escape in byte literal",
                                span.line, span.col
                            )
                        })?;
                        let hex = [h1, h2].iter().collect::<String>();
                        if !h1.is_ascii_hexdigit() || !h2.is_ascii_hexdigit() {
                            return Err(format!(
                                "[{}:{}] Invalid hex escape '\\x{}' in byte literal",
                                span.line, span.col, hex
                            ));
                        }
                        let val = u8::from_str_radix(&hex, 16).map_err(|_| {
                            format!(
                                "[{}:{}] Invalid hex escape '\\x{}' in byte literal",
                                span.line, span.col, hex
                            )
                        })?;
                        out.push(val);
                    }
                    other => {
                        return Err(format!(
                            "[{}:{}] Unknown escape sequence '\\{}' in byte literal",
                            span.line, span.col, other
                        ));
                    }
                }
            } else if c.is_ascii() {
                self.advance();
                out.push(c as u8);
            } else {
                return Err(format!(
                    "[{}:{}] Byte literal can only contain ASCII characters (use \\xNN for raw bytes)",
                    span.line, span.col
                ));
            }
        }
        Err(format!(
            "[{}:{}] Unterminated byte literal",
            span.line, span.col
        ))
    }

    fn lex_raw_byte_string(&mut self, span: Span) -> Result<Token, String> {
        let mut out = Vec::new();
        while let Some(c) = self.peek() {
            self.advance();
            if c == '"' {
                return Ok(Token {
                    kind: TokenKind::Bytes(out),
                    span,
                });
            }
            if !c.is_ascii() {
                return Err(format!(
                    "[{}:{}] Raw byte literal can only contain ASCII characters",
                    span.line, span.col
                ));
            }
            out.push(c as u8);
        }
        Err(format!(
            "[{}:{}] Unterminated raw byte literal",
            span.line, span.col
        ))
    }

    fn span(&self) -> Span {
        Span {
            line: self.line,
            col: self.col,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.chars.get(self.pos).copied()?;
        self.pos += 1;
        if ch == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }
}
