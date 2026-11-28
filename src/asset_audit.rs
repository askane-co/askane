#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::{AssignTarget, Expr, ImportSource, Program, Stmt};
use crate::bundle::logical_normalize_rel;
use crate::diag::Diagnostic;

#[derive(Clone, Debug, Default)]
pub struct AssetAuditReport {
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
    pub static_assets: Vec<String>,
    pub dynamic_calls: usize,
}

fn asset_arg_indices(mod_name: &str, fn_name: &str) -> Option<&'static [usize]> {
    match (mod_name, fn_name) {
        (
            "assets",
            "load_image" | "load_texture" | "load_sound" | "read_bytes" | "read_text" | "exists"
            | "mount",
        ) => Some(&[0]),
        ("gfx", "load_image" | "font_load_bmfont") => Some(&[0]),
        ("audio", "play" | "sound" | "stream") => Some(&[0]),
        ("mesh", "load") => Some(&[0]),
        ("tiled", "load") => Some(&[0]),
        ("terrain", "load") => Some(&[0]),
        ("postfx", "lut_load") => Some(&[0]),
        ("cubemap", "load") => Some(&[0, 1, 2, 3, 4, 5]),
        _ => None,
    }
}

fn resolve_audit_search_roots(source_file: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let Some(sf) = source_file else {
        return roots;
    };
    let start_dir = match sf.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    roots.push(start_dir.clone());
    let mut cur = std::fs::canonicalize(&start_dir).unwrap_or(start_dir);
    for _ in 0..8 {
        if cur.join("askane.toml").is_file() {
            if !roots.contains(&cur) {
                roots.push(cur.clone());
            }
            break;
        }
        let Some(parent) = cur.parent() else {
            break;
        };
        cur = parent.to_path_buf();
    }
    roots
}

fn asset_exists_on_disk(roots: &[PathBuf], norm_rel: &str) -> bool {
    roots.iter().any(|r| r.join(norm_rel).is_file())
}

struct AssetAuditor<'a> {
    file_label: Option<&'a str>,
    roots: Vec<PathBuf>,
    has_roots: bool,
    mod_aliases: HashMap<String, String>,
    fn_aliases: HashMap<String, (&'static str, &'static str)>,
    seen_static: HashSet<String>,
    report: AssetAuditReport,
}

impl<'a> AssetAuditor<'a> {
    fn new(file_path: Option<&'a Path>) -> Self {
        let roots = resolve_audit_search_roots(file_path);
        let has_roots = !roots.is_empty();
        Self {
            file_label: file_path.and_then(|p| p.to_str()),
            roots,
            has_roots,
            mod_aliases: HashMap::new(),
            fn_aliases: HashMap::new(),
            seen_static: HashSet::new(),
            report: AssetAuditReport::default(),
        }
    }

