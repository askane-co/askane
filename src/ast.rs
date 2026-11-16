use crate::lexer::Span;

#[derive(Debug, Clone)]
pub struct Program {
    pub stmts: Vec<Stmt>,
    pub tail: Option<Box<Expr>>,
}

#[derive(Debug, Clone)]
pub enum ImportSource {
    Name(String),
    Path(String),
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Import {
        source: ImportSource,
        alias: String,
        span: Span,
    },
    Let {
        name: String,
        value: Expr,
        span: Span,
    },
    FnDecl {
        name: String,
        params: Vec<String>,
        body: Expr,
        span: Span,
    },
    While {
        cond: Expr,
        body: Expr,
        span: Span,
    },
    For {
        var: String,
        iter: Expr,
        body: Expr,
        span: Span,
    },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    Break(Span),
    Continue(Span),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64, Span),
    Float(f64, Span),
    Str(String, Span),
    Bool(bool, Span),
    Unit(Span),
    Ident(String, Span),
    List(Vec<Expr>, Span),
    Dict(Vec<(Expr, Expr)>, Span),
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
        span: Span,
    },
    Binary {
        left: Box<Expr>,
        op: BinaryOp,
        right: Box<Expr>,
        span: Span,
    },
    Assign {
        target: AssignTarget,
        value: Box<Expr>,
        span: Span,
    },
    Block {
        stmts: Vec<Stmt>,
        tail: Option<Box<Expr>>,
        span: Span,
    },
    If {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Option<Box<Expr>>,
        span: Span,
    },
    Match {
        subject: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    Fn {
        params: Vec<String>,
        body: Box<Expr>,
        span: Span,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    GetAttr {
        object: Box<Expr>,
        name: String,
        span: Span,
    },
    OptGetAttr {
        object: Box<Expr>,
        name: String,
        span: Span,
    },
    NullCoalesce {
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Int(_, s)
            | Expr::Float(_, s)
            | Expr::Str(_, s)
            | Expr::Bool(_, s)
            | Expr::Unit(s)
            | Expr::Ident(_, s)
            | Expr::List(_, s)
            | Expr::Dict(_, s) => *s,
            Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Assign { span, .. }
            | Expr::Block { span, .. }
            | Expr::If { span, .. }
            | Expr::Match { span, .. }
            | Expr::Fn { span, .. }
            | Expr::Call { span, .. }
            | Expr::Index { span, .. }
            | Expr::GetAttr { span, .. }
            | Expr::OptGetAttr { span, .. }
            | Expr::NullCoalesce { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AssignTarget {
    Var(String),
    Index { object: Box<Expr>, index: Box<Expr> },
    Attr { object: Box<Expr>, name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard,
    Ident(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    Unit,
}
