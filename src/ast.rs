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
pub struct FnParam {
    pub name: String,
    pub default: Option<Expr>,
    pub is_variadic: bool,
}

#[derive(Debug, Clone)]
pub struct StructField {
    pub name: String,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct CallNamedExpr {
    pub callee: Box<Expr>,
    pub pos_args: Vec<Expr>,
    pub named_args: Vec<(String, Expr)>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StructInitExpr {
    pub target: Box<Expr>,
    pub fields: Vec<(String, Expr)>,
    pub spread: Option<Box<Expr>>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ChainedCmpExpr {
    pub first: Box<Expr>,
    pub comparisons: Vec<(BinaryOp, Expr)>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct SafePipeExpr {
    pub left: Box<Expr>,
    pub param: String,
    pub apply: Box<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct PipeAssignExpr {
    pub target: Box<AssignTarget>,
    pub param: String,
    pub stages: Vec<Expr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TaggedTemplateExpr {
    pub tag: String,
    pub parts: Vec<String>,
    pub exprs: Vec<Expr>,
    pub expr_labels: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct WithExpr {
    pub resource: Expr,
    pub alias: Option<String>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct SandboxExpr {
    pub policy: Vec<(String, Expr)>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StructMethod {
    pub name: String,
    pub params: Vec<FnParam>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ImportItem {
    pub name: String,
    pub alias: String,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Import {
        source: ImportSource,
        alias: String,
        span: Span,
    },
    ImportFrom {
        source: ImportSource,
        items: Vec<ImportItem>,
        span: Span,
    },
    Let {
        name: String,
        name_span: Span,
        value: Expr,
        is_pub: bool,
        span: Span,
    },
    LetList {
        names: Vec<String>,
        name_spans: Vec<Span>,
        value: Expr,
        is_pub: bool,
        span: Span,
    },
    LetDict {
        fields: Vec<String>,
        field_spans: Vec<Span>,
        value: Expr,
        is_pub: bool,
        span: Span,
    },
    FnDecl {
        name: String,
        params: Vec<FnParam>,
        body: Expr,
        is_pub: bool,
        span: Span,
    },
    StructDecl {
        name: String,
        is_pub: bool,
        fields: Vec<StructField>,
        methods: Vec<StructMethod>,
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
    ForPair {
        var1: String,
        var2: String,
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
    Needs {
        caps: Vec<(String, Vec<String>)>,
        span: Span,
    },
    Defer {
        body: Expr,
        span: Span,
    },
    Every {
        interval: Expr,
        body: Expr,
        span: Span,
    },
    Spec {
        name: String,
        body: Expr,
        span: Span,
    },
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64, Span),
    Float(f64, Span),
    Str(String, Span),
    Bytes(Vec<u8>, Span),
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
        target: Box<AssignTarget>,
        op: Option<BinaryOp>,
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
        params: Vec<FnParam>,
        body: Box<Expr>,
        span: Span,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    CallNamed(Box<CallNamedExpr>),
    StructInit(Box<StructInitExpr>),
    ChainedCmp(Box<ChainedCmpExpr>),
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    Slice {
        object: Box<Expr>,
        start: Option<Box<Expr>>,
        end: Option<Box<Expr>>,
        inclusive: bool,
        span: Span,
    },
    ListComp {
        expr: Box<Expr>,
        var: String,
        iter: Box<Expr>,
        cond: Option<Box<Expr>>,
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
    TryCatch {
        try_body: Box<Expr>,
        catch_var: Option<String>,
        catch_body: Box<Expr>,
        span: Span,
    },
    SafePipe(Box<SafePipeExpr>),
    MapPipe {
        left: Box<Expr>,
        mapper: Box<Expr>,
        span: Span,
    },
    PipeAssign(Box<PipeAssignExpr>),
    TaggedTemplate(Box<TaggedTemplateExpr>),
    With(Box<WithExpr>),
    Sandbox(Box<SandboxExpr>),
    After {
        delay: Box<Expr>,
        body: Box<Expr>,
        span: Span,
    },
    Throw {
        value: Box<Expr>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Int(_, s)
            | Expr::Float(_, s)
            | Expr::Str(_, s)
            | Expr::Bytes(_, s)
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
            | Expr::Slice { span, .. }
            | Expr::ListComp { span, .. }
            | Expr::GetAttr { span, .. }
            | Expr::OptGetAttr { span, .. }
            | Expr::NullCoalesce { span, .. }
            | Expr::TryCatch { span, .. }
            | Expr::MapPipe { span, .. }
            | Expr::After { span, .. }
            | Expr::Throw { span, .. } => *span,
            Expr::SafePipe(p) => p.span,
            Expr::PipeAssign(p) => p.span,
            Expr::TaggedTemplate(t) => t.span,
            Expr::With(w) => w.span,
            Expr::Sandbox(s) => s.span,
            Expr::CallNamed(c) => c.span,
            Expr::StructInit(s) => s.span,
            Expr::ChainedCmp(c) => c.span,
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
    In,
    NotIn,
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
    Range {
        start: i64,
        end: i64,
        inclusive: bool,
    },
}