    fn preregister_imports(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            match s {
                Stmt::Import {
                    source: ImportSource::Name(name),
                    alias,
                    ..
                } => {
                    self.mod_aliases.insert(alias.clone(), name.clone());
                }
                Stmt::ImportFrom {
                    source: ImportSource::Name(name),
                    items,
                    ..
                } => {
                    for it in items {
                        if let Some((m, f)) = static_pair(name, &it.name) {
                            self.fn_aliases.insert(it.alias.clone(), (m, f));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn check_arg(&mut self, mod_name: &str, fn_name: &str, arg: &Expr) {
        match arg {
            Expr::Str(lit, sp) => match logical_normalize_rel(lit) {
                Ok(norm) => {
                    if self.seen_static.insert(norm.clone()) {
                        self.report.static_assets.push(norm.clone());
                    }
                    if self.has_roots && !asset_exists_on_disk(&self.roots, &norm) {
                        let end_col = sp.col.saturating_add(lit.chars().count().saturating_add(1));
                        self.report.warnings.push(Diagnostic {
                            code: "W0502",
                            severity: "warning",
                            file: self.file_label.map(|s| s.to_string()),
                            line: sp.line,
                            col: sp.col,
                            end_col,
                            message: format!(
                                "Static asset '{}' referenced by {}.{}() was not found on disk",
                                norm, mod_name, fn_name
                            ),
                            hint: Some(
                                "Ensure the asset exists relative to the project or inside your ASKANE-PAK bundle."
                                    .to_string(),
                            ),
                            import_chain: Vec::new(),
                            stack_trace: Vec::new(),
                        });
                    }
                }
                Err(reason) => {
                    let end_col = sp.col.saturating_add(lit.chars().count().saturating_add(1));
                    self.report.errors.push(Diagnostic {
                        code: "E0565",
                        severity: "error",
                        file: self.file_label.map(|s| s.to_string()),
                        line: sp.line,
                        col: sp.col,
                        end_col,
                        message: format!(
                            "E_ASSET_STATIC: invalid asset path {:?} in {}.{}(): {}",
                            lit, mod_name, fn_name, reason
                        ),
                        hint: Some(
                            "Use a relative forward-slash path inside the project root without '..', drive letters, or reserved names."
                                .to_string(),
                        ),
                        import_chain: Vec::new(),
                        stack_trace: Vec::new(),
                    });
                }
            },
            other => {
                let sp = other.span();
                self.report.dynamic_calls += 1;
                self.report.warnings.push(Diagnostic {
                    code: "W0501",
                    severity: "warning",
                    file: self.file_label.map(|s| s.to_string()),
                    line: sp.line,
                    col: sp.col,
                    end_col: sp.col,
                    message: format!(
                        "Dynamic asset path passed to {}.{}() cannot be statically audited or auto-packed",
                        mod_name, fn_name
                    ),
                    hint: Some(
                        "Use a string literal asset path so `askane check` and `askane bundle` can verify it."
                            .to_string(),
                    ),
                    import_chain: Vec::new(),
                    stack_trace: Vec::new(),
                });
            }
        }
    }

    fn resolve_callee(&self, callee: &Expr) -> Option<(String, String)> {
        match callee {
            Expr::GetAttr { object, name, .. } => {
                if let Expr::Ident(alias, _) = object.as_ref() {
                    if let Some(mod_name) = self.mod_aliases.get(alias) {
                        return Some((mod_name.clone(), name.clone()));
                    }
                }
                None
            }
            Expr::Ident(fn_alias, _) => self
                .fn_aliases
                .get(fn_alias)
                .map(|&(m, f)| (m.to_string(), f.to_string())),
            _ => None,
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Import {
                source: ImportSource::Name(name),
                alias,
                ..
            } => {
                self.mod_aliases.insert(alias.clone(), name.clone());
            }
            Stmt::ImportFrom {
                source: ImportSource::Name(name),
                items,
                ..
            } => {
                for it in items {
                    if let Some((m, f)) = static_pair(name, &it.name) {
                        self.fn_aliases.insert(it.alias.clone(), (m, f));
                    }
                }
            }
            Stmt::Import { .. } | Stmt::ImportFrom { .. } => {}
            Stmt::Let { value, .. }
            | Stmt::LetList { value, .. }
            | Stmt::LetDict { value, .. } => self.visit_expr(value),
            Stmt::FnDecl { params, body, .. } => {
                for p in params {
                    if let Some(d) = &p.default {
                        self.visit_expr(d);
                    }
                }
                self.visit_expr(body);
            }
            Stmt::StructDecl {
                fields, methods, ..
            } => {
                for f in fields {
                    if let Some(d) = &f.default {
                        self.visit_expr(d);
                    }
                }
                for m in methods {
                    for p in &m.params {
                        if let Some(d) = &p.default {
                            self.visit_expr(d);
                        }
                    }
                    self.visit_expr(&m.body);
                }
            }
            Stmt::While { cond, body, .. } => {
                self.visit_expr(cond);
                self.visit_expr(body);
            }
            Stmt::For { iter, body, .. } | Stmt::ForPair { iter, body, .. } => {
                self.visit_expr(iter);
                self.visit_expr(body);
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    self.visit_expr(v);
                }
            }
            Stmt::Break(_) | Stmt::Continue(_) | Stmt::Needs { .. } => {}
            Stmt::Defer { body, .. } | Stmt::Spec { body, .. } => self.visit_expr(body),
            Stmt::Every { interval, body, .. } => {
                self.visit_expr(interval);
                self.visit_expr(body);
            }
            Stmt::Expr(e) => self.visit_expr(e),
        }
    }

    fn visit_assign_target(&mut self, target: &AssignTarget) {
        match target {
            AssignTarget::Var(_) => {}
            AssignTarget::Index { object, index } => {
                self.visit_expr(object);
                self.visit_expr(index);
            }
            AssignTarget::Attr { object, .. } => self.visit_expr(object),
        }
    }

    fn visit_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Int(..)
            | Expr::Float(..)
            | Expr::Str(..)
            | Expr::Bytes(..)
            | Expr::Bool(..)
            | Expr::Unit(..)
            | Expr::Ident(..) => {}
            Expr::List(items, _) => {
                for e in items {
                    self.visit_expr(e);
                }
            }
            Expr::Dict(pairs, _) => {
                for (k, v) in pairs {
                    self.visit_expr(k);
                    self.visit_expr(v);
                }
            }
            Expr::Unary { expr, .. } => self.visit_expr(expr),
            Expr::Binary { left, right, .. } | Expr::NullCoalesce { left, right, .. } => {
                self.visit_expr(left);
                self.visit_expr(right);
            }
            Expr::Assign { target, value, .. } => {
                self.visit_assign_target(target);
                self.visit_expr(value);
            }
            Expr::Block { stmts, tail, .. } => {
                for s in stmts {
                    self.visit_stmt(s);
                }
                if let Some(t) = tail {
                    self.visit_expr(t);
                }
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                self.visit_expr(cond);
                self.visit_expr(then_branch);
                if let Some(el) = else_branch {
                    self.visit_expr(el);
                }
            }
            Expr::Match { subject, arms, .. } => {
                self.visit_expr(subject);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        self.visit_expr(g);
                    }
                    self.visit_expr(&arm.body);
                }
            }
            Expr::Fn { params, body, .. } => {
                for p in params {
                    if let Some(d) = &p.default {
                        self.visit_expr(d);
                    }
                }
                self.visit_expr(body);
            }
            Expr::Call { callee, args, .. } => {
                self.visit_expr(callee);
                if let Some((m, f)) = self.resolve_callee(callee) {
                    if let Some(indices) = asset_arg_indices(&m, &f) {
                        for &idx in indices {
                            if let Some(arg) = args.get(idx) {
                                self.check_arg(&m, &f, arg);
                            }
                        }
                    }
                }
                for a in args {
                    self.visit_expr(a);
                }
            }
            Expr::CallNamed(cn) => {
                self.visit_expr(&cn.callee);
                if let Some((m, f)) = self.resolve_callee(&cn.callee) {
                    if let Some(indices) = asset_arg_indices(&m, &f) {
                        for &idx in indices {
                            if let Some(arg) = cn.pos_args.get(idx) {
                                self.check_arg(&m, &f, arg);
                            }
                        }
                        if let Some((_, narg)) = cn
                            .named_args
                            .iter()
                            .find(|(k, _)| matches!(k.as_str(), "path" | "file" | "fnt_path"))
                        {
                            self.check_arg(&m, &f, narg);
                        }
                    }
                }
                for a in &cn.pos_args {
                    self.visit_expr(a);
                }
                for (_, a) in &cn.named_args {
                    self.visit_expr(a);
                }
            }
            Expr::StructInit(si) => {
                self.visit_expr(&si.target);
                for (_, e) in &si.fields {
                    self.visit_expr(e);
                }
                if let Some(sp) = &si.spread {
                    self.visit_expr(sp);
                }
            }
            Expr::ChainedCmp(cc) => {
                self.visit_expr(&cc.first);
                for (_, rhs) in &cc.comparisons {
                    self.visit_expr(rhs);
                }
            }
            Expr::Index { object, index, .. } => {
                self.visit_expr(object);
                self.visit_expr(index);
            }
            Expr::Slice {
                object, start, end, ..
            } => {
                self.visit_expr(object);
                if let Some(s) = start {
                    self.visit_expr(s);
                }
                if let Some(e) = end {
                    self.visit_expr(e);
                }
            }
            Expr::ListComp {
                expr, iter, cond, ..
            } => {
                self.visit_expr(iter);
                if let Some(c) = cond {
                    self.visit_expr(c);
                }
                self.visit_expr(expr);
            }
            Expr::GetAttr { object, .. } | Expr::OptGetAttr { object, .. } => {
                self.visit_expr(object);
            }
            Expr::TryCatch {
                try_body,
                catch_body,
                ..
            } => {
                self.visit_expr(try_body);
                self.visit_expr(catch_body);
            }
            Expr::SafePipe(sp) => {
                self.visit_expr(&sp.left);
                self.visit_expr(&sp.apply);
            }
            Expr::MapPipe { left, mapper, .. } => {
                self.visit_expr(left);
                self.visit_expr(mapper);
            }
            Expr::PipeAssign(pa) => {
                self.visit_assign_target(&pa.target);
                for s in &pa.stages {
                    self.visit_expr(s);
                }
            }
            Expr::TaggedTemplate(tt) => {
                for e in &tt.exprs {
                    self.visit_expr(e);
                }
            }
            Expr::With(w) => {
                self.visit_expr(&w.resource);
                self.visit_expr(&w.body);
            }
            Expr::Sandbox(sb) => {
                for (_, e) in &sb.policy {
                    self.visit_expr(e);
                }
                self.visit_expr(&sb.body);
            }
            Expr::After { delay, body, .. } => {
                self.visit_expr(delay);
                self.visit_expr(body);
            }
            Expr::Throw { value, .. } => self.visit_expr(value),
        }
    }
}

fn static_pair(mod_name: &str, fn_name: &str) -> Option<(&'static str, &'static str)> {
    match (mod_name, fn_name) {
        ("assets", "load_image") => Some(("assets", "load_image")),
        ("assets", "load_texture") => Some(("assets", "load_texture")),
        ("assets", "load_sound") => Some(("assets", "load_sound")),
        ("assets", "read_bytes") => Some(("assets", "read_bytes")),
        ("assets", "read_text") => Some(("assets", "read_text")),
        ("assets", "exists") => Some(("assets", "exists")),
        ("assets", "mount") => Some(("assets", "mount")),
        ("gfx", "load_image") => Some(("gfx", "load_image")),
        ("gfx", "font_load_bmfont") => Some(("gfx", "font_load_bmfont")),
        ("audio", "play") => Some(("audio", "play")),
        ("audio", "sound") => Some(("audio", "sound")),
        ("audio", "stream") => Some(("audio", "stream")),
        ("mesh", "load") => Some(("mesh", "load")),
        ("tiled", "load") => Some(("tiled", "load")),
        ("terrain", "load") => Some(("terrain", "load")),
        ("postfx", "lut_load") => Some(("postfx", "lut_load")),
        ("cubemap", "load") => Some(("cubemap", "load")),
        _ => None,
    }
}

pub fn audit_program(program: &Program, source_file: Option<&Path>) -> AssetAuditReport {
    let mut auditor = AssetAuditor::new(source_file);
    auditor.preregister_imports(&program.stmts);
    for s in &program.stmts {
        auditor.visit_stmt(s);
    }
    if let Some(tail) = &program.tail {
        auditor.visit_expr(tail);
    }
    auditor.report
}
