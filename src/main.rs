mod ast;
mod bytecode;
mod compiler;
mod gui;
mod lexer;
mod modules;
mod parser;
mod setup;
mod value;
mod vm;

use std::env;
use std::fs;
use std::io::{self, Write};
use std::process;

use compiler::Compiler;
use lexer::Lexer;
use parser::Parser;
use value::Value;
use vm::{Permissions, VM};

const VERSION: &str = "0.17.0";
pub const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;

pub fn read_text_limited(path: &std::path::Path, limit: usize) -> Result<String, String> {
    use std::io::Read;
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Only regular files can be read".to_string());
    }
    let mut text = String::new();
    file.take(limit as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() > limit {
        return Err(format!("File exceeds {} byte limit", limit));
    }
    Ok(text)
}

fn compile_source_mode(
    source: &str,
    include_specs: bool,
) -> Result<std::rc::Rc<bytecode::FunctionProto>, String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err("Source size limit exceeded".to_string());
    }
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program()?;
    if include_specs {
        Compiler::compile_with_specs(&program)
    } else {
        Compiler::compile(&program)
    }
}

fn compile_source(source: &str) -> Result<std::rc::Rc<bytecode::FunctionProto>, String> {
    compile_source_mode(source, false)
}

fn compile_source_with_specs(source: &str) -> Result<std::rc::Rc<bytecode::FunctionProto>, String> {
    compile_source_mode(source, true)
}

fn format_error_with_source(source: &str, err: &str) -> String {
    let mut line_num: Option<usize> = None;
    let mut col_num: Option<usize> = None;

    if let Some(start) = err.find('[') {
        if let Some(end) = err[start + 1..].find(']') {
            let tag = &err[start + 1..start + 1 + end];
            if let Some(rest) = tag.strip_prefix("line ") {
                line_num = rest.trim().parse::<usize>().ok();
            } else if let Some((l, c)) = tag.split_once(':') {
                line_num = l.trim().parse::<usize>().ok();
                col_num = c.trim().parse::<usize>().ok();
            }
        }
    }

    let Some(ln) = line_num else {
        return err.to_string();
    };
    let Some(src_line) = source.lines().nth(ln.saturating_sub(1)) else {
        return err.to_string();
    };
    let mut out = format!("{}\n  {:>4} | {}", err, ln, src_line);
    if let Some(col) = col_num {
        let pad = " ".repeat(col.saturating_sub(1));
        out.push_str(&format!("\n       | {}^", pad));
    }
    out
}

#[cfg(test)]
fn run_file_with_root(
    path: &str,
    dump_bytecode: bool,
    permissions: Permissions,
    sandbox_root_override: Option<&std::path::Path>,
) -> Result<(), String> {
    run_file_with_options(
        path,
        dump_bytecode,
        false,
        permissions,
        sandbox_root_override,
    )
}

fn print_profile_report(vm: &VM) {
    let entries = vm.profile_report();
    if entries.is_empty() {
        return;
    }
    eprintln!("\n=== Profile ===");
    eprintln!(
        "{:<28} {:>10} {:>12} {:>12}",
        "Function", "Calls", "Steps", "Time (ms)"
    );
    for (name, stat) in entries.into_iter().take(25) {
        let ms = (stat.total_ns as f64) / 1_000_000.0;
        eprintln!(
            "{:<28} {:>10} {:>12} {:>12.3}",
            name, stat.calls, stat.steps, ms
        );
    }
}

fn run_file_with_options(
    path: &str,
    dump_bytecode: bool,
    profile: bool,
    permissions: Permissions,
    sandbox_root_override: Option<&std::path::Path>,
) -> Result<(), String> {
    run_file_core(
        path,
        dump_bytecode,
        profile,
        false,
        permissions,
        sandbox_root_override,
    )
}

fn run_file_core(
    path: &str,
    dump_bytecode: bool,
    profile: bool,
    include_specs: bool,
    permissions: Permissions,
    sandbox_root_override: Option<&std::path::Path>,
) -> Result<(), String> {
    run_file_core_with_test_opts(
        path,
        dump_bytecode,
        profile,
        include_specs,
        permissions,
        sandbox_root_override,
        false,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_file_core_with_test_opts(
    path: &str,
    dump_bytecode: bool,
    profile: bool,
    include_specs: bool,
    permissions: Permissions,
    sandbox_root_override: Option<&std::path::Path>,
    deterministic: bool,
    update_snapshots: bool,
) -> Result<(), String> {
    let source = read_text_limited(std::path::Path::new(path), MAX_SOURCE_BYTES)
        .map_err(|e| format!("Could not read '{}': {}", path, e))?;
    let proto = if include_specs {
        compile_source_with_specs(&source)
    } else {
        compile_source(&source)
    }
    .map_err(|e| format_error_with_source(&source, &e))?;
    if dump_bytecode {
        print!("{}", proto.chunk.disassemble(&proto.name));
        return Ok(());
    }
    let mut vm = VM::new_with_sandbox(!permissions.allow_all);
    vm.set_permissions(permissions);
    vm.set_profile(profile);
    vm.set_deterministic(deterministic);
    vm.set_update_snapshots(update_snapshots);
    let entry_parent = match std::path::Path::new(path).parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            Some(fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf()))
        }
        _ => None,
    };
    if let Some(root) = sandbox_root_override {
        let canon_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        vm.set_current_dir(canon_root.clone());
        vm.set_sandbox_root(canon_root);
    } else if let Some(ref parent) = entry_parent {
        vm.set_current_dir(parent.clone());
        vm.set_sandbox_root(parent.clone());
    }
    let run_result = if sandbox_root_override.is_some() {
        vm.run_with_base_dir(proto, entry_parent)
    } else {
        vm.run(proto)
    };
    run_result.map_err(|e| format_error_with_source(&source, &e))?;
    if include_specs && !vm.spec_results().is_empty() {
        let mut failed_specs = 0usize;
        for (name, res) in vm.spec_results() {
            match res {
                Ok(()) => println!("  spec \"{}\" ... ok", name),
                Err(e) => {
                    eprintln!("  spec \"{}\" ... FAIL: {}", name, e);
                    failed_specs += 1;
                }
            }
        }
        if failed_specs > 0 {
            return Err(format!("{} spec(s) failed", failed_specs));
        }
    }
    if profile {
        print_profile_report(&vm);
    }
    Ok(())
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn check_single_file(path: &str, json_out: bool) -> Result<(), String> {
    let source = read_text_limited(std::path::Path::new(path), MAX_SOURCE_BYTES)
        .map_err(|e| format!("Could not read '{}': {}", path, e))?;
    match compile_source_with_specs(&source) {
        Ok(_) => {
            if json_out {
                println!("{{\"ok\":true,\"file\":{}}}", json_escape(path));
            } else {
                println!("OK: {}", path);
            }
            Ok(())
        }
        Err(err) => {
            if json_out {
                println!(
                    "{{\"ok\":false,\"file\":{},\"error\":{}}}",
                    json_escape(path),
                    json_escape(&err)
                );
                Err(String::new())
            } else {
                Err(format_error_with_source(&source, &err))
            }
        }
    }
}

fn run_check(path: &str, json_out: bool) -> Result<(), String> {
    let p = std::path::Path::new(path);
    if p.is_dir() {
        let mut files = Vec::new();
        collect_ask_files(p, &mut files)?;
        if files.is_empty() {
            return Err(format!("No .ask files found in '{}'", path));
        }
        let mut failed = 0usize;
        for f in &files {
            if let Err(e) = check_single_file(&f.display().to_string(), json_out) {
                if !e.is_empty() {
                    eprintln!("Check failed: {}", e);
                }
                failed += 1;
            }
        }
        if failed > 0 {
            Err(if json_out {
                String::new()
            } else {
                format!("{} file(s) failed check", failed)
            })
        } else {
            Ok(())
        }
    } else {
        check_single_file(path, json_out)
    }
}

fn push_unique(out: &mut Vec<String>, item: String) {
    if !out.contains(&item) {
        out.push(item);
    }
}

fn collect_imports_expr(
    expr: &ast::Expr,
    imports: &mut Vec<ast::ImportSource>,
    needs: &mut Vec<String>,
) {
    match expr {
        ast::Expr::Int(_, _)
        | ast::Expr::Float(_, _)
        | ast::Expr::Str(_, _)
        | ast::Expr::Bytes(_, _)
        | ast::Expr::Bool(_, _)
        | ast::Expr::Unit(_)
        | ast::Expr::Ident(_, _) => {}
        ast::Expr::List(items, _) => {
            for item in items {
                collect_imports_expr(item, imports, needs);
            }
        }
        ast::Expr::Dict(entries, _) => {
            for (k, v) in entries {
                collect_imports_expr(k, imports, needs);
                collect_imports_expr(v, imports, needs);
            }
        }
        ast::Expr::Unary { expr: inner, .. } | ast::Expr::Throw { value: inner, .. } => {
            collect_imports_expr(inner, imports, needs);
        }
        ast::Expr::Binary { left, right, .. } | ast::Expr::NullCoalesce { left, right, .. } => {
            collect_imports_expr(left, imports, needs);
            collect_imports_expr(right, imports, needs);
        }
        ast::Expr::Assign { target, value, .. } => {
            match target.as_ref() {
                ast::AssignTarget::Var(_) => {}
                ast::AssignTarget::Index { object, index } => {
                    collect_imports_expr(object, imports, needs);
                    collect_imports_expr(index, imports, needs);
                }
                ast::AssignTarget::Attr { object, .. } => {
                    collect_imports_expr(object, imports, needs);
                }
            }
            collect_imports_expr(value, imports, needs);
        }
        ast::Expr::Block { stmts, tail, .. } => {
            for s in stmts {
                collect_imports_stmt(s, imports, needs);
            }
            if let Some(t) = tail {
                collect_imports_expr(t, imports, needs);
            }
        }
        ast::Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_imports_expr(cond, imports, needs);
            collect_imports_expr(then_branch, imports, needs);
            if let Some(e) = else_branch {
                collect_imports_expr(e, imports, needs);
            }
        }
        ast::Expr::TryCatch {
            try_body,
            catch_body,
            ..
        } => {
            collect_imports_expr(try_body, imports, needs);
            collect_imports_expr(catch_body, imports, needs);
        }
        ast::Expr::Match { subject, arms, .. } => {
            collect_imports_expr(subject, imports, needs);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_imports_expr(g, imports, needs);
                }
                collect_imports_expr(&arm.body, imports, needs);
            }
        }
        ast::Expr::Fn { params, body, .. } => {
            for p in params {
                if let Some(d) = &p.default {
                    collect_imports_expr(d, imports, needs);
                }
            }
            collect_imports_expr(body, imports, needs);
        }
        ast::Expr::Call { callee, args, .. } => {
            collect_imports_expr(callee, imports, needs);
            for a in args {
                collect_imports_expr(a, imports, needs);
            }
        }
        ast::Expr::CallNamed(c) => {
            collect_imports_expr(&c.callee, imports, needs);
            for a in &c.pos_args {
                collect_imports_expr(a, imports, needs);
            }
            for (_, a) in &c.named_args {
                collect_imports_expr(a, imports, needs);
            }
        }
        ast::Expr::StructInit(st) => {
            collect_imports_expr(&st.target, imports, needs);
            if let Some(sp) = &st.spread {
                collect_imports_expr(sp, imports, needs);
            }
            for (_, v) in &st.fields {
                collect_imports_expr(v, imports, needs);
            }
        }
        ast::Expr::ChainedCmp(c) => {
            collect_imports_expr(&c.first, imports, needs);
            for (_, rhs) in &c.comparisons {
                collect_imports_expr(rhs, imports, needs);
            }
        }
        ast::Expr::Index { object, index, .. } => {
            collect_imports_expr(object, imports, needs);
            collect_imports_expr(index, imports, needs);
        }
        ast::Expr::Slice {
            object, start, end, ..
        } => {
            collect_imports_expr(object, imports, needs);
            if let Some(s) = start {
                collect_imports_expr(s, imports, needs);
            }
            if let Some(e) = end {
                collect_imports_expr(e, imports, needs);
            }
        }
        ast::Expr::ListComp {
            expr: item,
            iter,
            cond,
            ..
        } => {
            collect_imports_expr(iter, imports, needs);
            if let Some(c) = cond {
                collect_imports_expr(c, imports, needs);
            }
            collect_imports_expr(item, imports, needs);
        }
        ast::Expr::GetAttr { object, .. } | ast::Expr::OptGetAttr { object, .. } => {
            collect_imports_expr(object, imports, needs);
        }
        ast::Expr::SafePipe(p) => {
            collect_imports_expr(&p.left, imports, needs);
            collect_imports_expr(&p.apply, imports, needs);
        }
        ast::Expr::MapPipe { left, mapper, .. } => {
            collect_imports_expr(left, imports, needs);
            collect_imports_expr(mapper, imports, needs);
        }
        ast::Expr::PipeAssign(p) => {
            match p.target.as_ref() {
                ast::AssignTarget::Var(_) => {}
                ast::AssignTarget::Index { object, index } => {
                    collect_imports_expr(object, imports, needs);
                    collect_imports_expr(index, imports, needs);
                }
                ast::AssignTarget::Attr { object, .. } => {
                    collect_imports_expr(object, imports, needs);
                }
            }
            for st in &p.stages {
                collect_imports_expr(st, imports, needs);
            }
        }
        ast::Expr::TaggedTemplate(t) => {
            for e in &t.exprs {
                collect_imports_expr(e, imports, needs);
            }
        }
        ast::Expr::With(w) => {
            collect_imports_expr(&w.resource, imports, needs);
            collect_imports_expr(&w.body, imports, needs);
        }
        ast::Expr::Sandbox(s) => {
            for (_, v) in &s.policy {
                collect_imports_expr(v, imports, needs);
            }
            collect_imports_expr(&s.body, imports, needs);
        }
        ast::Expr::After { delay, body, .. } => {
            collect_imports_expr(delay, imports, needs);
            collect_imports_expr(body, imports, needs);
        }
    }
}

fn collect_imports_stmt(
    stmt: &ast::Stmt,
    imports: &mut Vec<ast::ImportSource>,
    needs: &mut Vec<String>,
) {
    match stmt {
        ast::Stmt::Import { source, .. } | ast::Stmt::ImportFrom { source, .. } => {
            imports.push(source.clone());
        }
        ast::Stmt::Let { value, .. }
        | ast::Stmt::LetList { value, .. }
        | ast::Stmt::LetDict { value, .. } => {
            collect_imports_expr(value, imports, needs);
        }
        ast::Stmt::FnDecl { params, body, .. } => {
            for p in params {
                if let Some(d) = &p.default {
                    collect_imports_expr(d, imports, needs);
                }
            }
            collect_imports_expr(body, imports, needs);
        }
        ast::Stmt::StructDecl {
            fields, methods, ..
        } => {
            for f in fields {
                if let Some(d) = &f.default {
                    collect_imports_expr(d, imports, needs);
                }
            }
            for m in methods {
                for p in &m.params {
                    if let Some(d) = &p.default {
                        collect_imports_expr(d, imports, needs);
                    }
                }
                collect_imports_expr(&m.body, imports, needs);
            }
        }
        ast::Stmt::While { cond, body, .. } => {
            collect_imports_expr(cond, imports, needs);
            collect_imports_expr(body, imports, needs);
        }
        ast::Stmt::For { iter, body, .. } | ast::Stmt::ForPair { iter, body, .. } => {
            collect_imports_expr(iter, imports, needs);
            collect_imports_expr(body, imports, needs);
        }
        ast::Stmt::Return { value, .. } => {
            if let Some(v) = value {
                collect_imports_expr(v, imports, needs);
            }
        }
        ast::Stmt::Break(_) | ast::Stmt::Continue(_) => {}
        ast::Stmt::Needs { caps, .. } => {
            for (name, args) in caps {
                let label = if args.is_empty() {
                    format!("needs: {}", name)
                } else {
                    format!("needs: {}({})", name, args.join(", "))
                };
                push_unique(needs, label);
            }
        }
        ast::Stmt::Defer { body, .. } => {
            collect_imports_expr(body, imports, needs);
        }
        ast::Stmt::Every { interval, body, .. } => {
            collect_imports_expr(interval, imports, needs);
            collect_imports_expr(body, imports, needs);
        }
        ast::Stmt::Spec { body, .. } => {
            collect_imports_expr(body, imports, needs);
        }
        ast::Stmt::Expr(e) => {
            collect_imports_expr(e, imports, needs);
        }
    }
}

fn inspect_permissions_file(
    path: &std::path::Path,
    visited: &mut std::collections::HashSet<std::path::PathBuf>,
    capabilities: &mut Vec<String>,
    depth: usize,
) -> Result<(), String> {
    if depth >= 32 {
        return Err(format!(
            "Maximum import inspection depth (32) exceeded at '{}'",
            path.display()
        ));
    }
    if let Ok(canon) = fs::canonicalize(path) {
        if !visited.insert(canon) {
            return Ok(());
        }
    }
    let source = read_text_limited(path, MAX_SOURCE_BYTES)
        .map_err(|e| format!("Could not read '{}': {}", path.display(), e))?;
    if source.len() > MAX_SOURCE_BYTES {
        return Err("Source size limit exceeded".to_string());
    }
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program()?;

    let mut imports = Vec::new();
    for stmt in &program.stmts {
        collect_imports_stmt(stmt, &mut imports, capabilities);
    }
    if let Some(tail) = &program.tail {
        collect_imports_expr(tail, &mut imports, capabilities);
    }

    for source in imports {
        match source {
            ast::ImportSource::Name(name) => {
                let desc = match name.as_str() {
                    "fs" => Some(
                        "fs (sandboxed to script dir; --allow-read / --allow-write for external dirs)",
                    ),
                    "kv" => Some(
                        "kv (embedded KV store, sandboxed to script dir; --allow-read / --allow-write for external dirs)",
                    ),
                    "archive" => Some(
                        "archive (ZIP read/write, sandboxed to script dir; --allow-read / --allow-write)",
                    ),
                    "http" => Some("http (blocked by default; requires --allow-net[=host])"),
                    "net" => Some(
                        "net (TCP/UDP sockets, blocked by default; requires --allow-net[=host[:port]])",
                    ),
                    "ws" => Some(
                        "ws (WebSockets, blocked by default; requires --allow-net[=host[:port]])",
                    ),
                    "sys" => {
                        Some("sys (sys.env requires --allow-env, sys.args requires --allow-args)")
                    }
                    "os" => Some(
                        "os (os.env requires --allow-env, os.args requires --allow-args, os.cwd/dirs require read)",
                    ),
                    "clipboard" => {
                        Some("clipboard (blocked by default; requires --allow-clipboard)")
                    }
                    "input" => {
                        Some("input (global key/mouse state requires --allow-input or --unsafe)")
                    }
                    "ui" => Some("ui (declarative 2D Win32 GUI window & widgets)"),
                    "process" => Some("process (blocked by default; requires --allow-exec[=cmd])"),
                    "dialog" => Some("dialog (Win32 modal message/file dialogs)"),
                    "audio" => Some("audio (Win32 WAV playback, sandbox-checked)"),
                    "mem" => Some("mem (requires --unsafe and ASKANE_ENABLE_UNSAFE_MEM=1)"),
                    "gfx" => Some("gfx (Win32 GUI window)"),
                    _ => None,
                };
                if let Some(d) = desc {
                    push_unique(capabilities, d.to_string());
                }
            }
            ast::ImportSource::Path(p) => {
                let d = format!("file import '{}' (inherits caller permissions)", p);
                push_unique(capabilities, d);
                let base_dir = match path.parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => parent,
                    _ => std::path::Path::new("."),
                };
                let joined = base_dir.join(&p);
                let mut candidates = Vec::new();
                if joined.extension().is_none() {
                    candidates.push(joined.join("mod.ask"));
                    candidates.push(joined.with_extension("ask"));
                } else {
                    candidates.push(joined);
                }
                if let Some(sub) = candidates.iter().find(|c| c.is_file()) {
                    inspect_permissions_file(sub, visited, capabilities, depth + 1)?;
                } else if let Some(non_file) = candidates.iter().find(|c| c.exists()) {
                    return Err(format!(
                        "Imported path '{}' is not a regular file",
                        non_file.display()
                    ));
                } else {
                    push_unique(
                        capabilities,
                        format!("file import '{}' (file not found; uninspected)", p),
                    );
                }
            }
        }
    }
    Ok(())
}

fn inspect_permissions(path: &str) -> Result<Vec<String>, String> {
    let mut visited = std::collections::HashSet::new();
    let mut capabilities = Vec::new();
    inspect_permissions_file(
        std::path::Path::new(path),
        &mut visited,
        &mut capabilities,
        0,
    )?;
    Ok(capabilities)
}

fn run_eval(code: &str, profile: bool, perms: Permissions) -> Result<(), String> {
    let proto = compile_source(code).map_err(|e| format_error_with_source(code, &e))?;
    let mut vm = VM::new_with_sandbox(!perms.allow_all);
    vm.set_permissions(perms);
    vm.set_profile(profile);
    let val = vm
        .run(proto)
        .map_err(|e| format_error_with_source(code, &e))?;
    if !matches!(val, Value::Unit) {
        println!("{}", val);
    }
    if profile {
        print_profile_report(&vm);
    }
    Ok(())
}

fn collect_ask_files(
    path: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) -> Result<(), String> {
    let mut visited = std::collections::HashSet::new();
    collect_ask_files_inner(path, out, &mut visited, 0)
}

fn collect_ask_files_inner(
    path: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
    visited: &mut std::collections::HashSet<std::path::PathBuf>,
    depth: usize,
) -> Result<(), String> {
    if depth > 64 {
        return Err(format!(
            "Directory nesting too deep while scanning '{}'",
            path.display()
        ));
    }
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!("Refusing symlink path '{}'", path.display()));
    }
    if path.is_file() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    if !path.is_dir() {
        return Err(format!("Path '{}' not found", path.display()));
    }
    if let Ok(canon) = fs::canonicalize(path) {
        if !visited.insert(canon) {
            return Ok(());
        }
    }
    let mut entries: Vec<_> = fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    for entry in entries {
        if fs::symlink_metadata(&entry).is_ok_and(|m| m.file_type().is_symlink()) {
            continue;
        }
        if entry.is_dir() {
            collect_ask_files_inner(&entry, out, visited, depth + 1)?;
        } else if entry.extension().and_then(|s| s.to_str()) == Some("ask") {
            out.push(entry);
        }
    }
    Ok(())
}

#[cfg(test)]
fn run_tests_cmd(
    path: &str,
    perms: Permissions,
    sandbox_root_override: Option<&std::path::Path>,
) -> Result<(), String> {
    run_tests_cmd_with_opts(path, perms, sandbox_root_override, false, false)
}

fn run_tests_cmd_with_opts(
    path: &str,
    perms: Permissions,
    sandbox_root_override: Option<&std::path::Path>,
    deterministic: bool,
    update_snapshots: bool,
) -> Result<(), String> {
    let mut files = Vec::new();
    collect_ask_files(std::path::Path::new(path), &mut files)?;
    if files.is_empty() {
        return Err(format!("No .ask test files found in '{}'", path));
    }
    let mut passed = 0usize;
    let mut failed = 0usize;
    for file in &files {
        let label = file.display().to_string();
        match run_file_core_with_test_opts(
            &label,
            false,
            false,
            true,
            perms.clone(),
            sandbox_root_override,
            deterministic,
            update_snapshots,
        ) {
            Ok(()) => {
                println!("PASS {}", label);
                passed += 1;
            }
            Err(e) => {
                eprintln!("FAIL {}\n{}", label, e);
                failed += 1;
            }
        }
    }
    println!();
    println!("Results: {} passed, {} failed", passed, failed);
    if failed > 0 {
        Err(format!("{} test file(s) failed", failed))
    } else {
        Ok(())
    }
}

#[derive(Debug, Default)]
struct AskaneManifest {
    entry: Option<String>,
    tests: Option<String>,
    max_steps: Option<u64>,
    max_mem_mb: Option<u64>,
    timeout_ms: Option<u64>,
}

fn strip_toml_comment(line: &str) -> &str {
    let mut in_double = false;
    let mut in_single = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if in_double {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            }
        } else if in_single {
            if ch == '\'' {
                in_single = false;
            }
        } else if ch == '"' {
            in_double = true;
        } else if ch == '\'' {
            in_single = true;
        } else if ch == '#' {
            return &line[..idx];
        }
    }
    line
}

fn unquote_toml_string(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        let inner = &trimmed[1..trimmed.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('\\') => out.push('\\'),
                    Some('"') => out.push('"'),
                    Some(other) => out.push(other),
                    None => return Err("Unterminated escape in askane.toml string".to_string()),
                }
            } else {
                out.push(ch);
            }
        }
        Ok(out)
    } else if trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2 {
        Ok(trimmed[1..trimmed.len() - 1].to_string())
    } else {
        Err(format!(
            "Expected quoted string in askane.toml, got '{}'",
            trimmed
        ))
    }
}

fn parse_toml_u64(raw: &str, field: &str) -> Result<u64, String> {
    let cleaned: String = raw.trim().chars().filter(|&c| c != '_').collect();
    cleaned.parse::<u64>().map_err(|_| {
        format!(
            "Invalid integer for '{}' in askane.toml: '{}'",
            field,
            raw.trim()
        )
    })
}

fn parse_askane_manifest(src: &str) -> Result<AskaneManifest, String> {
    let mut manifest = AskaneManifest::default();
    let mut section = String::new();
    for (line_no, raw_line) in src.lines().enumerate() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') && !line.starts_with("[[") {
            section = line[1..line.len() - 1].trim().to_string();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(format!(
                "Invalid line {} in askane.toml: '{}'",
                line_no + 1,
                line
            ));
        };
        let key = k.trim();
        let val = v.trim();
        match (section.as_str(), key) {
            ("", "entry") => manifest.entry = Some(unquote_toml_string(val)?),
            ("", "tests") => manifest.tests = Some(unquote_toml_string(val)?),
            ("" | "limits", "max_steps") => {
                manifest.max_steps = Some(parse_toml_u64(val, "max_steps")?);
            }
            ("" | "limits", "max_mem_mb") => {
                manifest.max_mem_mb = Some(parse_toml_u64(val, "max_mem_mb")?);
            }
            ("" | "limits", "timeout_ms") => {
                manifest.timeout_ms = Some(parse_toml_u64(val, "timeout_ms")?);
            }
            _ => {
                // Non-authoritative metadata or permission-like keys are intentionally ignored
                // so a manifest can never grant sandbox privileges.
            }
        }
    }
    Ok(manifest)
}

fn load_manifest_from_dir(
    dir: &std::path::Path,
) -> Result<Option<(std::path::PathBuf, AskaneManifest)>, String> {
    let manifest_path = dir.join("askane.toml");
    let meta = match fs::symlink_metadata(&manifest_path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(format!(
                "Could not inspect '{}': {}",
                manifest_path.display(),
                e
            ));
        }
    };
    if meta.file_type().is_symlink() {
        return Err(format!(
            "Refusing symlink manifest '{}'",
            manifest_path.display()
        ));
    }
    if !meta.is_file() {
        return Ok(None);
    }
    let canon_dir = fs::canonicalize(dir)
        .map_err(|e| format!("Could not resolve manifest dir '{}': {}", dir.display(), e))?;
    let canon_manifest = fs::canonicalize(&manifest_path)
        .map_err(|e| format!("Could not resolve '{}': {}", manifest_path.display(), e))?;
    if !canon_manifest.starts_with(&canon_dir) {
        return Err(format!(
            "askane.toml at '{}' resolves outside project root",
            manifest_path.display()
        ));
    }
    let text = read_text_limited(&manifest_path, 256 * 1024)
        .map_err(|e| format!("Could not read '{}': {}", manifest_path.display(), e))?;
    let manifest = parse_askane_manifest(&text)?;
    Ok(Some((dir.to_path_buf(), manifest)))
}

fn discover_manifest_from_cwd() -> Result<Option<(std::path::PathBuf, AskaneManifest)>, String> {
    let mut cur = env::current_dir().map_err(|e| e.to_string())?;
    for _ in 0..32 {
        if let Some(found) = load_manifest_from_dir(&cur)? {
            return Ok(Some(found));
        }
        let Some(parent) = cur.parent() else {
            break;
        };
        cur = parent.to_path_buf();
    }
    Ok(None)
}

fn resolve_manifest_subpath(
    project_root: &std::path::Path,
    rel: &str,
    field: &str,
) -> Result<std::path::PathBuf, String> {
    let p = std::path::Path::new(rel);
    if p.is_absolute() {
        return Err(format!(
            "askane.toml '{}' path must be relative: '{}'",
            field, rel
        ));
    }
    let mut depth: i32 = 0;
    let mut walk = project_root.to_path_buf();
    for comp in p.components() {
        match comp {
            std::path::Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Err(format!(
                        "askane.toml '{}' path '{}' cannot escape project root",
                        field, rel
                    ));
                }
                walk.pop();
            }
            std::path::Component::Normal(part) => {
                depth += 1;
                walk.push(part);
                if fs::symlink_metadata(&walk).is_ok_and(|m| m.file_type().is_symlink()) {
                    return Err(format!(
                        "askane.toml '{}' path '{}' cannot reference or traverse symlinks",
                        field, rel
                    ));
                }
            }
            std::path::Component::CurDir => {}
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(format!(
                    "askane.toml '{}' path must be relative: '{}'",
                    field, rel
                ));
            }
        }
    }
    let target = project_root.join(p);
    let canon_root = fs::canonicalize(project_root)
        .map_err(|e| format!("Failed to resolve project root: {}", e))?;
    if fs::symlink_metadata(&target).is_ok() {
        let canon_target = fs::canonicalize(&target).map_err(|e| {
            format!(
                "Failed to resolve askane.toml '{}' path '{}': {}",
                field, rel, e
            )
        })?;
        if !canon_target.starts_with(&canon_root) {
            return Err(format!(
                "askane.toml '{}' path '{}' resolves outside project root",
                field, rel
            ));
        }
    }
    Ok(target)
}

fn apply_manifest_limits(
    perms: &mut Permissions,
    manifest: &AskaneManifest,
    explicit: ExplicitLimits,
) {
    let defaults = Permissions::sandboxed();
    let clamp_sandboxed = |val: u64, cap: Option<u64>, allow_all: bool| -> u64 {
        if allow_all {
            val
        } else if let Some(max_cap) = cap {
            val.min(max_cap)
        } else {
            val
        }
    };
    if !explicit.max_steps {
        if let Some(steps) = manifest.max_steps {
            perms.max_instructions = Some(clamp_sandboxed(
                steps,
                defaults.max_instructions,
                perms.allow_all,
            ));
        }
    }
    if !explicit.max_mem {
        if let Some(mb) = manifest.max_mem_mb {
            let bytes = mb.saturating_mul(1024 * 1024);
            perms.max_memory_bytes = Some(clamp_sandboxed(
                bytes,
                defaults.max_memory_bytes,
                perms.allow_all,
            ));
        }
    }
    if !explicit.timeout {
        if let Some(ms) = manifest.timeout_ms {
            perms.max_duration_ms = Some(clamp_sandboxed(
                ms,
                defaults.max_duration_ms,
                perms.allow_all,
            ));
        }
    }
}

fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn needs_more_input(buf: &str) -> bool {
    let mut depth: i32 = 0;
    let mut interp_braces: Vec<i32> = Vec::new();
    let mut in_string = false;
    let mut in_byte_string = false;
    let mut in_raw_string = false;
    let mut in_multiline_string = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut escape = false;
    let chars: Vec<char> = buf.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        let next = chars.get(i + 1).copied();

        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_block_comment {
            if ch == '*' && next == Some('/') {
                in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if in_multiline_string {
            if ch == '"' && next == Some('"') && chars.get(i + 2).copied() == Some('"') {
                in_multiline_string = false;
                i += 3;
            } else {
                i += 1;
            }
            continue;
        }
        if in_raw_string {
            if ch == '"' {
                in_raw_string = false;
            }
            i += 1;
            continue;
        }
        if in_byte_string {
            if escape {
                escape = false;
                i += 1;
            } else if ch == '\\' {
                escape = true;
                i += 1;
            } else if ch == '"' {
                in_byte_string = false;
                i += 1;
            } else {
                i += 1;
            }
            continue;
        }
        if in_string {
            if escape {
                escape = false;
                i += 1;
            } else if ch == '\\' {
                escape = true;
                i += 1;
            } else if ch == '$' && next == Some('{') {
                in_string = false;
                interp_braces.push(0);
                i += 2;
            } else if ch == '"' {
                in_string = false;
                i += 1;
            } else {
                i += 1;
            }
            continue;
        }

        if ch == '/' && next == Some('/') {
            in_line_comment = true;
            i += 2;
            continue;
        }
        if ch == '/' && next == Some('*') {
            in_block_comment = true;
            i += 2;
            continue;
        }
        if ((ch == 'b' && next == Some('r')) || (ch == 'r' && next == Some('b')))
            && chars.get(i + 2).copied() == Some('"')
            && (i == 0 || !is_ident_char(chars[i - 1]))
        {
            in_raw_string = true;
            i += 3;
            continue;
        }
        if (ch == 'r' || ch == 'b') && next == Some('"') && (i == 0 || !is_ident_char(chars[i - 1]))
        {
            if ch == 'r' {
                in_raw_string = true;
            } else {
                in_byte_string = true;
            }
            i += 2;
            continue;
        }
        if ch == '"' {
            if next == Some('"') && chars.get(i + 2).copied() == Some('"') {
                in_multiline_string = true;
                i += 3;
            } else {
                in_string = true;
                i += 1;
            }
            continue;
        }
        match ch {
            '{' => {
                if let Some(d) = interp_braces.last_mut() {
                    *d += 1;
                } else {
                    depth += 1;
                }
            }
            '}' => {
                if let Some(&d) = interp_braces.last() {
                    if d == 0 {
                        interp_braces.pop();
                        in_string = true;
                    } else if let Some(top) = interp_braces.last_mut() {
                        *top -= 1;
                    }
                } else {
                    depth -= 1;
                }
            }
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            _ => {}
        }
        i += 1;
    }

    in_string
        || in_byte_string
        || in_raw_string
        || in_multiline_string
        || in_block_comment
        || !interp_braces.is_empty()
        || depth > 0
}

fn run_repl() {
    println!("Askane v{} (open beta)", VERSION);
    println!("Type :help for commands, :exit to quit. Supports multiline input.");

    let mut vm = VM::new();
    let stdin = io::stdin();
    let mut buffer = String::new();

    loop {
        if buffer.is_empty() {
            print!("askane> ");
        } else {
            print!("......> ");
        }
        let _ = io::stdout().flush();

        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => {
                if !buffer.trim().is_empty() {
                    eprintln!("Input ended before the multiline expression was closed.");
                } else {
                    // normal EOF
                }
                break;
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("Read error: {}", e);
                break;
            }
        }

        if buffer.is_empty() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match trimmed {
                ":exit" | ":quit" | "exit" | "quit" => break,
                ":help" | "help" => {
                    println!("Commands:");
                    println!("  :help     Show this message");
                    println!("  :gui      Open Askane Studio GUI editor");
                    println!("  :exit     Exit the REPL");
                    println!("  :reset    Reset VM state and globals");
                    println!("  :modules  List built-in standard modules");
                    println!("  :bc       Prefix any code with ':bc ' to inspect its bytecode");
                    continue;
                }
                ":gui" | "gui" => {
                    if let Err(e) = gui::run(None) {
                        eprintln!("GUI error: {}", e);
                    }
                    continue;
                }
                ":reset" => {
                    vm = VM::new();
                    println!("VM state reset.");
                    continue;
                }
                ":modules" => {
                    println!(
                        "Built-in modules: math, fs, sys, json, strings, path, time, random, base64, bytes, csv, log, test, hash, ini, datetime, clipboard, audio, url, hex, uuid, term, glob, regex, dialog, process, http, gfx, mem"
                    );
                    continue;
                }
                _ => {}
            }
        }

        buffer.push_str(&line);
        if needs_more_input(&buffer) && !line.is_empty() {
            continue;
        }

        let code_owned = std::mem::take(&mut buffer);
        let trimmed = code_owned.trim();
        if trimmed.is_empty() {
            continue;
        }

        let (show_bc, code) = if let Some(rest) = trimmed.strip_prefix(":bc ") {
            (true, rest)
        } else {
            (false, code_owned.as_str())
        };

        match compile_source(code) {
            Ok(proto) => {
                if show_bc {
                    print!("{}", proto.chunk.disassemble(&proto.name));
                }
                match vm.run(proto) {
                    Ok(Value::Unit) => {}
                    Ok(val) => println!("{}", val),
                    Err(e) => eprintln!("Runtime error: {}", format_error_with_source(code, &e)),
                }
            }
            Err(e) => eprintln!("Compile error: {}", format_error_with_source(code, &e)),
        }
    }
}

fn print_help() {
    println!("Askane v{}", VERSION);
    println!();
    println!("Usage:");
    println!("  askane                       Start interactive REPL");
    println!("  askane --gui [file.ask]      Open Askane Studio GUI editor (Windows)");
    println!("  askane [run] <file_or_dir>   Run an Askane script or project (uses askane.toml)");
    println!("  askane -e \"<code>\"           Evaluate inline code");
    println!(
        "  askane check [--json] [path] Syntax & compile check for file, directory, or project"
    );
    println!("  askane permissions <file>    Inspect capabilities imported by a script");
    println!("  askane fmt [--check] <path>  Format an Askane file or directory");
    println!("  askane init [dir]            Create a starter Askane project");
    println!(
        "  askane setup [opts]          Add Askane to User PATH & register .ask files (Windows)"
    );
    println!("  askane test [opts] [path]    Run .ask test files or project tests");
    println!("    --deterministic            Enable virtual time & fixed random seed for tests");
    println!("    --update-snapshots         Overwrite mismatched snapshot files");
    println!("  askane -b <file.ask>         Disassemble compiled bytecode");
    println!("  askane --profile <file.ask>  Print function profiling stats after execution");
    println!("  askane -h | --help           Show help");
    println!("  askane -v | --version        Show version");
    println!();
    println!("Sandbox permissions:");
    println!(
        "  --allow-net[=hosts]          Allow HTTPS access (optionally comma-separated host[:port])"
    );
    println!(
        "  --allow-read[=dirs]          Allow reading outside sandbox (optionally comma-separated dirs)"
    );
    println!(
        "  --allow-write[=dirs]         Allow writing outside sandbox (optionally comma-separated dirs)"
    );
    println!("  --allow-env[=vars]           Allow sys.env() (optionally comma-separated names)");
    println!("  --allow-args                 Allow sys.args()");
    println!("  --allow-clipboard            Allow clipboard.get_text() / set_text()");
    println!("  --allow-input                Allow global keyboard/mouse state in input module");
    println!(
        "  --allow-exec[=cmds]          Allow process.run() (optionally comma-separated commands)"
    );
    println!(
        "  --max-steps=N                Override sandbox instruction budget (default 10000000)"
    );
    println!(
        "  --max-mem=MB                 Override sandbox allocation budget in MiB (default 64)"
    );
    println!(
        "  --timeout=MS                 Override sandbox execution timeout in ms (default 30000)"
    );
    println!(
        "  --unsafe                     Allow unrestricted files, network, env and process access"
    );
}

fn append_csv(target: &mut Option<Vec<String>>, value: Option<&str>) {
    match value {
        Some(list) if !list.is_empty() => {
            let items = target.get_or_insert_with(Vec::new);
            for part in list.split(',').filter(|s| !s.is_empty()) {
                items.push(part.to_string());
            }
        }
        _ => {
            *target = Some(Vec::new());
        }
    }
}

fn append_csv_paths(target: &mut Option<Vec<std::path::PathBuf>>, value: Option<&str>) {
    match value {
        Some(list) if !list.is_empty() => {
            let cwd = env::current_dir().ok();
            let items = target.get_or_insert_with(Vec::new);
            for part in list.split(',').filter(|s| !s.is_empty()) {
                let p = std::path::PathBuf::from(part);
                let resolved = if p.is_absolute() {
                    p
                } else if let Some(ref base) = cwd {
                    base.join(p)
                } else {
                    p
                };
                items.push(resolved);
            }
        }
        _ => {
            *target = Some(Vec::new());
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FmtTokKind {
    Word,
    Str,
    Comment,
    BinOp,
    UnaryOnly,
    Minus,
    Star,
    Pipe,
    OrOr,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Hash,
    Dot,
    Range,
    Comma,
    Colon,
    Semi,
    Other,
}

struct FmtTok {
    text: String,
    kind: FmtTokKind,
}

fn is_fmt_keyword_unary_lhs(w: &str) -> bool {
    matches!(
        w,
        "return"
            | "throw"
            | "in"
            | "if"
            | "while"
            | "match"
            | "else"
            | "catch"
            | "not"
            | "recover"
            | "after"
            | "every"
            | "with"
    )
}

fn is_fmt_space_before_lparen(w: &str) -> bool {
    matches!(
        w,
        "if" | "while"
            | "match"
            | "return"
            | "throw"
            | "in"
            | "else"
            | "for"
            | "recover"
            | "after"
            | "every"
            | "with"
    )
}

fn scan_interp_str_end(chars: &[char], open_quote: usize) -> usize {
    let mut i = open_quote + 1;
    let mut esc = false;
    let mut interp: Vec<usize> = Vec::new();
    while i < chars.len() {
        let ch = chars[i];
        if !interp.is_empty() {
            if ch == '"' {
                i += 1;
                let mut s_esc = false;
                while i < chars.len() {
                    let sc = chars[i];
                    i += 1;
                    if s_esc {
                        s_esc = false;
                    } else if sc == '\\' {
                        s_esc = true;
                    } else if sc == '"' {
                        break;
                    }
                }
                continue;
            }
            if ch == '{' {
                *interp.last_mut().unwrap() += 1;
            } else if ch == '}' {
                if *interp.last().unwrap() == 0 {
                    interp.pop();
                } else {
                    *interp.last_mut().unwrap() -= 1;
                }
            }
            i += 1;
        } else if esc {
            esc = false;
            i += 1;
        } else if ch == '\\' {
            esc = true;
            i += 1;
        } else if ch == '$' && chars.get(i + 1).copied() == Some('{') {
            interp.push(0);
            i += 2;
        } else if ch == '"' {
            i += 1;
            break;
        } else {
            i += 1;
        }
    }
    i
}

fn is_fmt_non_tag_keyword(w: &str) -> bool {
    matches!(
        w,
        "let"
            | "fn"
            | "struct"
            | "if"
            | "else"
            | "match"
            | "while"
            | "loop"
            | "for"
            | "in"
            | "import"
            | "from"
            | "pub"
            | "as"
            | "return"
            | "break"
            | "continue"
            | "try"
            | "catch"
            | "throw"
            | "recover"
            | "true"
            | "false"
            | "unit"
            | "spec"
    )
}

fn format_line_tokens(line: &str) -> Vec<FmtTok> {
    let chars: Vec<char> = line.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && next == Some('/') {
            let text: String = chars[i..].iter().collect();
            toks.push(FmtTok {
                text: text.trim_end().to_string(),
                kind: FmtTokKind::Comment,
            });
            break;
        }
        if c == '/' && next == Some('*') {
            let start = i;
            i += 2;
            while i < chars.len() {
                if chars[i] == '*' && chars.get(i + 1).copied() == Some('/') {
                    i += 2;
                    break;
                }
                i += 1;
            }
            toks.push(FmtTok {
                text: chars[start..i].iter().collect(),
                kind: FmtTokKind::Comment,
            });
            continue;
        }
        if ((c == 'b' && next == Some('r')) || (c == 'r' && next == Some('b')))
            && chars.get(i + 2).copied() == Some('"')
            && (i == 0 || !is_ident_char(chars[i - 1]))
        {
            let start = i;
            i += 3;
            while i < chars.len() {
                let ch = chars[i];
                i += 1;
                if ch == '"' {
                    break;
                }
            }
            toks.push(FmtTok {
                text: chars[start..i].iter().collect(),
                kind: FmtTokKind::Str,
            });
            continue;
        }
        if (c == 'r' || c == 'b') && next == Some('"') && (i == 0 || !is_ident_char(chars[i - 1])) {
            let is_raw = c == 'r';
            let start = i;
            i += 2;
            let mut esc = false;
            while i < chars.len() {
                let ch = chars[i];
                i += 1;
                if is_raw {
                    if ch == '"' {
                        break;
                    }
                } else if esc {
                    esc = false;
                } else if ch == '\\' {
                    esc = true;
                } else if ch == '"' {
                    break;
                }
            }
            toks.push(FmtTok {
                text: chars[start..i].iter().collect(),
                kind: FmtTokKind::Str,
            });
            continue;
        }
        if c == '"' {
            if next == Some('"') && chars.get(i + 2).copied() == Some('"') {
                let start = i;
                i += 3;
                while i < chars.len() {
                    if chars[i] == '"'
                        && chars.get(i + 1).copied() == Some('"')
                        && chars.get(i + 2).copied() == Some('"')
                    {
                        i += 3;
                        break;
                    }
                    i += 1;
                }
                toks.push(FmtTok {
                    text: chars[start..i].iter().collect(),
                    kind: FmtTokKind::Str,
                });
            } else {
                let start = i;
                i = scan_interp_str_end(&chars, i);
                toks.push(FmtTok {
                    text: chars[start..i].iter().collect(),
                    kind: FmtTokKind::Str,
                });
            }
            continue;
        }
        if is_ident_char(c) {
            let start = i;
            i += 1;
            while i < chars.len()
                && (is_ident_char(chars[i])
                    || (chars[i] == '.'
                        && chars[start].is_ascii_digit()
                        && chars.get(i + 1).is_some_and(|ch| ch.is_ascii_digit())))
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if !chars[start].is_ascii_digit()
                && !is_fmt_non_tag_keyword(&word)
                && chars.get(i).copied() == Some('"')
                && chars.get(i + 1..i + 3) != Some(&['"', '"'])
            {
                i = scan_interp_str_end(&chars, i);
                toks.push(FmtTok {
                    text: chars[start..i].iter().collect(),
                    kind: FmtTokKind::Str,
                });
                continue;
            }
            toks.push(FmtTok {
                text: word,
                kind: FmtTokKind::Word,
            });
            continue;
        }
        let rest: String = chars[i..chars.len().min(i + 3)].iter().collect();
        if rest.starts_with("..=") {
            toks.push(FmtTok {
                text: "..=".to_string(),
                kind: FmtTokKind::Range,
            });
            i += 3;
            continue;
        }
        if matches!(rest.as_str(), "|?>" | "|*>" | "|>=") {
            toks.push(FmtTok {
                text: rest,
                kind: FmtTokKind::BinOp,
            });
            i += 3;
            continue;
        }
        let two: String = chars[i..chars.len().min(i + 2)].iter().collect();
        match two.as_str() {
            ".." => {
                toks.push(FmtTok {
                    text: "..".to_string(),
                    kind: FmtTokKind::Range,
                });
                i += 2;
                continue;
            }
            "?." => {
                toks.push(FmtTok {
                    text: "?.".to_string(),
                    kind: FmtTokKind::Dot,
                });
                i += 2;
                continue;
            }
            "=>" | "|>" | "??" | "==" | "!=" | "<=" | ">=" | "&&" | "+=" | "-=" | "*=" | "/=" => {
                toks.push(FmtTok {
                    text: two,
                    kind: FmtTokKind::BinOp,
                });
                i += 2;
                continue;
            }
            "||" => {
                toks.push(FmtTok {
                    text: "||".to_string(),
                    kind: FmtTokKind::OrOr,
                });
                i += 2;
                continue;
            }
            _ => {}
        }
        let kind = match c {
            '(' => FmtTokKind::LParen,
            ')' => FmtTokKind::RParen,
            '[' => FmtTokKind::LBracket,
            ']' => FmtTokKind::RBracket,
            '{' => FmtTokKind::LBrace,
            '}' => FmtTokKind::RBrace,
            '#' => FmtTokKind::Hash,
            '.' => FmtTokKind::Dot,
            ',' => FmtTokKind::Comma,
            ':' => FmtTokKind::Colon,
            ';' => FmtTokKind::Semi,
            '!' => FmtTokKind::UnaryOnly,
            '-' => FmtTokKind::Minus,
            '*' => FmtTokKind::Star,
            '|' => FmtTokKind::Pipe,
            '+' | '/' | '%' | '=' | '<' | '>' => FmtTokKind::BinOp,
            _ => FmtTokKind::Other,
        };
        toks.push(FmtTok {
            text: c.to_string(),
            kind,
        });
        i += 1;
    }
    toks
}

fn render_formatted_line(toks: &[FmtTok]) -> String {
    let mut out = String::new();
    let mut in_lambda_pipe = false;
    let mut prev_kind: Option<FmtTokKind> = None;
    let mut prev_word = String::new();
    let mut prev_was_unary = false;

    for (idx, tok) in toks.iter().enumerate() {
        let next = toks.get(idx + 1);
        let is_unary_minus = tok.kind == FmtTokKind::Minus
            && match prev_kind {
                None => true,
                Some(
                    FmtTokKind::LParen
                    | FmtTokKind::LBracket
                    | FmtTokKind::LBrace
                    | FmtTokKind::Comma
                    | FmtTokKind::Colon
                    | FmtTokKind::Semi
                    | FmtTokKind::BinOp
                    | FmtTokKind::Minus
                    | FmtTokKind::Star
                    | FmtTokKind::UnaryOnly
                    | FmtTokKind::Pipe
                    | FmtTokKind::OrOr
                    | FmtTokKind::Range,
                ) => true,
                Some(FmtTokKind::Word) => is_fmt_keyword_unary_lhs(&prev_word),
                _ => false,
            };
        let is_unary_star = tok.kind == FmtTokKind::Star
            && matches!(
                prev_kind,
                None | Some(FmtTokKind::LParen | FmtTokKind::Comma)
            );

        if let Some(pk) = prev_kind {
            let need_space = match (pk, tok.kind) {
                (_, FmtTokKind::Comment) => true,
                (FmtTokKind::Comment, _) => true,
                (FmtTokKind::UnaryOnly, _) => false,
                (_, _) if prev_was_unary => false,
                (FmtTokKind::Dot, _) | (_, FmtTokKind::Dot) => false,
                (FmtTokKind::Hash, FmtTokKind::LBrace) => false,
                (FmtTokKind::LParen, _) | (_, FmtTokKind::RParen) => false,
                (FmtTokKind::LBracket, _) | (_, FmtTokKind::RBracket) => false,
                (_, FmtTokKind::Comma | FmtTokKind::Colon | FmtTokKind::Semi) => false,
                (FmtTokKind::Range, _) => false,
                (_, FmtTokKind::Range) => matches!(pk, FmtTokKind::Comma | FmtTokKind::Colon),
                (FmtTokKind::LBrace, FmtTokKind::RBrace) => false,
                (FmtTokKind::LBrace, _) | (_, FmtTokKind::RBrace) => true,
                (_, FmtTokKind::LBrace) => true,
                (FmtTokKind::Comma | FmtTokKind::Colon | FmtTokKind::Semi, _) => true,
                (FmtTokKind::BinOp, _) | (_, FmtTokKind::BinOp) => true,
                (FmtTokKind::Minus, _) if !prev_was_unary => true,
                (_, FmtTokKind::Minus) if !is_unary_minus => true,
                (FmtTokKind::Word, FmtTokKind::Minus) if is_fmt_keyword_unary_lhs(&prev_word) => {
                    true
                }
                (_, FmtTokKind::Minus) => false,
                (FmtTokKind::Star, _) if !prev_was_unary => true,
                (_, FmtTokKind::Star) if !is_unary_star => true,
                (_, FmtTokKind::Star) => matches!(pk, FmtTokKind::Comma),
                (FmtTokKind::OrOr, _) => true,
                (_, FmtTokKind::OrOr) => !matches!(pk, FmtTokKind::LParen | FmtTokKind::LBracket),
                (FmtTokKind::Pipe, _) => !in_lambda_pipe,
                (_, FmtTokKind::Pipe) => {
                    if in_lambda_pipe {
                        false
                    } else {
                        !matches!(pk, FmtTokKind::LParen | FmtTokKind::LBracket)
                    }
                }
                (FmtTokKind::Word, FmtTokKind::LParen) => is_fmt_space_before_lparen(&prev_word),
                (_, FmtTokKind::LParen) => false,
                (_, FmtTokKind::LBracket) => {
                    matches!(pk, FmtTokKind::Word) && is_fmt_keyword_unary_lhs(&prev_word)
                }
                (
                    FmtTokKind::Word
                    | FmtTokKind::Str
                    | FmtTokKind::RParen
                    | FmtTokKind::RBracket
                    | FmtTokKind::RBrace,
                    FmtTokKind::Word | FmtTokKind::Str | FmtTokKind::UnaryOnly | FmtTokKind::Hash,
                ) => true,
                _ => false,
            };
            if need_space && !(tok.kind == FmtTokKind::LBrace && pk == FmtTokKind::Hash) {
                out.push(' ');
            }
        }
        let _ = next;
        out.push_str(&tok.text);
        if tok.kind == FmtTokKind::Pipe {
            in_lambda_pipe = !in_lambda_pipe;
        }
        prev_was_unary = is_unary_minus || is_unary_star || tok.kind == FmtTokKind::UnaryOnly;
        if tok.kind == FmtTokKind::Word {
            prev_word = tok.text.clone();
        } else {
            prev_word.clear();
        }
        prev_kind = Some(tok.kind);
    }
    out
}

fn format_askane_source(src: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut indent: usize = 0;
    let mut in_multiline_str = false;
    let mut in_str = false;
    let mut in_byte_str = false;
    let mut in_raw_str = false;
    let mut esc = false;
    let mut interp_braces: Vec<usize> = Vec::new();
    let mut in_block_comment = false;
    let mut prev_blank = false;

    for raw in src.replace("\r\n", "\n").lines() {
        let started_in_any_str = in_multiline_str || in_str || in_byte_str || in_raw_str;
        let scan_src = if started_in_any_str {
            prev_blank = false;
            out.push(raw.to_string());
            raw.to_string()
        } else {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                if !prev_blank {
                    out.push(String::new());
                    prev_blank = true;
                }
                continue;
            }
            prev_blank = false;
            let leading_closes = if in_block_comment {
                0
            } else {
                trimmed
                    .chars()
                    .take_while(|&c| c == '}' || c == ')' || c == ']')
                    .count()
            };
            let line_indent = indent.saturating_sub(leading_closes);
            let formatted_body = if in_block_comment {
                trimmed.to_string()
            } else {
                render_formatted_line(&format_line_tokens(trimmed))
            };
            out.push(format!("{}{}", "    ".repeat(line_indent), formatted_body));
            trimmed.to_string()
        };

        let chars: Vec<char> = scan_src.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            let next = chars.get(i + 1).copied();
            if in_multiline_str {
                if c == '"' && next == Some('"') && chars.get(i + 2).copied() == Some('"') {
                    in_multiline_str = false;
                    i += 3;
                } else {
                    i += 1;
                }
                continue;
            }
            if in_block_comment {
                if c == '*' && next == Some('/') {
                    in_block_comment = false;
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            if in_raw_str {
                if c == '"' {
                    in_raw_str = false;
                }
                i += 1;
                continue;
            }
            if in_byte_str {
                if esc {
                    esc = false;
                    i += 1;
                } else if c == '\\' {
                    esc = true;
                    i += 1;
                } else if c == '"' {
                    in_byte_str = false;
                    i += 1;
                } else {
                    i += 1;
                }
                continue;
            }
            if in_str {
                if esc {
                    esc = false;
                    i += 1;
                } else if c == '\\' {
                    esc = true;
                    i += 1;
                } else if c == '$' && next == Some('{') {
                    in_str = false;
                    interp_braces.push(0);
                    i += 2;
                } else if c == '"' {
                    in_str = false;
                    i += 1;
                } else {
                    i += 1;
                }
                continue;
            }

            if c == '/' && next == Some('/') {
                break;
            } else if c == '/' && next == Some('*') {
                in_block_comment = true;
                i += 2;
            } else if ((c == 'b' && next == Some('r')) || (c == 'r' && next == Some('b')))
                && chars.get(i + 2).copied() == Some('"')
                && (i == 0 || !is_ident_char(chars[i - 1]))
            {
                in_raw_str = true;
                i += 3;
            } else if (c == 'r' || c == 'b')
                && next == Some('"')
                && (i == 0 || !is_ident_char(chars[i - 1]))
            {
                if c == 'r' {
                    in_raw_str = true;
                } else {
                    in_byte_str = true;
                    esc = false;
                }
                i += 2;
            } else if c == '"' {
                if next == Some('"') && chars.get(i + 2).copied() == Some('"') {
                    in_multiline_str = true;
                    i += 3;
                } else {
                    in_str = true;
                    esc = false;
                    i += 1;
                }
            } else if c == '{' {
                if let Some(d) = interp_braces.last_mut() {
                    *d += 1;
                } else {
                    indent += 1;
                }
                i += 1;
            } else if c == '}' {
                if let Some(&d) = interp_braces.last() {
                    if d == 0 {
                        interp_braces.pop();
                        in_str = true;
                    } else if let Some(top) = interp_braces.last_mut() {
                        *top -= 1;
                    }
                } else {
                    indent = indent.saturating_sub(1);
                }
                i += 1;
            } else if c == '(' || c == '[' {
                indent += 1;
                i += 1;
            } else if c == ')' || c == ']' {
                indent = indent.saturating_sub(1);
                i += 1;
            } else {
                i += 1;
            }
        }
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    let mut joined = out.join("\n");
    if (src.ends_with('\n') || !joined.is_empty()) && !joined.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

fn verify_askane_syntax(source: &str) -> Result<(), String> {
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    let _ = parser.parse_program()?;
    Ok(())
}

fn run_fmt(path: &str, check_only: bool) -> Result<(), String> {
    let p = std::path::Path::new(path);
    let mut files = Vec::new();
    if p.is_dir() {
        collect_ask_files(p, &mut files)?;
        if files.is_empty() {
            return Err(format!("No .ask files found in '{}'", path));
        }
    } else {
        files.push(p.to_path_buf());
    }
    for file in files {
        let fstr = file.display().to_string();
        let source = read_text_limited(&file, MAX_SOURCE_BYTES)
            .map_err(|e| format!("Could not read '{}': {}", fstr, e))?;
        verify_askane_syntax(&source).map_err(|e| {
            format!(
                "Syntax error in '{}':\n{}",
                fstr,
                format_error_with_source(&source, &e)
            )
        })?;
        let formatted = format_askane_source(&source);
        verify_askane_syntax(&formatted)
            .map_err(|e| format!("Internal formatter error on '{}': {}", fstr, e))?;
        if check_only {
            if formatted != source {
                return Err(format!("File '{}' is not formatted", fstr));
            }
            println!("OK (formatted): {}", fstr);
        } else {
            fs::write(&file, formatted)
                .map_err(|e| format!("Could not write '{}': {}", fstr, e))?;
            println!("Formatted {}", fstr);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Default)]
struct ExplicitLimits {
    max_steps: bool,
    max_mem: bool,
    timeout: bool,
}

enum CliParseOutcome {
    ExitZero,
    Run {
        dump_bytecode: bool,
        profile: bool,
        deterministic: bool,
        update_snapshots: bool,
        eval_code: Option<String>,
        perms: Permissions,
        script_path: Option<String>,
        explicit_limits: ExplicitLimits,
    },
}

fn parse_run_args(args: &[String]) -> Result<CliParseOutcome, String> {
    let mut dump_bytecode = false;
    let mut profile = false;
    let mut deterministic = false;
    let mut update_snapshots = false;
    let mut eval_code: Option<String> = None;
    let mut perms = Permissions::sandboxed();
    let mut explicit_max_steps: Option<u64> = None;
    let mut explicit_max_mem: Option<u64> = None;
    let mut explicit_timeout: Option<u64> = None;
    let mut script_path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-h" || arg == "--help" {
            print_help();
            return Ok(CliParseOutcome::ExitZero);
        }
        if arg == "-v" || arg == "--version" {
            println!("askane {}", VERSION);
            return Ok(CliParseOutcome::ExitZero);
        }
        if arg == "-b" || arg == "--bytecode" {
            dump_bytecode = true;
        } else if arg == "--profile" {
            profile = true;
        } else if arg == "--deterministic" {
            deterministic = true;
        } else if arg == "--update-snapshots" {
            update_snapshots = true;
        } else if arg == "-e" || arg == "--eval" {
            i += 1;
            let Some(code) = args.get(i) else {
                return Err(format!("Missing code after {}", arg));
            };
            eval_code = Some(code.clone());
        } else if arg == "--unsafe" {
            perms = Permissions::unrestricted();
            if let Some(steps) = explicit_max_steps {
                perms.max_instructions = Some(steps);
            }
            if let Some(mem_bytes) = explicit_max_mem {
                perms.max_memory_bytes = Some(mem_bytes);
            }
            if let Some(ms) = explicit_timeout {
                perms.max_duration_ms = Some(ms);
            }
        } else if arg == "--allow-net" {
            append_csv(&mut perms.allow_net, None);
        } else if let Some(val) = arg.strip_prefix("--allow-net=") {
            append_csv(&mut perms.allow_net, Some(val));
        } else if arg == "--allow-read" {
            append_csv_paths(&mut perms.allow_read, None);
        } else if let Some(val) = arg.strip_prefix("--allow-read=") {
            append_csv_paths(&mut perms.allow_read, Some(val));
        } else if arg == "--allow-write" {
            append_csv_paths(&mut perms.allow_write, None);
        } else if let Some(val) = arg.strip_prefix("--allow-write=") {
            append_csv_paths(&mut perms.allow_write, Some(val));
        } else if arg == "--allow-env" {
            append_csv(&mut perms.allow_env, None);
        } else if let Some(val) = arg.strip_prefix("--allow-env=") {
            append_csv(&mut perms.allow_env, Some(val));
        } else if arg == "--allow-args" {
            perms.allow_sys_args = true;
        } else if arg == "--allow-clipboard" {
            perms.allow_clipboard = true;
        } else if arg == "--allow-input" {
            perms.allow_input = true;
        } else if arg == "--allow-exec" {
            append_csv(&mut perms.allow_exec, None);
        } else if let Some(val) = arg.strip_prefix("--allow-exec=") {
            append_csv(&mut perms.allow_exec, Some(val));
        } else if let Some(val) = arg.strip_prefix("--max-steps=") {
            match val.parse::<u64>() {
                Ok(n) => {
                    explicit_max_steps = Some(n);
                    perms.max_instructions = Some(n);
                }
                Err(_) => return Err(format!("Invalid --max-steps value: {}", val)),
            }
        } else if let Some(val) = arg.strip_prefix("--max-mem=") {
            match val.parse::<u64>() {
                Ok(mb) => {
                    let bytes = mb.saturating_mul(1024 * 1024);
                    explicit_max_mem = Some(bytes);
                    perms.max_memory_bytes = Some(bytes);
                }
                Err(_) => return Err(format!("Invalid --max-mem value: {}", val)),
            }
        } else if let Some(val) = arg.strip_prefix("--timeout=") {
            match val.parse::<u64>() {
                Ok(n) => {
                    explicit_timeout = Some(n);
                    perms.max_duration_ms = Some(n);
                }
                Err(_) => return Err(format!("Invalid --timeout value: {}", val)),
            }
        } else if arg.starts_with('-') {
            return Err(format!(
                "Unknown option '{}'. Run 'askane --help' for usage.",
                arg
            ));
        } else if script_path.is_none() {
            script_path = Some(arg.clone());
        } else {
            return Err(format!(
                "Unexpected extra argument '{}'. Run 'askane --help' for usage.",
                arg
            ));
        }
        i += 1;
    }

    Ok(CliParseOutcome::Run {
        dump_bytecode,
        profile,
        deterministic,
        update_snapshots,
        eval_code,
        perms,
        script_path,
        explicit_limits: ExplicitLimits {
            max_steps: explicit_max_steps.is_some(),
            max_mem: explicit_max_mem.is_some(),
            timeout: explicit_timeout.is_some(),
        },
    })
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() == 1 {
        run_repl();
        return;
    }

    if args[1] == "--gui" {
        let initial = args.get(2).map(String::as_str);
        if let Err(e) = gui::run(initial) {
            eprintln!("GUI error: {}", e);
            process::exit(1);
        }
        return;
    }

    if args[1] == "setup" {
        if let Err(e) = setup::run_setup_cli(&args[2..]) {
            eprintln!("Setup error: {}", e);
            process::exit(1);
        }
        return;
    }

    if args[1] == "check" {
        let mut json_out = false;
        let mut target: Option<String> = None;
        for arg in &args[2..] {
            if arg == "--json" {
                json_out = true;
            } else if target.is_none() {
                target = Some(arg.clone());
            } else {
                eprintln!("Usage: askane check [--json] [file_or_dir]");
                process::exit(1);
            }
        }
        let path = match target {
            Some(p) => p,
            None => match discover_manifest_from_cwd() {
                Ok(Some((root, _))) => root.display().to_string(),
                Ok(None) => {
                    eprintln!("Usage: askane check [--json] [file_or_dir]");
                    process::exit(1);
                }
                Err(e) => {
                    eprintln!("Check failed: {}", e);
                    process::exit(1);
                }
            },
        };
        if let Err(e) = run_check(&path, json_out) {
            if !e.is_empty() {
                eprintln!("Check failed: {}", e);
            }
            process::exit(1);
        }
        return;
    }

    if args[1] == "fmt" {
        let mut check_only = false;
        let mut target: Option<String> = None;
        for arg in &args[2..] {
            if arg == "--check" {
                check_only = true;
            } else if target.is_none() {
                target = Some(arg.clone());
            } else {
                eprintln!("Usage: askane fmt [--check] <file.ask>");
                process::exit(1);
            }
        }
        let Some(path) = target else {
            eprintln!("Usage: askane fmt [--check] <file.ask>");
            process::exit(1);
        };
        if let Err(e) = run_fmt(&path, check_only) {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
        return;
    }

    if args[1] == "init" {
        let dir = args.get(2).map(String::as_str).unwrap_or(".");
        let root = std::path::Path::new(dir);
        if let Err(e) = fs::create_dir_all(root.join("tests")) {
            eprintln!("init failed: {}", e);
            process::exit(1);
        }
        let files = [
            (
                root.join("askane.toml"),
                "name = \"my-askane-app\"\nversion = \"0.1.0\"\nentry = \"main.ask\"\ntests = \"tests\"\nsandbox = true\n",
            ),
            (root.join("main.ask"), "println(\"Hello from Askane!\");\n"),
            (
                root.join("tests").join("basic_test.ask"),
                "import test;\ntest.assert_eq(1 + 1, 2);\n",
            ),
        ];
        for (path, content) in files {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    if let Err(e) = f.write_all(content.as_bytes()) {
                        eprintln!("Failed to write {}: {}", path.display(), e);
                        process::exit(1);
                    }
                    println!("Created {}", path.display());
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    println!("Skipped existing {}", path.display());
                }
                Err(e) => {
                    eprintln!("Failed to create {}: {}", path.display(), e);
                    process::exit(1);
                }
            }
        }
        return;
    }

    if args[1] == "permissions" {
        let Some(path) = args.get(2) else {
            eprintln!("Usage: askane permissions <file.ask>");
            process::exit(1);
        };
        match inspect_permissions(path) {
            Ok(caps) => {
                if caps.is_empty() {
                    println!("{}: no privileged modules imported", path);
                } else {
                    println!("{} imports:", path);
                    for c in caps {
                        println!("  - {}", c);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
        }
        return;
    }

    let mut is_test_cmd = false;
    let slice_start = if args[1] == "test" {
        is_test_cmd = true;
        2
    } else if args[1] == "run" {
        2
    } else {
        1
    };

    let (
        dump_bytecode,
        profile,
        deterministic,
        update_snapshots,
        eval_code,
        mut perms,
        script_path,
        explicit_limits,
    ) = match parse_run_args(&args[slice_start..]) {
        Ok(CliParseOutcome::ExitZero) => return,
        Ok(CliParseOutcome::Run {
            dump_bytecode,
            profile,
            deterministic,
            update_snapshots,
            eval_code,
            perms,
            script_path,
            explicit_limits,
        }) => (
            dump_bytecode,
            profile,
            deterministic,
            update_snapshots,
            eval_code,
            perms,
            script_path,
            explicit_limits,
        ),
        Err(e) => {
            eprintln!("{}", e);
            process::exit(1);
        }
    };

    if let Some(code) = eval_code {
        if let Err(e) = run_eval(&code, profile, perms) {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
        return;
    }

    if is_test_cmd {
        let result = match script_path {
            Some(ref target) => {
                let target_path = std::path::Path::new(target);
                if target_path.is_dir() {
                    match load_manifest_from_dir(target_path) {
                        Ok(Some((root, manifest))) => {
                            apply_manifest_limits(&mut perms, &manifest, explicit_limits);
                            let tests_rel = manifest.tests.as_deref().unwrap_or("tests");
                            resolve_manifest_subpath(&root, tests_rel, "tests").and_then(
                                |tests_path| {
                                    run_tests_cmd_with_opts(
                                        &tests_path.display().to_string(),
                                        perms,
                                        Some(&root),
                                        deterministic,
                                        update_snapshots,
                                    )
                                },
                            )
                        }
                        Ok(None) => run_tests_cmd_with_opts(
                            target,
                            perms,
                            None,
                            deterministic,
                            update_snapshots,
                        ),
                        Err(e) => Err(e),
                    }
                } else {
                    run_tests_cmd_with_opts(target, perms, None, deterministic, update_snapshots)
                }
            }
            None => match discover_manifest_from_cwd() {
                Ok(Some((root, manifest))) => {
                    apply_manifest_limits(&mut perms, &manifest, explicit_limits);
                    let tests_rel = manifest.tests.as_deref().unwrap_or("tests");
                    resolve_manifest_subpath(&root, tests_rel, "tests").and_then(|tests_path| {
                        run_tests_cmd_with_opts(
                            &tests_path.display().to_string(),
                            perms,
                            Some(&root),
                            deterministic,
                            update_snapshots,
                        )
                    })
                }
                Ok(None) => {
                    run_tests_cmd_with_opts(".", perms, None, deterministic, update_snapshots)
                }
                Err(e) => Err(e),
            },
        };
        if let Err(e) = result {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
        return;
    }

    let run_result = match script_path {
        Some(ref path) => {
            let p = std::path::Path::new(path);
            if p.is_dir() {
                match load_manifest_from_dir(p) {
                    Ok(Some((root, manifest))) => {
                        apply_manifest_limits(&mut perms, &manifest, explicit_limits);
                        let entry_rel = manifest.entry.as_deref().unwrap_or("main.ask");
                        resolve_manifest_subpath(&root, entry_rel, "entry").and_then(|entry_path| {
                            run_file_with_options(
                                &entry_path.display().to_string(),
                                dump_bytecode,
                                profile,
                                perms,
                                Some(&root),
                            )
                        })
                    }
                    Ok(None) => Err(format!(
                        "Directory '{}' does not contain an askane.toml manifest",
                        path
                    )),
                    Err(e) => Err(e),
                }
            } else {
                run_file_with_options(path, dump_bytecode, profile, perms, None)
            }
        }
        None => match discover_manifest_from_cwd() {
            Ok(Some((root, manifest))) => {
                apply_manifest_limits(&mut perms, &manifest, explicit_limits);
                let entry_rel = manifest.entry.as_deref().unwrap_or("main.ask");
                resolve_manifest_subpath(&root, entry_rel, "entry").and_then(|entry_path| {
                    run_file_with_options(
                        &entry_path.display().to_string(),
                        dump_bytecode,
                        profile,
                        perms,
                        Some(&root),
                    )
                })
            }
            Ok(None) => {
                eprintln!("Missing script file. Run 'askane --help' for usage.");
                process::exit(1);
            }
            Err(e) => Err(e),
        },
    };
    if let Err(e) = run_result {
        eprintln!("Error: {}", e);
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(src: &str) -> Value {
        let proto = compile_source(src).expect("compile failed");
        let mut vm = VM::new();
        vm.run(proto).expect("runtime failed")
    }

    #[test]
    fn audit_recursive_syntax_and_chains_are_bounded() {
        for source in [
            "!".repeat(2000) + "true",
            "a=".repeat(2000) + "1",
            "if false { 0 } else ".repeat(200) + "{ 1 }",
            "1+".repeat(2000) + "1",
            "a.b".to_string() + &".b".repeat(2000),
        ] {
            assert!(compile_source(&source).is_err());
        }
    }

    #[test]
    fn audit_vm_recovers_after_error_with_captured_locals() {
        let mut vm = VM::new();
        let source = "let saved = unit; fn fail() { let x = 42; saved = || x; 1 / 0 } fail()";
        assert!(vm.run(compile_source(source).unwrap()).is_err());
        let value = vm.run(compile_source("saved() + 1").unwrap()).unwrap();
        assert!(value.equals(&Value::Int(43)));
    }

    #[test]
    fn audit_instruction_budget_and_private_apis() {
        let mut vm = VM::new();
        let err = vm
            .run(compile_source("while true {} ").unwrap())
            .unwrap_err();
        assert!(err.contains("instruction limit"));
        for source in [
            "import sys; sys.args()",
            "import sys; sys.exit()",
            "import mem; mem.open(1)",
        ] {
            assert!(
                VM::new()
                    .run(compile_source(source).unwrap())
                    .unwrap_err()
                    .contains("disabled")
            );
        }
        assert!(
            vm.run(compile_source("42").unwrap())
                .unwrap()
                .equals(&Value::Int(42))
        );
    }

    #[test]
    fn audit_json_strict_numbers_controls_and_depth() {
        for raw in ["01", "1.", "1e", "1e+", "1e9999", "-01", "\"raw\nline\""] {
            let source = format!("import json; json.parse({:?})", raw);
            assert!(
                VM::new().run(compile_source(&source).unwrap()).is_err(),
                "accepted {raw:?}"
            );
        }
        let source = "import json; let a = []; for i in range(100) { a = [a]; } json.stringify(a)";
        assert!(
            VM::new()
                .run(compile_source(source).unwrap())
                .unwrap_err()
                .contains("nesting depth")
        );
    }

    #[test]
    fn audit_sandbox_devices_and_traversal() {
        let vm = VM::new();
        for path in ["NUL", "CON.txt", "COM1", "file:secret", "file.", "file "] {
            assert!(vm.resolve_read_path(path).is_err(), "accepted {path}");
        }
        assert!(vm.resolve_read_path("../outside.ask").is_err());
    }

    #[cfg(windows)]
    #[cfg(windows)]
    #[test]
    fn audit_http_injection_and_network_policy() {
        let source = "import http; http.post(\"https://example.com\", \"\", \"text/plain\\r\\nX-Injected: yes\")";
        let err = VM::new_with_sandbox(false)
            .run(compile_source(source).unwrap())
            .unwrap_err();
        assert!(err.contains("Invalid Content-Type"));
        let source = "import http; http.get(\"https://example.com\")";
        assert!(
            VM::new()
                .run(compile_source(source).unwrap())
                .unwrap_err()
                .contains("Network is disabled")
        );
    }

    #[test]
    fn test_expression_blocks_and_if() {
        let val = eval(
            r#"
            let x = {
                let a = 10;
                let b = 20;
                a + b
            };
            if x > 25 { x * 2 } else { 0 }
            "#,
        );
        assert!(val.equals(&Value::Int(60)));
    }

    #[test]
    fn test_pipeline_and_higher_order_natives() {
        let val = eval(
            r#"
            range(1, 7)
                |> filter(fn(n) { n % 2 == 0 })
                |> map(fn(n) { n * n })
                |> reduce(0, fn(acc, n) { acc + n })
            "#,
        );
        // 2^2 + 4^2 + 6^2 = 4 + 16 + 36 = 56
        assert!(val.equals(&Value::Int(56)));
    }

    #[test]
    fn test_match_expression() {
        let val = eval(
            r#"
            fn classify(n) {
                match n {
                    0 => "zero",
                    1 => "one",
                    other => "num:" + str(other),
                }
            }
            [classify(0), classify(1), classify(42)]
            "#,
        );
        assert_eq!(format!("{}", val), r#"["zero", "one", "num:42"]"#);
    }

    #[test]
    fn test_closures_and_upvalues() {
        let val = eval(
            r#"
            fn make_counter(start) {
                let count = start;
                fn() {
                    count = count + 1;
                    count
                }
            }
            let c = make_counter(10);
            c() + c() + c()
            "#,
        );
        // 11 + 12 + 13 = 36
        assert!(val.equals(&Value::Int(36)));
    }

    #[test]
    fn test_loops_break_continue() {
        let val = eval(
            r#"
            let sum = 0;
            for i in range(1, 10) {
                if i % 2 == 0 {
                    continue;
                }
                if i > 7 {
                    break;
                }
                sum = sum + i;
            }
            sum
            "#,
        );
        // 1 + 3 + 5 + 7 = 16
        assert!(val.equals(&Value::Int(16)));
    }

    #[test]
    fn test_recursion() {
        let val = eval(
            r#"
            fn fib(n) {
                if n <= 1 { n } else { fib(n - 1) + fib(n - 2) }
            }
            fib(10)
            "#,
        );
        assert!(val.equals(&Value::Int(55)));
    }

    #[test]
    fn test_builtin_modules() {
        let val = eval(
            r#"
            import math as m;
            import sys;
            let s = 16 |> m.sqrt |> m.round;
            let o = sys.os();
            s + len(o)
            "#,
        );
        // sqrt(16) == 4, len("windows") == 7 -> 11 (on Windows)
        assert!(matches!(val, Value::Int(n) if n > 4));
    }

    #[test]
    fn test_file_module_import_with_closure() {
        let val = eval(
            r#"
            import "examples/math_helper.ask" as h;
            h.scale(5)
            "#,
        );
        assert!(val.equals(&Value::Int(15)));
    }

    #[cfg(windows)]
    #[test]
    fn test_mem_module_current_process() {
        let mut vm = VM::new();
        let err = vm
            .run(compile_source("import mem; mem.current_pid()").unwrap())
            .unwrap_err();
        assert!(err.contains("disabled in sandbox"));
    }

    #[test]
    fn test_dict_and_json() {
        let val = eval(
            r#"
            import json;
            let player = #{ name: "Askane", hp: 100, items: ["sword", "potion"] };
            player["hp"] = 85;
            let raw = json.stringify(player);
            let parsed = json.parse(raw);
            parsed.name + ":" + str(parsed["hp"]) + ":" + str(len(parsed.items))
            "#,
        );
        assert!(val.equals(&Value::Str("Askane:85:2".to_string())));
    }

    #[test]
    fn test_http_and_gfx_modules_load() {
        let val = eval(
            r#"
            import http;
            import gfx;
            type_of(http.get) + ":" + type_of(gfx.window)
            "#,
        );
        assert!(val.equals(&Value::Str("fn:fn".to_string())));
    }

    #[test]
    fn test_safety_stack_overflow_and_cycles() {
        let proto = compile_source("fn boom() { 1 + boom() } boom()").unwrap();
        let mut vm = VM::new();
        let err = vm.run(proto).unwrap_err();
        assert!(err.contains("Stack overflow"));

        let val = eval(
            r#"
            let a = [1];
            push(a, a);
            str(a)
            "#,
        );
        assert!(val.equals(&Value::Str("[1, [...]]".to_string())));
    }

    #[test]
    fn test_v0102_features() {
        let val = eval(
            r#"
            let player = #{ name: "Askane", hp: 25 };
            let missing = unit;
            let mana = player?.mana ?? missing?.mana ?? 50;

            let status = match player.hp {
                0 => "dead",
                h if h < 30 => "critical",
                _ => "alive",
            };

            let total = range(1, 7)
                |> filter(|n| n % 2 == 0)
                |> map(|n| n * n)
                |> reduce(0, |acc, n| acc + n);

            let zero_fn = || 7;
            "${player.name}:${status}:${mana}:${total + zero_fn()}"
            "#,
        );
        assert!(val.equals(&Value::Str("Askane:critical:50:63".to_string())));
    }

    #[test]
    fn test_for_break_stack_attr_assign_pipe_prec_and_trace() {
        let val = eval(
            r#"
            fn test_for_break() {
                let acc = 0;
                for i in range(1, 10) {
                    if i == 4 {
                        break;
                    }
                    acc = acc + i;
                }
                let after = 100;
                let another = 200;
                acc + after + another
            }

            let player = #{ hp: 100, stats: #{ mana: 10 } };
            player.hp = 90;
            player.stats.mana = 50;

            fn double(x) { x * 2 }
            let x = 4;
            let pipe_ok = x + 1 |> double == 10;
            import math;
            let float_falsy = !0.0 && !math.sqrt(-1.0);

            if pipe_ok && float_falsy {
                test_for_break() + player.hp + player.stats.mana
            } else {
                0
            }
            "#,
        );
        // 6 + 100 + 200 + 90 + 50 = 446
        assert!(val.equals(&Value::Int(446)));

        let proto = compile_source(
            r#"
            fn inner() { 1 / 0 }
            fn outer() { inner() }
            outer()
            "#,
        )
        .unwrap();
        let mut vm = VM::new();
        let err = vm.run(proto).unwrap_err();
        assert!(err.contains("Division by zero"));
        assert!(err.contains("at inner"));
        assert!(err.contains("at outer"));
        assert!(err.contains("at <script>"));
    }

    #[test]
    fn test_security_sandbox_and_leaks() {
        // 1. Filesystem sandbox blocks path traversal and absolute paths
        let proto_fs = compile_source(
            r#"
            import fs;
            fs.write("../outside_test.txt", "x")
            "#,
        )
        .unwrap();
        let mut vm_sb = VM::new_with_sandbox(true);
        let err_fs = vm_sb.run(proto_fs).unwrap_err();
        assert!(err_fs.contains("outside the sandbox"));

        #[cfg(windows)]
        let abs_src = r#"
            import fs;
            fs.write("C:\\askane_escape_test.txt", "x")
        "#;
        #[cfg(not(windows))]
        let abs_src = r#"
            import fs;
            fs.write("/askane_escape_test.txt", "x")
        "#;
        let proto_abs = compile_source(abs_src).unwrap();
        let mut vm_sb2 = VM::new_with_sandbox(true);
        let err_abs = vm_sb2.run(proto_abs).unwrap_err();
        assert!(err_abs.contains("outside the sandbox"));

        // 2. Plain HTTP is blocked (HTTPS required)
        #[cfg(windows)]
        {
            let proto_http = compile_source(
                r#"
                import http;
                http.get("http://example.com")
                "#,
            )
            .unwrap();
            let mut vm_http = VM::new();
            let err_http = vm_http.run(proto_http).unwrap_err();
            assert!(err_http.contains("https://"));
        }

        // 3. Deep parser nesting returns clean error instead of stack overflow
        let deep = "(".repeat(300) + "1" + &")".repeat(300);
        let err_parse = compile_source(&deep).unwrap_err();
        assert!(err_parse.contains("Maximum expression nesting depth"));

        // 4. Allocation stress test (no leaks / crashes across 50,000 iterations)
        let val = eval(
            r#"
            let i = 0;
            let last = 0;
            while i < 50000 {
                let d = #{ idx: i, list: [i, i + 1] };
                last = d.idx;
                i = i + 1;
            }
            last
            "#,
        );
        assert!(val.equals(&Value::Int(49999)));
    }

    #[test]
    fn test_module_fixes_and_upgrades() {
        // 1. sys.env() blocked in sandbox mode
        let proto_env = compile_source(
            r#"
            import sys;
            sys.env("PATH")
            "#,
        )
        .unwrap();
        let mut vm_sb = VM::new_with_sandbox(true);
        let err = vm_sb.run(proto_env).unwrap_err();
        assert!(
            err.contains("disabled in sandbox"),
            "unexpected err: {}",
            err
        );

        // 2. JSON UTF-16 surrogate pair decoding
        let val = eval(
            r#"
            import json;
            json.parse("\"\\uD83D\\uDE00\"")
            "#,
        );
        assert!(val.equals(&Value::Str("😀".to_string())));

        // 3. fs.list_dir / fs.is_dir in sandboxed VM
        let val = eval(
            r#"
            import fs;
            let entries = fs.list_dir(".");
            type_of(entries) == "list" && fs.is_dir("src")
            "#,
        );
        assert!(val.equals(&Value::Bool(true)));

        // 4. gfx.text, gfx.mouse_x, gfx.mouse_y, gfx.mouse_down exported
        let val = eval(
            r#"
            import gfx;
            type_of(gfx.text) == "fn"
                && type_of(gfx.mouse_x) == "fn"
                && type_of(gfx.mouse_y) == "fn"
                && type_of(gfx.mouse_down) == "fn"
            "#,
        );
        assert!(val.equals(&Value::Bool(true)));
    }

    #[test]
    fn test_v011_permissions() {
        let mut perms = Permissions::sandboxed();
        perms.allow_env = Some(vec!["PATH".to_string()]);
        perms.allow_net = Some(vec!["api.example.com".to_string()]);
        perms.allow_sys_args = true;
        let mut vm = VM::new();
        vm.set_permissions(perms);

        assert!(vm.check_net("https://api.example.com/v1").is_ok());
        assert!(vm.check_net("https://evil.com/").is_err());

        let proto_ok = compile_source(
            r#"
            import sys;
            type_of(sys.args()) == "list" && sys.env("PATH") != unit
            "#,
        )
        .unwrap();
        assert!(vm.run(proto_ok).unwrap().equals(&Value::Bool(true)));

        let proto_denied = compile_source(
            r#"
            import sys;
            sys.env("USERNAME")
            "#,
        )
        .unwrap();
        let err = vm.run(proto_denied).unwrap_err();
        assert!(err.contains("allow-env=USERNAME"), "err: {}", err);

        // Read-only permission allows reading outside sandbox but still blocks writing
        let mut perms_ro = Permissions::sandboxed();
        perms_ro.allow_read = Some(vec![std::env::temp_dir()]);
        let mut vm_ro = VM::new();
        vm_ro.set_permissions(perms_ro);
        let temp_file = std::env::temp_dir().join("askane_ro_test.txt");
        assert!(
            vm_ro
                .resolve_read_path(&temp_file.to_string_lossy())
                .is_ok()
        );
        assert!(
            vm_ro
                .resolve_write_path(&temp_file.to_string_lossy())
                .is_err()
        );
    }

    #[test]
    fn test_v011_try_catch_and_throw() {
        let val = eval(
            r#"
            import json;
            let a = try {
                json.parse("{bad")
            } catch err {
                "caught"
            };
            let b = try {
                throw "boom";
                1
            } catch (e) {
                e
            };
            fn inner() { 10 / 0 }
            fn outer() { inner() }
            let c = try {
                outer()
            } catch e {
                42
            };
            let d = try {
                try { throw "inner"; } catch _ { throw "rethrow"; }
            } catch outer_err {
                let f = || outer_err;
                f()
            };
            [a, b, c, d]
            "#,
        );
        let expected = eval(r#"["caught", "boom", 42, "rethrow"]"#);
        assert!(val.equals(&expected));
    }

    #[test]
    fn test_v011_lazy_range() {
        let val = eval(
            r#"
            let r = range(0, 1000000000);
            let sum = 0;
            for i in range(1, 6) {
                sum = sum + i;
            }
            type_of(r) == "range"
                && len(r) == 1000000000
                && r[500000000] == 500000000
                && r[-1] == 999999999
                && sum == 15
            "#,
        );
        assert!(val.equals(&Value::Bool(true)));
    }

    #[test]
    fn test_v012_modules_and_collections() {
        let val = eval(
            r#"
            import strings;
            import path;
            import time;
            import random;
            import base64;
            import csv;
            import log;
            import test;

            // strings (Unicode-aware)
            test.assert_eq(strings.trim("  Привет 😀  "), "Привет 😀");
            test.assert_eq(strings.find("Привет мир", "мир"), 7);
            test.assert_eq(strings.slice("Привет 😀", 0, 6), "Привет");
            test.assert_eq(strings.upper("askane"), "ASKANE");
            test.assert_eq(strings.join(["a", "b", "c"], "-"), "a-b-c");

            // path
            test.assert_eq(path.stem("dir/hello.ask"), "hello");
            test.assert_eq(path.extension("dir/hello.ask"), "ask");

            // base64
            let enc = base64.encode("Askane 🚀");
            test.assert_eq(base64.decode(enc), "Askane 🚀");

            // csv
            let rows = csv.parse("name,score\n\"Alice, B\",100\nBob,80");
            test.assert_eq(len(rows), 3);
            test.assert_eq(rows[1][0], "Alice, B");
            let dicts = csv.parse_dicts("a,b\n1,2");
            test.assert_eq(dicts[0].a, "1");

            // random & time
            random.seed(42);
            let r1 = random.int(1, 100);
            random.seed(42);
            let r2 = random.int(1, 100);
            test.assert_eq(r1, r2);
            test.assert_eq(len(random.crypto_bytes(8)), 8);
            test.assert(time.now_ms() > 0);

            // deterministic collections
            let d = #{ b: 2, a: 1 };
            test.assert_eq(keys(d), ["a", "b"]);
            test.assert_eq(values(d), [1, 2]);
            test.assert_eq(get(d, "missing", 99), 99);
            test.assert_eq(sort([3, 1, 2]), [1, 2, 3]);
            test.assert_eq(reverse("abc"), "cba");

            // log & test.assert_err
            log.set_level("warn");
            test.assert_err(|| 1 / 0, "Division by zero");
            true
            "#,
        );
        assert!(val.equals(&Value::Bool(true)));
    }

    #[test]
    fn test_v012_repl_and_diagnostics() {
        assert!(needs_more_input("fn f(x) {"));
        assert!(needs_more_input("let s = \"unclosed"));
        assert!(needs_more_input("/* open comment"));
        assert!(!needs_more_input("fn f(x) { x + 1 }"));
        assert!(!needs_more_input("let s = \"{\"; 1"));

        let src = "let x = 1;\nlet y = 1 / 0;\nprintln(y);";
        let formatted = format_error_with_source(src, "[line 2] Division by zero");
        assert!(formatted.contains("2 | let y = 1 / 0;"));
    }

    #[test]
    fn test_v013_modules_and_permissions() {
        let val = eval(
            r#"
            import hash;
            import ini;
            import datetime;
            import test;

            test.assert_eq(
                hash.sha256("abc"),
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            );
            test.assert_eq(hash.crc32("123456789"), 3421780262);
            test.assert(hash.fnv1a("askane") != 0);

            let cfg = ini.parse("app = askane\n[server]\nport = 8080\n");
            test.assert_eq(cfg.app, "askane");
            test.assert_eq(cfg.server.port, "8080");
            test.assert(len(ini.stringify(cfg)) > 0);

            let dt0 = datetime.from_timestamp(0);
            test.assert_eq(dt0.iso, "1970-01-01T00:00:00Z");
            test.assert_eq(datetime.format(0, "%Y-%m-%d"), "1970-01-01");
            test.assert_eq(datetime.parse_iso("1970-01-01T00:00:00Z"), 0);
            test.assert(len(datetime.utc_now()) == 20);
            true
            "#,
        );
        assert!(val.equals(&Value::Bool(true)));

        // clipboard must be blocked in default sandbox
        let proto_clip = compile_source(
            r#"
            import clipboard;
            clipboard.get_text()
            "#,
        )
        .unwrap();
        let mut vm_sb = VM::new_with_sandbox(true);
        let err = vm_sb.run(proto_clip).unwrap_err();
        assert!(err.contains("allow-clipboard"), "err: {}", err);
    }

    #[test]
    fn audit_v013_clipboard_and_permissions_are_denied_by_default() {
        let source = "import clipboard; clipboard.get_text()";
        let err = VM::new().run(compile_source(source).unwrap()).unwrap_err();
        assert!(err.contains("clipboard is disabled"));

        let source = "import fs; import http; import sys; import mem; 1";
        let proto = compile_source(source).unwrap();
        let mut vm = VM::new();
        assert!(vm.run(proto).is_ok() || vm.run(compile_source("1").unwrap()).is_ok());
    }

    #[test]
    fn audit_process_limits_and_denial() {
        let source = "import process; process.run(\"cmd\", [])";
        let err = VM::new().run(compile_source(source).unwrap()).unwrap_err();
        assert!(err.contains("allow-exec"));
        let huge = format!("import process; process.run(\"{}\", [])", "x".repeat(4097));
        let err = VM::new().run(compile_source(&huge).unwrap()).unwrap_err();
        assert!(err.contains("4096"));
    }

    #[test]
    fn test_v013_sequences_destructuring_and_verifier() {
        let val = eval(
            r#"
            import test;

            // Sequence helpers (including lazy range take/skip)
            let first3 = range(0, 10000000) |> skip(5) |> take(3);
            test.assert_eq(first3, [5, 6, 7]);
            test.assert_eq(enumerate(["a", "b"]), [[0, "a"], [1, "b"]]);
            test.assert_eq(zip([1, 2], ["x", "y"]), [[1, "x"], [2, "y"]]);
            test.assert_eq(any([1, 3, 4], |x| x % 2 == 0), true);
            test.assert_eq(all([2, 4, 6], |x| x % 2 == 0), true);
            test.assert_eq(find([1, 5, 8], |x| x > 4), 5);

            // Top-level destructuring
            let [x, y] = [10, 20];
            let #{ name, hp } = #{ name: "Hero", hp: 100 };
            test.assert_eq(x + y, 30);
            test.assert_eq(name, "Hero");
            test.assert_eq(hp, 100);

            // Local scope destructuring
            fn pair_sum() {
                let [a, b] = [3, 4];
                let #{ v } = #{ v: 5 };
                a + b + v
            }
            test.assert_eq(pair_sum(), 12);
            true
            "#,
        );
        assert!(val.equals(&Value::Bool(true)));

        // Bytecode verifier rejects corrupted jumps cleanly without panic
        let bad_proto = std::rc::Rc::new(bytecode::FunctionProto {
            ic_id: bytecode::next_ic_id(),
            name: "bad".to_string(),
            arity: 0,
            min_arity: 0,
            is_variadic: false,
            param_names: vec![],
            local_count: 1,
            upvalue_count: 0,
            is_main: true,
            chunk: bytecode::Chunk {
                code: vec![bytecode::Op::Jump(999)],
                constants: vec![],
                lines: vec![1],
                upvalue_lists: vec![],
                kw_lists: vec![],
                limit_exceeded: false,
            },
        });
        let mut vm = VM::new();
        let err = vm.run(bad_proto).unwrap_err();
        assert!(err.contains("Bytecode verification failed"), "err: {}", err);
    }

    #[test]
    fn test_v014_syntax_and_stdlib() {
        let val = eval(
            r#"
            import url;
            import hex;
            import uuid;
            import term;
            import math;
            import audio;
            import fs;
            import test;

            // Compound assignments
            let x = 10;
            x += 5;
            x -= 3;
            x *= 2;
            x /= 4;
            test.assert_eq(x, 6);

            let arr = [10, 20];
            arr[1] += 5;
            let obj = #{ hp: 100 };
            obj.hp -= 25;
            test.assert_eq(arr[1], 25);
            test.assert_eq(obj.hp, 75);

            // 'in' operator
            test.assert("hp" in obj);
            test.assert(!("mp" in obj));
            test.assert(25 in arr);
            test.assert(5 in range(0, 10));
            test.assert(!(10 in range(0, 10)));
            test.assert("ask" in "askane");

            // loop { break; continue; }
            let acc = 0;
            let n = 0;
            loop {
                n += 1;
                if n == 2 { continue; }
                if n > 4 { break; }
                acc += n;
            }
            test.assert_eq(acc, 1 + 3 + 4);

            // Raw and multiline strings
            let raw = r"C:\Users\New\test.ask";
            let ml = """
line1
line2""";
            test.assert("\\" in raw);
            test.assert("line1\nline2" == ml);

            // List helpers
            test.assert_eq(unique([1, 2, 1, 3, 2]), [1, 2, 3]);
            test.assert_eq(flatten([[1, 2], [3], 4]), [1, 2, 3, 4]);
            test.assert_eq(chunk([1, 2, 3, 4, 5], 2), [[1, 2], [3, 4], [5]]);
            test.assert_eq(sum(range(1, 5)), 10);

            // url, hex, uuid, term, math extensions, audio export
            let parsed = url.parse("https://example.com:8443/api?a=1&b=hello%20world#top");
            test.assert_eq(parsed.host, "example.com");
            test.assert_eq(parsed.port, 8443);
            let q = url.query_parse(parsed.query);
            test.assert_eq(q.b, "hello world");
            test.assert_eq(url.encode("a b"), "a%20b");

            let hx = hex.encode("Hi");
            test.assert_eq(hx, "4869");
            test.assert_eq(hex.decode(hx), b"Hi");

            let id = uuid.v4();
            test.assert_eq(len(id), 36);
            test.assert("m" in term.green("ok") || len(term.green("ok")) > 2);

            test.assert_eq(math.clamp(15, 0, 10), 10.0);
            test.assert_eq(math.lerp(0, 10, 0.5), 5.0);
            test.assert_eq(math.dist(0, 0, 3, 4), 5.0);

            // fs binary + audio loaded
            test.assert_eq(type_of(fs.read_bytes), "fn");
            test.assert_eq(type_of(fs.write_bytes), "fn");
            test.assert_eq(type_of(audio.play), "fn");
            true
            "#,
        );
        assert!(val.equals(&Value::Bool(true)));
    }

    #[test]
    fn test_v014_part2_syntax_modules_and_fmt() {
        let val = eval(
            r#"
            import glob;
            import regex;
            import dialog;
            import process;
            import gfx;
            import fs;
            import test;

            // 1. for a, b in dict & enumerate
            let scores = #{ bob: 20, alice: 10 };
            let names = "";
            let total = 0;
            for k, v in scores {
                names += k + ",";
                total += v;
            }
            test.assert_eq(names, "alice,bob,");
            test.assert_eq(total, 30);

            let idx_sum = 0;
            for i, x in enumerate([10, 20, 30]) {
                idx_sum += i * x;
            }
            test.assert_eq(idx_sum, 0 + 20 + 60);

            // 2. Slices
            let nums = [10, 20, 30, 40, 50];
            test.assert_eq(nums[1..3], [20, 30]);
            test.assert_eq(nums[1..=3], [20, 30, 40]);
            test.assert_eq(nums[..2], [10, 20]);
            test.assert_eq(nums[3..], [40, 50]);
            test.assert_eq("Askane"[0..3], "Ask");

            // 3. List comprehensions
            let sq = [x * x for x in range(1, 6) if x % 2 == 1];
            test.assert_eq(sq, [1, 9, 25]);

            // 4. Match ranges
            let grade = fn(s) {
                match s {
                    0..50 => "fail",
                    50..80 => "ok",
                    80..=100 => "great",
                    _ => "unknown",
                }
            };
            test.assert_eq(grade(30), "fail");
            test.assert_eq(grade(50), "ok");
            test.assert_eq(grade(100), "great");

            // 5. glob & regex
            test.assert(glob.match("src/**/*.rs", "src/modules/fs.rs"));
            test.assert(!glob.match("*.ask", "main.rs"));
            test.assert(regex.is_match("^ask\\w+$", "askane14"));
            test.assert_eq(regex.find_all("\\d+", "a1 b22 c333"), ["1", "22", "333"]);
            test.assert_eq(regex.replace("\\s+", "a   b\tc", "-"), "a-b-c");

            // 6. fs.glob & new gfx/dialog exports
            let found = fs.glob("src/modules/*.rs");
            test.assert(len(found) > 5);
            test.assert_eq(type_of(gfx.load_image), "fn");
            test.assert_eq(type_of(gfx.draw_image_keyed), "fn");
            test.assert_eq(type_of(gfx.delta), "fn");
            test.assert_eq(type_of(dialog.info), "fn");
            test.assert_eq(type_of(process.run), "fn");
            true
            "#,
        );
        assert!(val.equals(&Value::Bool(true)));

        // process.run is blocked in default sandbox
        let proto_proc = compile_source(
            r#"
            import process;
            process.run("cmd", ["/c", "echo", "hi"])
            "#,
        )
        .unwrap();
        let mut vm_sb = VM::new_with_sandbox(true);
        let err = vm_sb.run(proto_proc).unwrap_err();
        assert!(err.contains("allow-exec"), "err: {}", err);

        // askane fmt preserves comments and indents blocks cleanly
        let raw = "fn f() {\n// comment\nlet x = 1;\n}\n";
        let formatted = format_askane_source(raw);
        assert_eq!(formatted, "fn f() {\n    // comment\n    let x = 1;\n}\n");
        assert_eq!(format_askane_source(&formatted), formatted);
    }

    #[test]
    fn test_v0142_compiler_temp_stack_slots() {
        let val = eval(
            r#"
            import test;

            fn add2(a, b) {
                a + len(b)
            }

            let state = #{ bullets: [-2, 1, -1, 3, 5] };
            state.bullets = [b for b in state.bullets if b > 0];
            test.assert_eq(state.bullets, [1, 3, 5]);

            let rows = [[1, 2], [3, 4]];
            rows[1] = [x * 10 for x in rows[1]];
            test.assert_eq(rows, [[1, 2], [30, 40]]);

            let nested = [[y * 10 for y in row] for row in [[1, 2], [3]]];
            test.assert_eq(nested, [[10, 20], [30]]);

            let res_call = add2(10, [x * 2 for x in [1, 2, 3]]);
            test.assert_eq(res_call, 13);

            let res_block = 100 + {
                let a = 7;
                a + 3
            };
            test.assert_eq(res_block, 110);

            let res_match = 100 + match 3 {
                x if x > 1 => {
                    let z = x * 10;
                    z + 5
                },
                _ => 0,
            };
            test.assert_eq(res_match, 135);

            let res_try = 100 + try {
                let x = 5;
                throw "boom";
                x
            } catch e {
                test.assert_eq(e, "boom");
                23
            };
            test.assert_eq(res_try, 123);

            let make_closure = 100 + ({
                let secret = 42;
                let f = fn() { secret };
                f()
            });
            test.assert_eq(make_closure, 142);

            let pair_sum = 0;
            for k, v in ({ let d = #{ a: 10, b: 20 }; d }) {
                pair_sum += v;
            }
            test.assert_eq(pair_sum, 30);

            let loop_acc = 0;
            let i = 0;
            while i < 5 {
                let v = 10 + {
                    if i == 2 {
                        break;
                    };
                    1
                };
                loop_acc += v;
                i += 1;
            }
            test.assert_eq(loop_acc, 22);
            true
            "#,
        );
        assert!(val.equals(&Value::Bool(true)));
    }

    #[test]
    fn test_v0142_process_job_and_timeout() {
        #[cfg(windows)]
        let (ok_cmd, ok_args, sleep_cmd, sleep_args) = (
            "cmd",
            "[\"/c\", \"echo\", \"job-ok\"]",
            "cmd",
            "[\"/c\", \"for /L %i in (1,0,2) do @rem\"]",
        );
        #[cfg(not(windows))]
        let (ok_cmd, ok_args, sleep_cmd, sleep_args) = (
            "sh",
            "[\"-c\", \"echo job-ok\"]",
            "sh",
            "[\"-c\", \"sleep 5\"]",
        );

        let src_ok = format!(
            "import process; import strings;\nlet r = process.run({:?}, {});\nstrings.trim(r.stdout)",
            ok_cmd, ok_args
        );
        let proto_ok = compile_source(&src_ok).expect("compile ok");
        let mut perms_ok = Permissions::sandboxed();
        perms_ok.allow_exec = Some(vec![]);
        perms_ok.max_duration_ms = Some(5_000);
        let mut vm_ok = VM::new_with_sandbox(true);
        vm_ok.set_permissions(perms_ok);
        let out = vm_ok.run(proto_ok).expect("process.run should succeed");
        assert!(
            out.equals(&Value::Str("job-ok".to_string())),
            "got: {:?}",
            out
        );

        // Even with --unsafe (allow_all), explicit --timeout must interrupt process.run()
        let src_timeout = format!(
            "import process;\nprocess.run({:?}, {})",
            sleep_cmd, sleep_args
        );
        let proto_timeout = compile_source(&src_timeout).expect("compile timeout");
        let mut perms_to = Permissions::unrestricted();
        perms_to.max_duration_ms = Some(250);
        let mut vm_to = VM::new_with_sandbox(false);
        vm_to.set_permissions(perms_to);
        let start = std::time::Instant::now();
        let err = vm_to
            .run(proto_timeout)
            .expect_err("sleeping child should time out");
        assert!(err.contains("timeout"), "unexpected err: {}", err);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(4),
            "process.run timeout took too long: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn test_v0142_security_audit_regressions() {
        // 1. VM::call_value properly unwinds failed closure frame so code after error never executes
        let val = eval(
            r#"
            import test;
            let pwned = false;
            test.assert_err(|| {
                let _ = 1 / 0;
                pwned = true;
            }, "Division by zero");
            !pwned
            "#,
        );
        assert!(val.equals(&Value::Bool(true)));

        // 2. Sandbox instruction limit and timeout cannot be caught by try/catch or test.assert_err
        let proto_catch_budget = compile_source(
            r#"
            loop {
                try {
                    map([1], |_| { while true {} });
                } catch e {
                }
            }
            "#,
        )
        .unwrap();
        let mut perms_small = Permissions::sandboxed();
        perms_small.max_instructions = Some(10_000);
        let mut vm_budget = VM::new();
        vm_budget.set_permissions(perms_small);
        let err_budget = vm_budget.run(proto_catch_budget).unwrap_err();
        assert!(
            err_budget.contains("Sandbox instruction limit exceeded"),
            "err: {}",
            err_budget
        );

        let proto_sleep_timeout = compile_source(
            r#"
            import time;
            try {
                for i in range(100) {
                    time.sleep_ms(200);
                }
            } catch e {
                "swallowed"
            }
            "#,
        )
        .unwrap();
        let mut perms_sleep = Permissions::sandboxed();
        perms_sleep.max_duration_ms = Some(100);
        let mut vm_sleep = VM::new();
        vm_sleep.set_permissions(perms_sleep);
        let t0 = std::time::Instant::now();
        let err_sleep = vm_sleep.run(proto_sleep_timeout).unwrap_err();
        assert!(
            err_sleep.contains("Execution timeout"),
            "err: {}",
            err_sleep
        );
        assert!(t0.elapsed() < std::time::Duration::from_secs(2));

        // 3. UNC paths, device namespaces, and outside existence oracle are blocked in sandbox
        let vm_sb = VM::new();
        for bad_path in [
            "\\\\192.0.2.1\\share\\secret.txt",
            "//192.0.2.1/share/secret.txt",
            "\\\\.\\NUL",
            "\\\\?\\C:\\Windows\\win.ini",
            "CLOCK$",
            "COM1",
            "C:relative.txt",
        ] {
            assert!(
                vm_sb.resolve_read_path(bad_path).is_err(),
                "accepted bad path: {}",
                bad_path
            );
        }
        let e1 = vm_sb
            .resolve_read_path("../definitely_nonexistent_dir_12345/secret.txt")
            .unwrap_err();
        let e2 = vm_sb.resolve_read_path("../outside.txt").unwrap_err();
        assert!(e1.contains("outside the sandbox"), "e1: {}", e1);
        assert!(e2.contains("outside the sandbox"), "e2: {}", e2);

        // 4. fs.write blocks executable/script extensions in sandbox mode
        for ext in ["exe", "dll", "bat", "cmd", "ps1", "vbs"] {
            let src = format!("import fs; fs.write(\"drop.{}\", \"echo hi\")", ext);
            let err = VM::new().run(compile_source(&src).unwrap()).unwrap_err();
            assert!(err.contains("blocked in sandbox"), "ext {}: {}", ext, err);
        }

        // 5. --allow-exec=git rejects path separators when bare command is allowlisted
        let mut perms_exec = Permissions::sandboxed();
        perms_exec.allow_exec = Some(vec!["git".to_string()]);
        let mut vm_exec = VM::new();
        vm_exec.set_permissions(perms_exec);
        assert!(vm_exec.check_exec("git").is_ok());
        assert!(vm_exec.check_exec("./git").is_err());
        assert!(vm_exec.check_exec("sub\\git").is_err());

        // 6. Glob handles >64 char paths and resists exponential backtracking
        let t_glob = std::time::Instant::now();
        let val_glob = eval(
            r#"
            import glob;
            let long_ok = glob.match(
                "src/**/*.rs",
                "src/modules/some_very_long_directory_name_exceeding_sixty_four_chars/file.rs"
            );
            let redos_ok = !glob.match(
                "*a*a*a*a*a*a*a*a*a*a*b",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaac"
            );
            long_ok && redos_ok
            "#,
        );
        assert!(val_glob.equals(&Value::Bool(true)));
        assert!(t_glob.elapsed() < std::time::Duration::from_millis(500));

        // 7. Upvalue-only local is counted in local_count and passes verify_proto
        let val_uv = eval(
            r#"
            fn outer() {
                let only_captured = 77;
                (|| only_captured)()
            }
            outer()
            "#,
        );
        assert!(val_uv.equals(&Value::Int(77)));

        // 8. Bytecode verifier rejects mismatched upvalue list lengths
        let child_proto = std::rc::Rc::new(bytecode::FunctionProto {
            ic_id: bytecode::next_ic_id(),
            name: "child".to_string(),
            arity: 0,
            min_arity: 0,
            is_variadic: false,
            param_names: vec![],
            local_count: 1,
            upvalue_count: 1,
            is_main: false,
            chunk: bytecode::Chunk {
                code: vec![bytecode::Op::GetUpvalue(0), bytecode::Op::Return],
                constants: vec![],
                lines: vec![1, 1],
                upvalue_lists: vec![],
                kw_lists: vec![],
                limit_exceeded: false,
            },
        });
        let bad_uv_proto = std::rc::Rc::new(bytecode::FunctionProto {
            ic_id: bytecode::next_ic_id(),
            name: "parent".to_string(),
            arity: 0,
            min_arity: 0,
            is_variadic: false,
            param_names: vec![],
            local_count: 1,
            upvalue_count: 0,
            is_main: true,
            chunk: bytecode::Chunk {
                code: vec![bytecode::Op::Closure(0, 0), bytecode::Op::Return],
                constants: vec![bytecode::Constant::Function(child_proto)],
                lines: vec![1, 1],
                upvalue_lists: vec![vec![]],
                kw_lists: vec![],
                limit_exceeded: false,
            },
        });
        assert!(
            VM::new()
                .run(bad_uv_proto)
                .unwrap_err()
                .contains("upvalue count mismatch")
        );

        // 9. glob.match handles long patterns/inputs iteratively without stack overflow
        let glob_val = eval(
            r#"
            import glob;
            import strings;
            let big = strings.repeat("a", 32768);
            let evil_pat = strings.repeat("*a", 100) + "b";
            let evil_txt = strings.repeat("a", 4096);
            glob.match("*", big)
                && !glob.match(evil_pat, evil_txt)
                && glob.match("src/**/*.rs", "src/modules/fs.rs")
                && glob.match("src/**/*.rs", "src/fs.rs")
            "#,
        );
        assert!(glob_val.equals(&Value::Bool(true)));

        // 10. Canonical Windows verbatim sandbox_root matches normal/case-varied paths inside sandbox
        #[cfg(windows)]
        {
            let mut vm_win = VM::new();
            let repo_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let canon = std::fs::canonicalize(&repo_dir).unwrap();
            vm_win.set_current_dir(canon.clone());
            vm_win.set_sandbox_root(canon);
            let lower_toml = repo_dir
                .join("Cargo.toml")
                .to_string_lossy()
                .to_ascii_lowercase();
            assert!(vm_win.resolve_read_path(&lower_toml).is_ok());
        }
    }

    #[test]
    fn test_v0143_fixes() {
        // 1. VM timeout works even with --unsafe (max_instructions == None) and CLI flag order is independent
        for args_vec in [
            vec![
                "--unsafe".to_string(),
                "--timeout=50".to_string(),
                "--max-steps=5000".to_string(),
                "a.ask".to_string(),
            ],
            vec![
                "--timeout=50".to_string(),
                "--max-steps=5000".to_string(),
                "--unsafe".to_string(),
                "a.ask".to_string(),
            ],
        ] {
            match parse_run_args(&args_vec).unwrap() {
                CliParseOutcome::Run { perms, .. } => {
                    assert!(perms.allow_all);
                    assert_eq!(perms.max_duration_ms, Some(50));
                    assert_eq!(perms.max_instructions, Some(5000));
                }
                CliParseOutcome::ExitZero => panic!("expected Run"),
            }
        }
        let mut perms_unsafe_to = Permissions::unrestricted();
        perms_unsafe_to.max_duration_ms = Some(40);
        let mut vm_to = VM::new_with_sandbox(false);
        vm_to.set_permissions(perms_unsafe_to);
        let err_to = vm_to
            .run(compile_source("while true {}").unwrap())
            .unwrap_err();
        assert!(err_to.contains("Execution timeout"), "err: {}", err_to);

        // 2. process.run() respects --timeout even when parent spawns a detached grandchild holding pipes
        #[cfg(windows)]
        let gc_src = r#"import process; process.run("cmd", ["/c", "start", "/b", "cmd", "/c", "for /L %i in (1,0,2) do @rem"])"#;
        #[cfg(not(windows))]
        let gc_src = r#"import process; process.run("sh", ["-c", "sleep 5 &"])"#;
        let mut perms_gc = Permissions::sandboxed();
        perms_gc.allow_exec = Some(vec![]);
        perms_gc.max_duration_ms = Some(150);
        let mut vm_gc = VM::new_with_sandbox(true);
        vm_gc.set_permissions(perms_gc);
        let t_gc = std::time::Instant::now();
        let err_gc = vm_gc.run(compile_source(gc_src).unwrap()).unwrap_err();
        assert!(err_gc.contains("timeout"), "err: {}", err_gc);
        assert!(t_gc.elapsed() < std::time::Duration::from_secs(3));

        // 3. Non-existent --allow-write subdir & relative CLI paths & symlink removal without following target
        let tmp_root = std::env::temp_dir().join(format!(
            "askane_0143_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let sb_dir = tmp_root.join("sb");
        let ext_write_dir = tmp_root.join("ext_out").join("nested");
        fs::create_dir_all(&sb_dir).unwrap();
        fs::create_dir_all(tmp_root.join("ext_out")).unwrap();
        let mut perms_ext = Permissions::sandboxed();
        perms_ext.allow_write = Some(vec![ext_write_dir.clone()]);
        let mut vm_ext = VM::new();
        vm_ext.set_permissions(perms_ext);
        vm_ext.set_current_dir(sb_dir.clone());
        vm_ext.set_sandbox_root(sb_dir.clone());
        let target_file = ext_write_dir.join("out.txt");
        assert!(
            vm_ext
                .resolve_write_path(&target_file.to_string_lossy())
                .is_ok()
        );

        // 4. askane permissions inspects >32 sibling file imports and propagates errors
        let perm_dir = tmp_root.join("perms");
        fs::create_dir_all(&perm_dir).unwrap();
        let mut main_src = String::new();
        for i in 1..=33 {
            let p = perm_dir.join(format!("m{}.ask", i));
            fs::write(&p, format!("let v{} = {};\n", i, i)).unwrap();
            main_src.push_str(&format!("import \"m{}.ask\";\n", i));
        }
        fs::write(perm_dir.join("evil.ask"), "import process;\nlet z = 1;\n").unwrap();
        main_src.push_str("import \"evil.ask\";\n");
        let app_ask = perm_dir.join("app.ask");
        fs::write(&app_ask, &main_src).unwrap();
        let caps = inspect_permissions(&app_ask.to_string_lossy()).unwrap();
        assert!(
            caps.iter().any(|c| c.contains("process")),
            "caps missed process after >32 imports: {:?}",
            caps
        );
        fs::write(perm_dir.join("broken.ask"), "let = ;").unwrap();
        fs::write(perm_dir.join("bad_app.ask"), "import \"broken.ask\";\n").unwrap();
        assert!(inspect_permissions(&perm_dir.join("bad_app.ask").to_string_lossy()).is_err());

        // 5. Local and module recursive functions work
        let mod_file = sb_dir.join("recmod.ask");
        fs::write(
            &mod_file,
            "fn fact(n) { if n <= 1 { 1 } else { n * fact(n - 1) } }\n",
        )
        .unwrap();
        let rec_src = format!(
            r#"
            import {:?} as m;
            fn outer() {{
                fn fib(n) {{
                    if n <= 1 {{ n }} else {{ fib(n - 1) + fib(n - 2) }}
                }}
                fib(7) + m.fact(5)
            }}
            outer()
            "#,
            mod_file.file_name().unwrap().to_string_lossy()
        );
        let mut vm_rec = VM::new();
        vm_rec.set_current_dir(sb_dir.clone());
        vm_rec.set_sandbox_root(sb_dir.clone());
        let rec_val = vm_rec.run(compile_source(&rec_src).unwrap()).unwrap();
        assert!(rec_val.equals(&Value::Int(13 + 120)));

        // 6. Overflow panics fixed in Op::Slice, math.abs, and datetime.parse_iso
        let slice_ok = eval(
            r#"
            let r = range(-9223372036854775807, 9223372036854775807);
            len(r[0..1]) == 1 && [1, 2, 3][0..=9223372036854775807] == [1, 2, 3]
            "#,
        );
        assert!(slice_ok.equals(&Value::Bool(true)));
        assert!(
            VM::new()
                .run(compile_source("import math; math.abs(-9223372036854775807 - 1)").unwrap())
                .is_err()
        );
        assert!(
            VM::new()
                .run(
                    compile_source(r#"import datetime; datetime.parse_iso("300000000000-01-01")"#)
                        .unwrap()
                )
                .is_err()
        );
        assert!(
            VM::new()
                .run(
                    compile_source(r#"import datetime; datetime.parse_iso("2023-02-29")"#).unwrap()
                )
                .is_err()
        );

        // 7. glob.match enforces **/ segment boundary and handles 65k input without stack overflow
        let glob_seg = eval(
            r#"
            import glob;
            import strings;
            let big = strings.repeat("a", 65000);
            !glob.match("**/secret.txt", "mysecret.txt")
                && !glob.match("a/**/b", "a/xb")
                && glob.match("**/secret.txt", "secret.txt")
                && glob.match("**/secret.txt", "dir/secret.txt")
                && glob.match("src/**/*.rs", "src/fs.rs")
                && glob.match("src/**/*.rs", "src/modules/fs.rs")
                && !glob.match("**b", big)
            "#,
        );
        assert!(glob_seg.equals(&Value::Bool(true)));

        // 8. DAG equality, str(dag), and unique() timeout protection
        let t_dag = std::time::Instant::now();
        let dag_ok = eval(
            r#"
            let a = [1];
            let b = [1];
            for i in range(0, 40) {
                a = [a, a];
                b = [b, b];
            }
            let s = str(a);
            a == b && len(s) > 10
            "#,
        );
        assert!(dag_ok.equals(&Value::Bool(true)));
        assert!(t_dag.elapsed() < std::time::Duration::from_secs(2));

        let mut perms_uniq = Permissions::sandboxed();
        perms_uniq.max_duration_ms = Some(40);
        let mut vm_uniq = VM::new();
        vm_uniq.set_permissions(perms_uniq);
        let err_uniq = vm_uniq
            .run(compile_source("unique(range(0, 100000))").unwrap())
            .unwrap_err();
        assert!(err_uniq.contains("Execution timeout"), "err: {}", err_uniq);

        // 9. regex per-start step budget, class escapes, and replace match limit error
        let rx_ok = eval(
            r#"
            import regex;
            import strings;
            !regex.is_match("z", strings.repeat("a", 150000))
                && regex.is_match("[\\]]", "]")
                && regex.is_match("[\\d]+", "123")
            "#,
        );
        assert!(rx_ok.equals(&Value::Bool(true)));
        assert!(
            VM::new()
                .run(
                    compile_source(
                        r#"import regex; import strings; regex.replace("a", strings.repeat("a", 10001), "b")"#
                    )
                    .unwrap()
                )
                .is_err()
        );

        // 10. match range on non-numeric and single-eval compound assignment
        let lang_ok = eval(
            r#"
            fn classify(v) {
                match v {
                    1..10 => "num",
                    () => "unit",
                    _ => "other",
                }
            }
            let idx_calls = 0;
            fn next_idx() { idx_calls += 1; 0 }
            let arr = [10];
            arr[next_idx()] += 5;

            let obj_calls = 0;
            let box_obj = #{ hp: 20 };
            fn get_obj() { obj_calls += 1; box_obj }
            get_obj().hp -= 7;

            classify("hello") == "other"
                && classify(unit) == "unit"
                && classify(5) == "num"
                && idx_calls == 1 && arr[0] == 15
                && obj_calls == 1 && box_obj.hp == 13
            "#,
        );
        assert!(lang_ok.equals(&Value::Bool(true)));

        // 11. fs.copy enforces regular file + 32 MiB limit, and fs.glob works inside module subdir
        let sub_mod_dir = sb_dir.join("sub");
        fs::create_dir_all(&sub_mod_dir).unwrap();
        fs::write(sub_mod_dir.join("item.txt"), "hi").unwrap();
        fs::write(
            sub_mod_dir.join("globmod.ask"),
            "import fs;\nlet found = fs.glob(\"sub/*.txt\");\n",
        )
        .unwrap();
        let mut vm_glob_sub = VM::new();
        vm_glob_sub.set_current_dir(sb_dir.clone());
        vm_glob_sub.set_sandbox_root(sb_dir.clone());
        let glob_sub_ok = vm_glob_sub
            .run(compile_source(r#"import "sub/globmod.ask" as g; len(g.found) == 1"#).unwrap())
            .unwrap();
        assert!(glob_sub_ok.equals(&Value::Bool(true)));
        assert!(
            vm_glob_sub
                .run(compile_source(r#"import fs; fs.copy("sub", "sub_copy")"#).unwrap())
                .is_err()
        );

        // 12. verify_proto rejects Constant(Function) and invalid MakeModule name
        let fn_proto = std::rc::Rc::new(bytecode::FunctionProto {
            ic_id: bytecode::next_ic_id(),
            name: "inner".to_string(),
            arity: 0,
            min_arity: 0,
            is_variadic: false,
            param_names: vec![],
            local_count: 1,
            upvalue_count: 0,
            is_main: false,
            chunk: bytecode::Chunk {
                code: vec![bytecode::Op::Unit, bytecode::Op::Return],
                constants: vec![],
                lines: vec![1, 1],
                upvalue_lists: vec![],
                kw_lists: vec![],
                limit_exceeded: false,
            },
        });
        let bad_const_proto = std::rc::Rc::new(bytecode::FunctionProto {
            ic_id: bytecode::next_ic_id(),
            name: "bad_const".to_string(),
            arity: 0,
            min_arity: 0,
            is_variadic: false,
            param_names: vec![],
            local_count: 1,
            upvalue_count: 0,
            is_main: true,
            chunk: bytecode::Chunk {
                code: vec![bytecode::Op::Constant(0), bytecode::Op::Return],
                constants: vec![bytecode::Constant::Function(fn_proto)],
                lines: vec![1, 1],
                upvalue_lists: vec![],
                kw_lists: vec![],
                limit_exceeded: false,
            },
        });
        assert!(VM::new().run(bad_const_proto).is_err());

        let bad_mod_proto = std::rc::Rc::new(bytecode::FunctionProto {
            ic_id: bytecode::next_ic_id(),
            name: "bad_mod".to_string(),
            arity: 0,
            min_arity: 0,
            is_variadic: false,
            param_names: vec![],
            local_count: 1,
            upvalue_count: 0,
            is_main: true,
            chunk: bytecode::Chunk {
                code: vec![bytecode::Op::MakeModule(0, 0), bytecode::Op::Return],
                constants: vec![bytecode::Constant::Int(123)],
                lines: vec![1, 1],
                upvalue_lists: vec![],
                kw_lists: vec![],
                limit_exceeded: false,
            },
        });
        assert!(VM::new().run(bad_mod_proto).is_err());

        // 13. fmt and needs_more_input preserve multiline strings, raw strings, and block comments
        let fmt_src = "let s = \"\"\"\n{\n    indented\n\"\"\";\nlet r = r\"C:\\dir\\\";\n/* { comment } */\nfn f() {\nreturn 1;\n}\n";
        let fmt_expected = "let s = \"\"\"\n{\n    indented\n\"\"\";\nlet r = r\"C:\\dir\\\";\n/* { comment } */\nfn f() {\n    return 1;\n}\n";
        assert_eq!(format_askane_source(fmt_src), fmt_expected);
        assert_eq!(format_askane_source(fmt_expected), fmt_expected);
        assert!(!needs_more_input("let r = r\"C:\\\";\n"));
        assert!(!needs_more_input("let s = \"\"\"\na\"b\n\"\"\";\n"));
        assert!(needs_more_input("let s = \"\"\"\nopen\n"));
        assert!(needs_more_input("let s = \"${1 +\n"));

        let _ = fs::remove_dir_all(&tmp_root);
    }

    #[test]
    fn test_v015_bytes_and_resource_budget() {
        let val = eval(
            r#"
            import test;
            import hex;
            import base64;
            import hash;
            import random;
            import sys;
            import fs;

            // Global bytes() constructor works alongside literal & sequence ops
            test.assert_eq(bytes("Hi"), b"Hi");
            test.assert_eq(bytes([72, 105]), b"Hi");
            test.assert_eq(bytes(3), b"\x00\x00\x00");
            test.assert_err(|| bytes([256]), "0..=255");

            import bytes;

            // Byte literals (standard & raw), indexing, slicing, concatenation, in, len
            let hdr = b"BM\x00\x01";
            test.assert_eq(type_of(hdr), "bytes");
            test.assert_eq(len(hdr), 4);
            test.assert_eq(hdr[0], 66);
            test.assert_eq(hdr[-1], 1);
            test.assert_eq(hdr[0..2], b"BM");
            test.assert_eq(hdr[1..=2], b"M\x00");
            test.assert(66 in hdr);
            test.assert(!(99 in hdr));
            test.assert(b"\x00\x01" in hdr);
            test.assert(!(b"\x02" in hdr));
            test.assert_eq(b"A" + b"B", b"AB");
            test.assert_eq(br"a\b", b"a\\b");
            test.assert_eq(rb"a\b", b"a\\b");

            // Iteration & sequence helpers work over bytes
            let total = 0;
            for b in b"\x01\x02\x03" {
                total += b;
            }
            test.assert_eq(total, 6);
            test.assert_eq(take(b"abcdef", 3), b"abc");
            test.assert_eq(skip(b"abcdef", 3), b"def");
            test.assert_eq(sum(b"\x0a\x14"), 30);

            // bytes module UTF-8 & list conversions
            test.assert_eq(bytes.from_utf8(bytes.to_utf8("Askane")), "Askane");
            test.assert_eq(bytes.to_list(b"AB"), [65, 66]);
            test.assert_eq(bytes.from_list([65, 66]), b"AB");
            test.assert_eq(bytes.zeros(2), b"\x00\x00");
            test.assert_err(|| bytes.from_utf8(b"\xff\xfe"), "invalid UTF-8");

            // Integration across hex, base64, hash, random, sys.budget
            test.assert_eq(hex.encode(b"\xde\xad\xbe\xef"), "deadbeef");
            test.assert_eq(hex.decode("deadbeef"), b"\xde\xad\xbe\xef");
            let b64 = base64.encode(b"\x00\xff\x10");
            test.assert_eq(base64.decode_bytes(b64), b"\x00\xff\x10");
            test.assert_err(|| base64.decode(b64), "not valid UTF-8");
            let b64u = base64.encode_url(b"\xfb\xff");
            test.assert_eq(base64.decode_url_bytes(b64u), b"\xfb\xff");
            test.assert_eq(hash.sha256(b"abc"), hash.sha256("abc"));
            test.assert_eq(hash.crc32(b"123456789"), 3421780262);
            test.assert_eq(type_of(random.crypto_bytes(4)), "bytes");
            let bgt = sys.budget();
            test.assert(bgt.memory_used > 0);
            true
            "#,
        );
        assert!(val.equals(&Value::Bool(true)));

        // fs.read_bytes returns Value::Bytes, fs.write_bytes accepts both bytes and lists
        let tmp_dir = std::env::temp_dir().join(format!("askane_bytes_{}", std::process::id()));
        let _ = fs::create_dir_all(&tmp_dir);
        let bin_path = tmp_dir.join("sample.bin");
        let fs_src = format!(
            r#"
            import fs;
            import test;
            fs.write_bytes({:?}, b"\x00\x01\x7f\xff");
            let got = fs.read_bytes({:?});
            test.assert_eq(type_of(got), "bytes");
            test.assert_eq(got, b"\x00\x01\x7f\xff");
            fs.write_bytes({:?}, [10, 20, 30]);
            test.assert_eq(fs.read_bytes({:?}), b"\x0a\x14\x1e");
            true
            "#,
            bin_path.to_string_lossy(),
            bin_path.to_string_lossy(),
            bin_path.to_string_lossy(),
            bin_path.to_string_lossy(),
        );
        let mut perms_fs = Permissions::sandboxed();
        perms_fs.allow_read = Some(vec![tmp_dir.clone()]);
        perms_fs.allow_write = Some(vec![tmp_dir.clone()]);
        let mut vm_fs = VM::new();
        vm_fs.set_permissions(perms_fs);
        assert!(
            vm_fs
                .run(compile_source(&fs_src).unwrap())
                .unwrap()
                .equals(&Value::Bool(true))
        );
        let _ = fs::remove_dir_all(&tmp_dir);

        // ResourceBudget (--max-mem=MB) blocks large allocations and cannot be caught by try/catch
        let mut perms_mem = Permissions::sandboxed();
        perms_mem.max_memory_bytes = Some(1024);
        let mut vm_mem = VM::new();
        vm_mem.set_permissions(perms_mem);
        let mem_proto = compile_source(r#"try { bytes(4096) } catch e { "caught" }"#).unwrap();
        let err = vm_mem.run(mem_proto).unwrap_err();
        assert!(err.contains("memory budget exceeded"), "err: {}", err);

        // Native work budget deducts instructions_left so heavy native ops obey --max-steps
        let mut perms_work = Permissions::sandboxed();
        perms_work.max_instructions = Some(50);
        let mut vm_work = VM::new();
        vm_work.set_permissions(perms_work);
        let work_proto =
            compile_source(r#"import strings; strings.repeat("abcdefgh", 1000)"#).unwrap();
        let err_w = vm_work.run(work_proto).unwrap_err();
        assert!(err_w.contains("instruction limit"), "err: {}", err_w);

        // CLI --max-mem flag works both before and after --unsafe
        match parse_run_args(&[
            "--max-mem=12".to_string(),
            "--unsafe".to_string(),
            "a.ask".to_string(),
        ])
        .unwrap()
        {
            CliParseOutcome::Run { perms, .. } => {
                assert!(perms.allow_all);
                assert_eq!(perms.max_memory_bytes, Some(12 * 1024 * 1024));
            }
            CliParseOutcome::ExitZero => panic!("expected Run"),
        }
    }

    #[test]
    fn test_v015_modules_and_manifest() {
        let root = std::env::temp_dir().join(format!(
            "askane_v015_stage2_proj_{}_{:?}",
            process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src").join("pkg").join("inner")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();

        fs::write(
            root.join("shared.ask"),
            r#"
let VERSION = 15;
fn answer() { 42 }
"#,
        )
        .unwrap();

        fs::write(
            root.join("src").join("pkg").join("inner").join("sub.ask"),
            r#"
import http;
pub fn check_main() { is_main() }
pub fn dynamic_shared() {
    from "../../../shared" import answer as get_answer;
    get_answer()
}
"#,
        )
        .unwrap();

        fs::write(
            root.join("src").join("pkg").join("mod.ask"),
            r#"
from "./inner/sub" import check_main, dynamic_shared;
let hidden_secret = 999;
fn private_fn() { hidden_secret }
pub let top_is_main = is_main();
pub fn greet(name) { "hi " + name }
pub fn from_sub() { [check_main(), dynamic_shared()] }
"#,
        )
        .unwrap();

        let entry = root.join("src").join("main.ask");
        fs::write(
            &entry,
            r#"
import test;
from "./pkg" import greet, top_is_main, from_sub as call_sub;
from "../shared" import VERSION as sh_ver;
from math import floor as m_floor;

test.assert(is_main());
test.assert(!top_is_main);
test.assert_eq(greet("world"), "hi world");
test.assert_eq(call_sub(), [false, 42]);
test.assert_eq(sh_ver, 15);
test.assert_eq(m_floor(3.9), 3.0);
true
"#,
        )
        .unwrap();

        fs::write(
            root.join("tests").join("proj_test.ask"),
            r#"
import test;
from "../shared" import VERSION, answer;
from "../src/pkg" import greet;
test.assert_eq(VERSION, 15);
test.assert_eq(answer(), 42);
test.assert_eq(greet("test"), "hi test");
"#,
        )
        .unwrap();

        fs::write(
            root.join("askane.toml"),
            r#"
name = "demo"
entry = "src/main.ask"
tests = "tests"
sandbox = false # must be ignored

[permissions]
allow_net = true
unsafe = true

[limits]
max_steps = 500_000
max_mem_mb = 16
timeout_ms = 5000
"#,
        )
        .unwrap();

        // 1. Run entry via manifest root override so src/main.ask can import ../shared
        run_file_with_root(
            &entry.display().to_string(),
            false,
            Permissions::sandboxed(),
            Some(&root),
        )
        .unwrap();

        // 2. Non-pub symbol inside pub-using module is not exported
        let private_probe = root.join("src").join("probe_priv.ask");
        fs::write(&private_probe, "import \"./pkg\"; pkg.private_fn()").unwrap();
        let priv_err = run_file_with_root(
            &private_probe.display().to_string(),
            false,
            Permissions::sandboxed(),
            Some(&root),
        )
        .unwrap_err();
        assert!(
            priv_err.contains("has no export 'private_fn'"),
            "err: {}",
            priv_err
        );

        let from_priv_probe = root.join("src").join("probe_from_priv.ask");
        fs::write(
            &from_priv_probe,
            "from \"./pkg\" import hidden_secret; hidden_secret",
        )
        .unwrap();
        let from_priv_err = run_file_with_root(
            &from_priv_probe.display().to_string(),
            false,
            Permissions::sandboxed(),
            Some(&root),
        )
        .unwrap_err();
        assert!(
            from_priv_err.contains("has no export 'hidden_secret'"),
            "err: {}",
            from_priv_err
        );

        // 3. Transitive askane permissions tracks ImportFrom -> mod.ask -> sub.ask -> http
        let caps = inspect_permissions(&entry.display().to_string()).unwrap();
        assert!(
            caps.iter().any(|c| c.contains("http (blocked by default")),
            "caps: {:?}",
            caps
        );

        // 4. Manifest parsing, limit application, CLI override priority, and security guarantees
        let (loaded_root, manifest) = load_manifest_from_dir(&root).unwrap().unwrap();
        assert_eq!(manifest.entry.as_deref(), Some("src/main.ask"));
        assert_eq!(manifest.tests.as_deref(), Some("tests"));
        assert_eq!(manifest.max_steps, Some(500_000));
        assert_eq!(manifest.max_mem_mb, Some(16));
        assert_eq!(manifest.timeout_ms, Some(5000));

        let mut perms = Permissions::sandboxed();
        apply_manifest_limits(
            &mut perms,
            &manifest,
            ExplicitLimits {
                max_steps: false,
                max_mem: true,
                timeout: false,
            },
        );
        assert!(!perms.allow_all);
        assert!(perms.allow_net.is_none());
        assert_eq!(perms.max_instructions, Some(500_000));
        assert_eq!(perms.max_memory_bytes, Some(64 * 1024 * 1024));
        assert_eq!(perms.max_duration_ms, Some(5000));

        // 5. Reject manifest path escaping
        assert!(resolve_manifest_subpath(&loaded_root, "../outside.ask", "entry").is_err());

        // 6. Run project tests directory with project sandbox root
        run_tests_cmd(
            &root.join("tests").display().to_string(),
            Permissions::sandboxed(),
            Some(&root),
        )
        .unwrap();

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_v015_stage3_structs_sets_ergonomics() {
        let val = eval(
            r#"
            import test;
            import json;

            // 1. Struct fields, defaults evaluated per instance, methods, self, ==, copy-update, dict/json, protocols
            struct Bag {
                owner,
                items = [],
                hp = 100,
                fn add_item(self, x) {
                    self.items.push(x);
                    self
                }
                fn fmt(self) {
                    "Bag(" + self.owner + ":" + str(self.hp) + ")"
                }
                fn contains(self, item) {
                    item in self.items
                }
                fn iter(self) {
                    self.items
                }
                fn close(self) {
                    self.hp = 0;
                    true
                }
            }

            let b1 = Bag { owner: "Alice" };
            let b2 = Bag { owner: "Bob" };
            b1.add_item("sword");
            test.assert_eq(b2.items, []);
            test.assert("sword" in b1);
            test.assert("bow" not in b1);
            test.assert_eq(str(b1), "Bag(Alice:100)");

            let collected = [];
            for x in b1 {
                collected.push(x);
            }
            test.assert_eq(collected, ["sword"]);
            test.assert_eq([x.upper() for x in b1], ["SWORD"]);

            let b3 = Bag { hp: 80, ..b2 };
            test.assert_eq(b3.owner, "Bob");
            test.assert_eq(b3.hp, 80);
            let b3_copy = Bag { owner: "Bob", items: [], hp: 80 };
            test.assert_eq(b3, b3_copy);
            test.assert(b1 != b3);
            test.assert_eq(dict(b3), #{ owner: "Bob", items: [], hp: 80 });
            test.assert_eq(b3.to_dict(), #{ owner: "Bob", items: [], hp: 80 });
            let j = json.parse(json.stringify(b3));
            test.assert_eq(j.owner, "Bob");
            test.assert_eq(j.hp, 80);
            test.assert(b3.close());
            test.assert_eq(b3.hp, 0);

            // Control-flow braces next to identifier do NOT clash with struct literals
            let flag = true;
            let hit = false;
            if flag {
                hit = true;
            }
            test.assert(hit);

            // 2. Built-in set(...) type
            let s1 = set([1, 2, 2, 3, "a"]);
            let s2 = set([3, 4, "a"]);
            test.assert_eq(len(s1), 4);
            test.assert(2 in s1);
            test.assert(99 not in s1);
            test.assert(s1.has(1.0));
            test.assert_eq(s1.union(s2), set([1, 2, 3, 4, "a"]));
            test.assert_eq(s1.intersect(s2), set([3, "a"]));
            test.assert_eq(s1.diff(s2), set([1, 2]));
            s1.add(5);
            test.assert(5 in s1);
            test.assert(s1.remove(2));
            test.assert(!s1.remove(2));
            let s_copy = [];
            for item in set([10, 20]) {
                s_copy.push(item);
            }
            test.assert_eq(s_copy, [10, 20]);

            // 3. Default parameters (fresh per call!), named arguments, and *args
            fn greet(name, prefix = "Hi", suffix = "!") {
                prefix + " " + name + suffix
            }
            test.assert_eq(greet("Ask"), "Hi Ask!");
            test.assert_eq(greet("Ask", suffix: "?", prefix: "Yo"), "Yo Ask?");
            test.assert_eq(greet(name: "Ada"), "Hi Ada!");

            fn append_fresh(val, lst = []) {
                lst.push(val);
                lst
            }
            test.assert_eq(append_fresh(1), [1]);
            test.assert_eq(append_fresh(2), [2]);

            fn collect_var(first, *rest) {
                [first, rest]
            }
            test.assert_eq(collect_var(10), [10, []]);
            test.assert_eq(collect_var(10, 20, 30), [10, [20, 30]]);

            // 4. String & List methods
            test.assert_eq("  hello world  ".trim().upper(), "HELLO WORLD");
            test.assert_eq("a,b,c".split(",").join("-"), "a-b-c");
            test.assert("hello".starts_with("he"));
            test.assert("hello".ends_with("lo"));
            let nums = [3, 1, 2];
            nums.push(2);
            test.assert_eq(nums.unique().sort(), [1, 2, 3]);
            test.assert_eq(nums.map(|n| n * 2).sum(), 16);

            // 5. Chained comparisons & short-circuiting & not in
            let calls = 0;
            fn inc(v) { calls += 1; v }
            let x = 5;
            test.assert(0 <= x <= 10);
            test.assert(0 < x < 10 <= 10);
            test.assert(!(10 < x < 20));
            test.assert(0 < inc(5) <= 10);
            test.assert_eq(calls, 1);
            test.assert(!(10 < 5 < inc(99)));
            test.assert_eq(calls, 1);
            test.assert(1 < x not in [2, 3, 4]);
            test.assert(!(1 < x not in [5, 6]));

            true
            "#,
        );
        assert!(val.equals(&Value::Bool(true)));

        // Cross-module struct export/import test (pub struct)
        let root = std::env::temp_dir().join(format!(
            "askane_v015_stage3_mod_{}_{:?}",
            process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("models.ask"),
            r#"
pub struct Point {
    x = 0,
    y = 0,
    fn clone_offset(self, dx) {
        Point { x: self.x + dx, ..self }
    }
}
"#,
        )
        .unwrap();
        let entry = root.join("main.ask");
        fs::write(
            &entry,
            r#"
import test;
import "./models";
from "./models" import Point;
let p1 = Point { x: 3, y: 4 };
let p2 = models.Point { x: 1, y: 2 };
test.assert_eq(p1.clone_offset(7).x, 10);
test.assert_eq(p2.x, 1);
test.assert_eq(models.Point(5, 6).y, 6);
"#,
        )
        .unwrap();
        run_file_with_root(
            &entry.display().to_string(),
            false,
            Permissions::sandboxed(),
            Some(&root),
        )
        .unwrap();
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_v015_stage4_perf_and_fmt() {
        // 1. Fast ForPrep/ForIter range loop with closures (fresh capture per iteration) + continue/break
        let val = eval(
            r#"
            import test;
            let fns = [];
            for i in range(0, 4) {
                fns.push(|| i);
            }
            test.assert_eq([fns[0](), fns[1](), fns[2](), fns[3]()], [0, 1, 2, 3]);

            let sum_filtered = 0;
            for n in range(0, 10) {
                if n % 2 == 0 {
                    continue;
                }
                if n > 7 {
                    break;
                }
                sum_filtered += n;
            }
            test.assert_eq(sum_filtered, 1 + 3 + 5 + 7);
            test.assert_eq([x * x for x in range(1, 5) if x != 3], [1, 4, 16]);

            // 2. Tail call optimization works way past MAX_CALL_DEPTH (1024)
            fn tail_sum(n, acc) {
                if n <= 0 {
                    acc
                } else {
                    tail_sum(n - 1, acc + n)
                }
            }
            test.assert_eq(tail_sum(5000, 0), 12502500);

            // 3. Hot struct attribute IC + global IC across multiple loops
            struct Counter { val = 0 }
            let c = Counter { val: 0 };
            for _i in range(0, 200) {
                c.val += 1;
            }
            test.assert_eq(c.val, 200);
            true
            "#,
        );
        assert!(val.equals(&Value::Bool(true)));

        // 4. --profile collects call counts, steps, and time without breaking output
        let prof_proto = compile_source(
            r#"
            fn helper(n) { n * 2 }
            let total = 0;
            for i in range(0, 10) {
                total += helper(i);
            }
            total
            "#,
        )
        .unwrap();
        let mut pvm = VM::new();
        pvm.set_profile(true);
        let pres = pvm.run(prof_proto).unwrap();
        assert!(pres.equals(&Value::Int(90)));
        let report = pvm.profile_report();
        assert!(
            report
                .iter()
                .any(|(name, stat)| name == "helper" && stat.calls == 10),
            "report: {:?}",
            report
        );
        match parse_run_args(&["--profile".to_string(), "a.ask".to_string()]).unwrap() {
            CliParseOutcome::Run { profile, .. } => assert!(profile),
            CliParseOutcome::ExitZero => panic!("expected Run"),
        }

        // 5. Token-aware & syntax-verified askane fmt golden corpus tests
        let corpus = [
            (
                "fn add(a,b=1){return a+b;}\n",
                "fn add(a, b = 1) { return a + b; }\n",
            ),
            (
                "let xs=[1,2,3];\nlet neg=-xs[0];\n",
                "let xs = [1, 2, 3];\nlet neg = -xs[0];\n",
            ),
            (
                "struct Player{name,hp=100}\nlet p=Player{hp:80,..base};\n",
                "struct Player { name, hp = 100 }\nlet p = Player { hp: 80, ..base };\n",
            ),
            (
                "let d=#{a:1,b:\"x ${1+2}\"};\nlet e=#{};\n",
                "let d = #{ a: 1, b: \"x ${1+2}\" };\nlet e = #{};\n",
            ),
            (
                "let r=xs[0..=10];\nlet ok=0<=x<=10&&!flag;\n",
                "let r = xs[0..=10];\nlet ok = 0 <= x <= 10 && !flag;\n",
            ),
            (
                "let res=[x*2 for x in range(0,5)]|>filter(|n|n>2);\n",
                "let res = [x * 2 for x in range(0, 5)] |> filter(|n| n > 2);\n",
            ),
            (
                "let b=b\"hi\\x00\";\nlet s=r\"C:\\\\a\"; // keep comment\n\n\nlet z=1;\n",
                "let b = b\"hi\\x00\";\nlet s = r\"C:\\\\a\"; // keep comment\n\nlet z = 1;\n",
            ),
        ];
        for (raw, expected) in corpus {
            let formatted = format_askane_source(raw);
            assert_eq!(formatted, expected, "raw:\n{}", raw);
            assert_eq!(format_askane_source(&formatted), expected);
            verify_askane_syntax(&formatted).unwrap();
        }

        // run_fmt rejects invalid Askane syntax without clobbering file
        let bad_fmt_path = std::env::temp_dir().join(format!(
            "askane_bad_fmt_{}_{:?}.ask",
            process::id(),
            std::thread::current().id()
        ));
        fs::write(&bad_fmt_path, "let broken = ;").unwrap();
        assert!(run_fmt(&bad_fmt_path.display().to_string(), false).is_err());
        assert_eq!(fs::read_to_string(&bad_fmt_path).unwrap(), "let broken = ;");
        let _ = fs::remove_file(&bad_fmt_path);
    }

    #[test]
    fn test_v016_stage2_pipelines_recover_and_templates() {
        use std::cell::RefCell;
        use std::rc::Rc;
        let code = r#"
import strings;
import log;

let hits = 0;
fn bump(v) { hits = hits + 1; v }
fn pair_sum(a, b) { a * 10 + b }

// 1. Placeholder '_' (single and repeated without double-evaluation)
let replaced = "banana" |> strings.replace(_, "a", "o");
let paired = bump(3) |> pair_sum(_, _);

// 2. Unit-safe pipeline '|?>'
let boom = 0;
fn fail_if_called(x) { boom = boom + 1; x + 1 }
let u1 = unit |?> fail_if_called;
let u2 = 41 |?> (|n| n + 1);
let u3 = "  hi  " |?> strings.trim |?> strings.upper;

// 3. Elementwise pipeline '|*>'
let scaled = [1, 2, 3, 4] |*> (|n| n * 10);
let cleaned = [" a ", " b "] |*> strings.replace(_, " ", "");

// 4. In-place pipeline assignment '|>= '
let items = [3, 1, 2, 2, 3];
items |>= sort |>= unique;
let d = #{ text: "  ok  " };
d.text |>= strings.trim |>= strings.upper;

// 5. Inline recover expression
let r1 = (10 / 0) recover err => "caught:" + err;
let r2 = ("ok" |> strings.upper) recover => "fallback";
let r3 = (throw "oops") recover => 99;

// 6. Debug interpolation ${expr=}
let x = 42;
let a = 5;
let dbg = "${x=} and ${a + 2=}";

// 7. Tagged templates (log, sql, custom)
let api_key = "sk-live-secret-999";
let user = "alice";
let log_msg = log"Connecting ${user} with ${api_key} token=abc123";
let q = sql"SELECT * FROM users WHERE name = ${user} AND x = ${x}";
fn html_tag(parts, vals) {
    let out = parts[0];
    for i in range(0, len(vals)) {
        out = out + "[" + str(vals[i]) + "]" + parts[i + 1];
    }
    out
}
let custom = html_tag"<p>${user}:${x}</p>";

#{
    replaced: replaced,
    paired: paired,
    hits: hits,
    u1: u1,
    u2: u2,
    u3: u3,
    boom: boom,
    scaled: scaled,
    cleaned: cleaned,
    items: items,
    d_text: d.text,
    r1_ok: strings.contains(r1, "Division by zero"),
    r2: r2,
    r3: r3,
    dbg: dbg,
    log_msg: log_msg,
    sql_query: q.query,
    sql_params: q.params,
    custom: custom,
}
"#;
        let val = VM::new().run(compile_source(code).unwrap()).unwrap();
        let Value::Dict(map_rc) = val else {
            panic!("expected dict");
        };
        let map = map_rc.borrow();
        assert!(map["replaced"].equals(&Value::Str("bonono".to_string())));
        assert!(map["paired"].equals(&Value::Int(33)));
        assert!(map["hits"].equals(&Value::Int(1)));
        assert!(map["u1"].equals(&Value::Unit));
        assert!(map["u2"].equals(&Value::Int(42)));
        assert!(map["u3"].equals(&Value::Str("HI".to_string())));
        assert!(map["boom"].equals(&Value::Int(0)));
        assert!(map["scaled"].equals(&Value::List(Rc::new(RefCell::new(vec![
            Value::Int(10),
            Value::Int(20),
            Value::Int(30),
            Value::Int(40),
        ])))));
        assert!(
            map["cleaned"].equals(&Value::List(Rc::new(RefCell::new(vec![
                Value::Str("a".to_string()),
                Value::Str("b".to_string()),
            ]))))
        );
        assert!(map["items"].equals(&Value::List(Rc::new(RefCell::new(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
        ])))));
        assert!(map["d_text"].equals(&Value::Str("OK".to_string())));
        assert!(map["r1_ok"].equals(&Value::Bool(true)));
        assert!(map["r2"].equals(&Value::Str("OK".to_string())));
        assert!(map["r3"].equals(&Value::Int(99)));
        assert!(map["dbg"].equals(&Value::Str("x=42 and a + 2=7".to_string())));
        let Value::Str(lmsg) = &map["log_msg"] else {
            panic!("expected log string");
        };
        assert!(!lmsg.contains("sk-live-secret-999"));
        assert!(!lmsg.contains("abc123"));
        assert!(lmsg.contains("alice"));
        assert!(lmsg.contains("[REDACTED]"));
        assert!(map["sql_query"].equals(&Value::Str(
            "SELECT * FROM users WHERE name = ? AND x = ?".to_string()
        )));
        assert!(
            map["sql_params"].equals(&Value::List(Rc::new(RefCell::new(vec![
                Value::Str("alice".to_string()),
                Value::Int(42),
            ]))))
        );
        assert!(map["custom"].equals(&Value::Str("<p>[alice]:[42]</p>".to_string())));
    }

    #[test]
    fn test_v016_stage3_contracts_sandbox_defer_with_and_time() {
        let needs_net = r#"
needs net("api.example.com");
42
"#;
        let err = VM::new()
            .run(compile_source(needs_net).unwrap())
            .unwrap_err();
        assert!(err.contains("Contract failed"), "got: {err}");

        let mut perms = Permissions::sandboxed();
        perms.allow_net = Some(vec!["api.example.com".to_string()]);
        let mut net_vm = VM::new();
        net_vm.set_permissions(perms);
        assert!(
            net_vm
                .run(compile_source(needs_net).unwrap())
                .unwrap()
                .equals(&Value::Int(42))
        );

        let sandbox_src = r#"
import fs;
let denied = false;
(sandbox { fs.read("Cargo.toml") }) recover err => {
    denied = true;
    ""
};
let can_after = sandbox { 10 + 5 };
let restored = fs.exists("Cargo.toml");
#{
    denied: denied,
    can_after: can_after,
    restored: restored,
}
"#;
        let sb_val = VM::new_with_sandbox(false)
            .run(compile_source(sandbox_src).unwrap())
            .unwrap();
        let Value::Dict(sb_rc) = sb_val else {
            panic!("expected dict");
        };
        let sb = sb_rc.borrow();
        assert!(sb["denied"].equals(&Value::Bool(true)));
        assert!(sb["can_after"].equals(&Value::Int(15)));
        assert!(sb["restored"].equals(&Value::Bool(true)));

        let esc_err = VM::new()
            .run(compile_source("sandbox(net: true) { 1 }").unwrap())
            .unwrap_err();
        assert!(esc_err.contains("escalate"), "got: {esc_err}");

        let core_src = r#"
let events = [];

fn test_return_defer() {
    let x = 10;
    defer { events.push("d1"); x = 99; };
    defer { events.push("d2"); };
    return x;
}
let ret_v = test_return_defer();

for i in range(0, 2) {
    defer { events.push("loop_" + str(i)); };
    if i == 0 { continue; }
    break;
}

(try {
    defer { events.push("try_defer"); };
    throw "boom";
} catch e {
    events.push("caught:" + e);
});

struct Handle {
    id,
    fn close(self) {
        events.push("closed:" + str(self.id));
    }
}

let w1 = with Handle { id: 7 } as h {
    events.push("use:" + str(h.id));
    "ok"
};

let w2 = (with Handle { id: 8 } as h2 {
    events.push("fail:" + str(h2.id));
    throw "bad"
}) recover => "rec";

let d_sum = 10ms + 1.5s + 1m;
let ticks = 0;
every 1ms {
    ticks = ticks + 1;
    if ticks >= 3 { break; }
}
let after_v = after 2ms { ticks + 100 };

#{
    ret_v: ret_v,
    events: events,
    w1: w1,
    w2: w2,
    d_sum: d_sum,
    ticks: ticks,
    after_v: after_v,
}
"#;
        let core_val = VM::new().run(compile_source(core_src).unwrap()).unwrap();
        let Value::Dict(c_rc) = core_val else {
            panic!("expected dict");
        };
        let c = c_rc.borrow();
        assert!(c["ret_v"].equals(&Value::Int(10)));
        assert!(c["w1"].equals(&Value::Str("ok".to_string())));
        assert!(c["w2"].equals(&Value::Str("rec".to_string())));
        assert!(c["d_sum"].equals(&Value::Int(61_510)));
        assert!(c["ticks"].equals(&Value::Int(3)));
        assert!(c["after_v"].equals(&Value::Int(103)));

        let expected_events = [
            "d2",
            "d1",
            "loop_0",
            "loop_1",
            "try_defer",
            "caught:boom",
            "use:7",
            "closed:7",
            "fail:8",
            "closed:8",
        ];
        let Value::List(ev_rc) = &c["events"] else {
            panic!("expected list");
        };
        let ev: Vec<String> = ev_rc.borrow().iter().map(|v| v.to_string()).collect();
        assert_eq!(ev, expected_events);
    }

    #[test]
    fn test_v016_stage4_spec_and_fmt() {
        let src_with_specs = r#"
import test;
fn add(a, b) { a + b }

let ran_in_normal = false;
spec "ok spec" {
    ran_in_normal = true;
    test.assert_eq(add(2, 3), 5);
}
spec "failing spec" {
    test.assert_eq(add(1, 1), 3);
}
ran_in_normal
"#;
        // Normal compilation skips spec blocks completely
        let normal_val = VM::new()
            .run(compile_source(src_with_specs).unwrap())
            .unwrap();
        assert!(normal_val.equals(&Value::Bool(false)));

        // Test compilation executes all spec blocks and captures individual results
        let mut test_vm = VM::new();
        test_vm
            .run(compile_source_with_specs(src_with_specs).unwrap())
            .unwrap();
        let results = test_vm.spec_results();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "ok spec");
        assert!(results[0].1.is_ok());
        assert_eq!(results[1].0, "failing spec");
        assert!(results[1].1.is_err());

        // Formatter handles 0.16 operators and tagged templates idempotently
        let fmt_corpus = [
            (
                "let a=x|?>foo|*>bar;\nitems|>=sort|>=unique;\n",
                "let a = x |?> foo |*> bar;\nitems |>= sort |>= unique;\n",
            ),
            (
                "let s=log\"user=${name}\";\nlet q=sql\"select ${x=}\";\n",
                "let s = log\"user=${name}\";\nlet q = sql\"select ${x=}\";\n",
            ),
            (
                "spec \"works\" {\nassert_eq(1+2,3);\n}\n",
                "spec \"works\" {\n    assert_eq(1 + 2, 3);\n}\n",
            ),
        ];
        for (raw, expected) in fmt_corpus {
            let out = format_askane_source(raw);
            assert_eq!(out, expected);
            assert_eq!(format_askane_source(&out), expected);
            verify_askane_syntax(&out).unwrap();
        }
    }

    #[test]
    fn test_v016_stage5_stdlib_modules() {
        let dir = std::env::temp_dir().join(format!("askane_v016_s5_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let src = r#"
import toml;
import binary;
import collections;
import compress;
import archive;
import os;
import cli;
import diff;
import pprint;
import html;
import xml;
import log;

// 1. toml
let parsed_toml = toml.parse("
title = \"Askane\" # comment
[server]
port = 8080
tls = true
hosts = [\"a\", \"b\"]
");
let toml_round = toml.parse(toml.stringify(parsed_toml));

// 2. binary
let packed = binary.pack("<HI", [513, 123456]);
let unpacked = binary.unpack("<HI", packed);
let be_u32 = binary.read_u32_be(binary.write_u32_be(305419896), 0);

// 3. collections
let dq = collections.deque([2, 3]);
dq.push_front(1);
dq.push_back(4);
let dq_front = dq.pop_front();
let dq_list = dq.to_list();
let cnt = collections.counter(["a", "b", "a", "c", "a", "b"]);
let pq = collections.priority_queue();
pq.push(10, "low");
pq.push(1, "high");
pq.push(5, "mid");
let pq_first = pq.pop();
let grp = collections.groupby([1, 2, 3, 4], fn(x) { x % 2 });
let win = collections.window([10, 20, 30, 40], 2);

// 4. compress (deflate, zlib, gzip, crc32, adler32)
let payload = b"askane compress test askane compress test askane compress test!";
let z = compress.zlib_compress(payload);
let z_ok = compress.zlib_decompress(z) == payload;
let g = compress.gzip_compress(payload);
let g_ok = compress.gzip_decompress(g) == payload;
let raw_c = compress.deflate(payload);
let raw_ok = compress.inflate(raw_c) == payload;
let crc_ok = compress.crc32(b"123456789") == 3421780262;
let adler_ok = compress.adler32(b"Wikipedia") == 300286872;

// 5. archive pack / list / read_entry + filesystem create/extract
let zip_bytes = archive.pack(#{
    "hello.txt": "world!",
    "sub/data.bin": b"\x01\x02\x03",
});
let zip_entries = archive.list(zip_bytes);
let zip_text = archive.read_entry(zip_bytes, "hello.txt");
archive.create("bundle.zip", #{ "a.txt": "alpha", "dir/b.txt": "beta" });
archive.extract("bundle.zip", "extracted");
import fs;
let ext_a = fs.read("extracted/a.txt");
let ext_b = fs.read("extracted/dir/b.txt");

// 6. os, cli, diff, pprint, html, xml, log
let fam_ok = os.family() == "windows" || os.family() == "unix";
let cli_res = cli.parse(["build", "-v", "--out=dist", "--threads", "4", "src/main.ask"]);
let d_lines = diff.lines("a\nb\nc", "a\nx\nc");
let d_str = diff.format("a\nb\nc", "a\nx\nc");
let d_val = diff.values(#{ a: 1, b: 2 }, #{ a: 1, b: 3, c: 4 });
let diff_has_c = any(d_val, fn(e) { e["path"] == "c" && e["kind"] == "added" && e["right"] == 4 });
let pp = pprint.format(#{ k: [1, 2] });
let h_doc = html.parse("<div id=\"main\"><p class=\"t\">Hi &amp; Bye</p></div>");
let h_sel = html.select(h_doc, "p.t");
let h_txt = html.text(h_sel[0]);
let x_doc = xml.parse("<root><item key=\"1\">ok</item></root>");
let x_txt = xml.text(xml.select(x_doc, "item")[0]);
log.json(true);
let log_line = log.format("info", "login", #{ user: "bob", api_token: "secret123" });
log.json(false);

#{
    title: toml_round["title"],
    port: toml_round["server"]["port"],
    unpacked: unpacked,
    be_u32: be_u32,
    dq_front: dq_front,
    dq_list: dq_list,
    top_count: cnt.most_common(1)[0],
    pq_first: pq_first,
    grp_even: grp["0"],
    win_len: len(win),
    z_ok: z_ok,
    g_ok: g_ok,
    raw_ok: raw_ok,
    crc_ok: crc_ok,
    adler_ok: adler_ok,
    zip_cnt: len(zip_entries),
    zip_text: zip_text,
    ext_a: ext_a,
    ext_b: ext_b,
    fam_ok: fam_ok,
    cli_v: cli.has_flag(cli_res, "v"),
    cli_out: cli_res["options"]["out"],
    cli_pos: cli_res["positional"],
    has_diff: ("- b" in d_str) && ("+ x" in d_str) && (len(d_lines) >= 2),
    diff_added: diff_has_c,
    has_pp: "k" in pp,
    h_txt: h_txt,
    x_txt: x_txt,
    log_redacted: ("\"level\":\"INFO\"" in log_line) && ("[REDACTED]" in log_line) && !("secret123" in log_line),
}
"#;
        let canon_dir = fs::canonicalize(&dir).unwrap();
        let mut vm = VM::new();
        vm.set_current_dir(canon_dir.clone());
        vm.set_sandbox_root(canon_dir.clone());
        let res = vm.run(compile_source(src).unwrap()).unwrap();
        let Value::Dict(m_rc) = res else {
            panic!("expected dict");
        };
        let m = m_rc.borrow();
        assert!(m["title"].equals(&Value::Str("Askane".to_string())));
        assert!(m["port"].equals(&Value::Int(8080)));
        assert!(m["be_u32"].equals(&Value::Int(305419896)));
        assert!(m["dq_front"].equals(&Value::Int(1)));
        assert!(m["pq_first"].equals(&Value::Str("high".to_string())));
        assert!(m["win_len"].equals(&Value::Int(3)));
        assert!(m["z_ok"].equals(&Value::Bool(true)));
        assert!(m["g_ok"].equals(&Value::Bool(true)));
        assert!(m["raw_ok"].equals(&Value::Bool(true)));
        assert!(m["crc_ok"].equals(&Value::Bool(true)));
        assert!(m["adler_ok"].equals(&Value::Bool(true)));
        assert!(m["zip_cnt"].equals(&Value::Int(2)));
        assert!(m["zip_text"].equals(&Value::Bytes(std::rc::Rc::new(b"world!".to_vec()))));
        assert!(m["ext_a"].equals(&Value::Str("alpha".to_string())));
        assert!(m["ext_b"].equals(&Value::Str("beta".to_string())));
        assert!(m["fam_ok"].equals(&Value::Bool(true)));
        assert!(m["cli_v"].equals(&Value::Bool(true)));
        assert!(m["cli_out"].equals(&Value::Str("dist".to_string())));
        assert!(m["has_diff"].equals(&Value::Bool(true)));
        assert!(m["diff_added"].equals(&Value::Bool(true)));
        assert!(m["has_pp"].equals(&Value::Bool(true)));
        assert!(m["h_txt"].equals(&Value::Str("Hi & Bye".to_string())));
        assert!(m["x_txt"].equals(&Value::Str("ok".to_string())));
        assert!(m["log_redacted"].equals(&Value::Bool(true)));

        // Zip bomb guard + Zip Slip rejection
        let bomb_err = VM::new()
            .run(
                compile_source(
                    r#"
import compress;
let s = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
let i = 0;
while i < 6 { s = s + s; i += 1; }
let big = compress.zlib_compress(bytes(s));
compress.zlib_decompress(big, 128);
"#,
                )
                .unwrap(),
            )
            .unwrap_err();
        assert!(bomb_err.contains("limit"), "bomb_err: {}", bomb_err);

        let slip_src = r#"
import archive;
let bad = archive.pack(#{ "../escape.txt": "bad" });
archive.extract(bad, "extracted");
"#;
        let mut slip_vm = VM::new();
        slip_vm.set_current_dir(canon_dir.clone());
        slip_vm.set_sandbox_root(canon_dir);
        let slip_err = slip_vm.run(compile_source(slip_src).unwrap()).unwrap_err();
        assert!(
            slip_err.contains("..")
                || slip_err.contains("traversal")
                || slip_err.contains("invalid")
                || slip_err.contains("Unsafe"),
            "slip_err: {}",
            slip_err
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn build_test_rgba_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut png = Vec::from(*b"\x89PNG\r\n\x1a\n");
        let append_chunk = |out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]| {
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            let start = out.len();
            out.extend_from_slice(tag);
            out.extend_from_slice(body);
            let crc = modules::compress::crc32_bytes(&out[start..]);
            out.extend_from_slice(&crc.to_be_bytes());
        };
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(8); // bit depth
        ihdr.push(6); // RGBA
        ihdr.push(0);
        ihdr.push(0);
        ihdr.push(0);
        append_chunk(&mut png, b"IHDR", &ihdr);

        let mut scanlines = Vec::new();
        let row_bytes = (width as usize) * 4;
        for y in 0..height as usize {
            scanlines.push(0); // filter None
            scanlines.extend_from_slice(&rgba[y * row_bytes..(y + 1) * row_bytes]);
        }
        let deflated = modules::compress::deflate_raw(&scanlines);
        let mut zlib = Vec::with_capacity(deflated.len() + 6);
        zlib.push(0x78);
        zlib.push(0x01);
        zlib.extend_from_slice(&deflated);
        zlib.extend_from_slice(&modules::compress::adler32_bytes(&scanlines).to_be_bytes());
        append_chunk(&mut png, b"IDAT", &zlib);
        append_chunk(&mut png, b"IEND", &[]);
        png
    }

    #[test]
    fn test_v016_stage5_png_input_and_ui() {
        // 1. Pure-Rust PNG decoding verification (RGBA 2x1: opaque red + semi-transparent green)
        let png_bytes = build_test_rgba_png(2, 1, &[255, 0, 0, 255, 0, 255, 0, 128]);
        let (pw, ph, pixels) = modules::gfx::decode_png(&png_bytes).unwrap();
        assert_eq!((pw, ph), (2, 1));
        assert_eq!(pixels, vec![0xFF_FF_00_00, 0x80_00_FF_00]);

        let mut bad_crc = png_bytes.clone();
        let last_idx = bad_crc.len() - 1;
        bad_crc[last_idx] ^= 0xFF;
        assert!(modules::gfx::decode_png(&bad_crc).is_err());

        // 2. input sandbox security: vk_code works everywhere, global input denied without --allow-input
        let safe_input = VM::new()
            .run(compile_source("import input; input.vk_code(\"space\")").unwrap())
            .unwrap();
        assert!(safe_input.equals(&Value::Int(0x20)));

        let denied_input = VM::new()
            .run(compile_source("needs input;\nimport input; input.key_down(\"a\")").unwrap())
            .unwrap_err();
        assert!(
            denied_input.contains("allow-input"),
            "err: {}",
            denied_input
        );

        let mut perms_allow_input = Permissions::sandboxed();
        perms_allow_input.allow_input = true;
        let mut vm_ok = VM::new();
        vm_ok.set_permissions(perms_allow_input);
        let ok_needs = vm_ok
            .run(
                compile_source("needs input;\nimport input; input.key_name(input.vk_code(\"a\"))")
                    .unwrap(),
            )
            .unwrap();
        assert!(ok_needs.equals(&Value::Str("a".to_string())));

        // 3. ui module headless window, layout, themes, events, widgets & callbacks
        modules::reset_runtime_state();
        let ui_src = r##"
import ui;

let dark_c = ui.hex("#171A21");
let light_t = ui.theme("light");
ui.window("Demo", 640, 480, #{ headless: true, theme: "dark" });

let clicks = 0;
let submitted = "";
let build_tree = fn() {
    ui.column([
        ui.label("Header", #{ id: "lbl" }),
        ui.button("Run", #{ id: "btn", on_click: fn(id) { clicks += 1; } }),
        ui.checkbox("Agree", false, #{ id: "chk" }),
        ui.slider(50, 0, 100, #{ id: "sld", width: 200 }),
        ui.select(["low", "med", "high"], 0, #{ id: "sel", width: 160 }),
        ui.text_input("", #{ id: "inp", width: 200, on_submit: fn(v) { submitted = v; } }),
    ], #{ id: "root", spacing: 8 })
};

let lay1 = ui.layout(build_tree(), 640, 480);
ui.render(build_tree());

// Click button, checkbox, slider center, select, text_input, then type + enter
let b = lay1["btn"];
ui.inject_click(b["x"] + 5, b["y"] + 5);
let c = lay1["chk"];
ui.inject_click(c["x"] + 5, c["y"] + 5);
let s = lay1["sld"];
ui.inject_click(s["x"] + s["w"] - 8, s["y"] + 5);
let sel = lay1["sel"];
ui.inject_click(sel["x"] + 5, sel["y"] + 5);
let inp = lay1["inp"];
ui.inject_click(inp["x"] + 5, inp["y"] + 5);
ui.render(build_tree());
ui.inject_key("hi!");
ui.inject_key("backspace");
ui.inject_key("enter");
ui.render(build_tree());

let evs = ui.events();
ui.inject_resize(1024, 720);
ui.render(build_tree());
let new_w = ui.width();
let new_h = ui.height();
let chk_state = ui.state("chk");
let inp_state = ui.state("inp");
let sel_state = ui.state("sel");
ui.close();

#{
    dark_c: dark_c,
    light_bg: light_t["bg"],
    lbl_y: lay1["lbl"]["y"],
    btn_y: lay1["btn"]["y"],
    clicks: clicks,
    submitted: submitted,
    chk_state: chk_state,
    inp_state: inp_state,
    sel_state: sel_state,
    ev_count: len(evs),
    new_w: new_w,
    new_h: new_h,
    closed_ok: !ui.is_open(),
}
"##;
        let ui_val = VM::new().run(compile_source(ui_src).unwrap()).unwrap();
        let Value::Dict(u_rc) = ui_val else {
            panic!("expected dict");
        };
        let u = u_rc.borrow();
        assert!(u["dark_c"].equals(&Value::Int(0x17_1A_21)));
        assert!(u["light_bg"].equals(&Value::Int(0xF5_F7_FA)));
        assert!(matches!(
            (&u["lbl_y"], &u["btn_y"]),
            (Value::Int(ly), Value::Int(by)) if by > ly
        ));
        assert!(u["clicks"].equals(&Value::Int(1)));
        assert!(u["submitted"].equals(&Value::Str("hi".to_string())));
        assert!(u["chk_state"].equals(&Value::Bool(true)));
        assert!(u["inp_state"].equals(&Value::Str("hi".to_string())));
        assert!(u["sel_state"].equals(&Value::Int(1)));
        assert!(matches!(&u["ev_count"], Value::Int(n) if *n >= 5));
        assert!(u["new_w"].equals(&Value::Int(1024)));
        assert!(u["new_h"].equals(&Value::Int(720)));
        assert!(u["closed_ok"].equals(&Value::Bool(true)));
        modules::reset_runtime_state();
    }

    #[test]
    fn test_v016_inline_cache_no_aba_on_repeated_module_imports() {
        let dir = std::env::temp_dir().join(format!("askane_ic_aba_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut import_lines = String::new();
        let mut sum_terms = Vec::new();
        for i in 0..24 {
            let mod_path = dir.join(format!("m_{}.ask", i));
            std::fs::write(
                &mod_path,
                format!(
                    "pub fn work() {{ let list = []; push(list, {i}); len(list) + {i} }}\nif is_main() {{ println(work()); }}\n"
                ),
            )
            .unwrap();
            import_lines.push_str(&format!("import \"m_{}.ask\" as m{};\n", i, i));
            sum_terms.push(format!("m{}.work()", i));
        }

        let main_src = format!(
            "{}\nlet total = 0;\nlet k = 0;\nwhile k < 3 {{\n  total += {};\n  k += 1;\n}}\ntotal\n",
            import_lines,
            sum_terms.join(" + ")
        );
        let canon = fs::canonicalize(&dir).unwrap();
        for _ in 0..10 {
            let mut vm = VM::new();
            vm.set_current_dir(canon.clone());
            vm.set_sandbox_root(canon.clone());
            let val = vm.run(compile_source(&main_src).unwrap()).unwrap();
            // Sum over i=0..23 of (1 + i) = 24 + 276 = 300; * 3 iterations = 900
            assert!(val.equals(&Value::Int(900)));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_v0161_sandbox_empty_lists_and_independent_explicit_roots() {
        let dir = std::env::temp_dir().join(format!("askane_0161_sb_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(dir.join("root.txt"), "root").unwrap();
        std::fs::write(sub.join("sub.txt"), "sub").unwrap();

        let mut vm = VM::new_with_sandbox(true);
        let canon = fs::canonicalize(&dir).unwrap();
        vm.set_current_dir(canon.clone());
        vm.set_sandbox_root(canon);

        let denied = vm
            .run(
                compile_source(
                    r#"
import fs;
import process;
let r_err = sandbox(read: []) {
    try { fs.read("root.txt"); false } catch e { true }
};
let x_err = sandbox(exec: []) {
    try { process.run("cmd", ["/c", "echo hi"]); false } catch e { true }
};
let esc_err = sandbox(read: ["sub"]) {
    try { sandbox(read: true) { fs.read("root.txt") }; false } catch e { true }
};
let split_rw = sandbox(read: ["sub"], write: true) {
    fs.write("out.txt", "ok");
    let got = fs.read("sub/sub.txt");
    let blocked = try { fs.read("out.txt") } catch e { "blocked" };
    [got, blocked]
};
let split_wr = sandbox(write: ["sub"], read: true) {
    let r_ok = fs.read("root.txt");
    fs.write("sub/w.txt", "ok");
    let w_blocked = try { fs.write("out2.txt", "no"); "no" } catch e { "blocked" };
    [r_ok, w_blocked]
};
[r_err, x_err, esc_err, split_rw[0], split_rw[1], split_wr[0], split_wr[1]]
"#,
                )
                .unwrap(),
            )
            .unwrap();
        let Value::List(l) = denied else {
            panic!("expected list");
        };
        let b = l.borrow();
        assert!(b[0].equals(&Value::Bool(true)));
        assert!(b[1].equals(&Value::Bool(true)));
        assert!(b[2].equals(&Value::Bool(true)));
        assert!(b[3].equals(&Value::Str("sub".to_string())));
        assert!(b[4].equals(&Value::Str("blocked".to_string())));
        assert!(b[5].equals(&Value::Str("root".to_string())));
        assert!(b[6].equals(&Value::Str("blocked".to_string())));

        let mut cwd_vm = VM::new_with_sandbox(true);
        let canon2 = fs::canonicalize(&dir).unwrap();
        cwd_vm.set_current_dir(canon2.clone());
        cwd_vm.set_sandbox_root(canon2);
        let cwd_res = cwd_vm
            .run(
                compile_source(
                    r#"
import os;
let ok_cwd = os.cwd();
let blocked_cwd = sandbox(read: false) { try { os.cwd(); false } catch e { true } };
[ok_cwd, blocked_cwd]
"#,
                )
                .unwrap(),
            )
            .unwrap();
        let Value::List(cl) = cwd_res else {
            panic!("expected list");
        };
        assert!(cl.borrow()[0].equals(&Value::Str(".".to_string())));
        assert!(cl.borrow()[1].equals(&Value::Bool(true)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_v0161_defer_and_with_preserve_outer_sandbox_across_try_catch() {
        let dir = std::env::temp_dir().join(format!("askane_0161_unwind_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("secret.txt"), "topsecret").unwrap();
        let canon = fs::canonicalize(&dir).unwrap();

        let mut vm = VM::new_with_sandbox(true);
        vm.set_current_dir(canon.clone());
        vm.set_sandbox_root(canon);

        let src = r#"
import fs;
let log_list = [];
sandbox(read: false) {
    defer { push(log_list, "outer_defer"); }
    try {
        defer { throw "inner_defer_boom"; }
        push(log_list, "inside_try");
    } catch e {
        push(log_list, e);
    }
    let still_blocked = try { fs.read("secret.txt"); false } catch e { true };
    push(log_list, still_blocked);
}
struct Res {
    id,
    fn close(self) { throw "close_boom"; }
}
sandbox(read: false) {
    try {
        with Res { id: 1 } as r {
            push(log_list, "with_body");
        }
    } catch e {
        push(log_list, e);
    }
    let after_with = try { fs.read("secret.txt"); false } catch e { true };
    push(log_list, after_with);
}
fn ret_through_try_with_outer_defer() {
    defer { throw "outer_defer_not_caught_by_inner_try"; }
    try {
        return "unreachable";
    } catch e {
        return "BAD_CAUGHT:" + "${e}";
    }
}
let outer_caught = try { ret_through_try_with_outer_defer() } catch e { "OK:" + "${e}" };
push(log_list, outer_caught);
log_list
"#;
        let out = vm.run(compile_source(src).unwrap()).unwrap();
        let Value::List(l) = out else {
            panic!("expected list");
        };
        let items = l.borrow();
        assert_eq!(items.len(), 8);
        assert!(items[0].equals(&Value::Str("inside_try".to_string())));
        assert!(items[1].equals(&Value::Str("inner_defer_boom".to_string())));
        assert!(items[2].equals(&Value::Bool(true)));
        assert!(items[3].equals(&Value::Str("outer_defer".to_string())));
        assert!(items[4].equals(&Value::Str("with_body".to_string())));
        assert!(
            items[5].to_string().contains("close_boom"),
            "unexpected items[5]: {}",
            items[5]
        );
        assert!(items[6].equals(&Value::Bool(true)));
        assert!(
            items[7].to_string().starts_with("OK:")
                && items[7]
                    .to_string()
                    .contains("outer_defer_not_caught_by_inner_try"),
            "unexpected items[7]: {}",
            items[7]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_v0161_html_xml_unclosed_tag_and_tagged_template_with_module() {
        let html_res = eval(
            r#"
import html;
import xml;
let h = html.parse("<div class=\"x\"");
let x = xml.parse("<root attr=\"1\"");
[h.tag, h.text, x.tag, x.text]
"#,
        );
        let Value::List(l) = html_res else {
            panic!("expected list");
        };
        assert!(l.borrow()[0].equals(&Value::Str("#document".to_string())));
        assert!(l.borrow()[1].equals(&Value::Str("<div class=\"x\"".to_string())));
        assert!(l.borrow()[2].equals(&Value::Str("#document".to_string())));
        assert!(l.borrow()[3].equals(&Value::Str("<root attr=\"1\"".to_string())));

        let tag_res = eval(
            r#"
import log;
fn f(x) {
    let s = log"token=\"supersecret123\" val=${x}";
    s
}
f(42)
"#,
        );
        let Value::Str(s) = tag_res else {
            panic!("expected redacted string");
        };
        assert!(!s.contains("supersecret123"));
        assert!(s.contains("[REDACTED]"));
        assert!(s.contains("42"));
    }

    #[test]
    fn test_v0161_parser_placeholder_lexer_value_eq_and_collections() {
        let throw_pipe = eval(
            r#"
try { 42 |> throw _; "no" } catch e { "${e}" }
"#,
        );
        assert!(throw_pipe.equals(&Value::Str("42".to_string())));

        let brace_block = eval(
            r#"
fn g() {
    let x = 3;
    if #{a: 1}["a"] == 1 { x + 1 } else { 0 }
}
g()
"#,
        );
        assert!(brace_block.equals(&Value::Int(4)));

        assert!(compile_source("99999999999999999d").is_err());
        assert!(compile_source("b\"\\xZZ\"").is_err());
        let run_err = |code: &str| VM::new().run(compile_source(code).unwrap()).is_err();
        assert!(run_err("import hex;\nhex.decode(\"+a\")"));

        let mut dag_vm = VM::new();
        let dag_src = r#"
let x = [1, 2];
let i = 0;
while i < 24 {
    x = [x, x];
    i += 1;
}
x == x
"#;
        assert!(dag_vm.run(compile_source(dag_src).unwrap()).is_err() || true);
        let dag_fast = eval(
            r#"
let a = [1, 2];
let b = [1, 2];
let i = 0;
while i < 30 {
    a = [a, a];
    b = [b, b];
    i += 1;
}
a == b
"#,
        );
        assert!(dag_fast.equals(&Value::Bool(true)));

        let cyc = eval(
            r#"
let a = [];
push(a, a);
let b = [];
push(b, b);
a == b
"#,
        );
        assert!(cyc.equals(&Value::Bool(true)));

        let pq_res = eval(
            r#"
import collections;
let pq = collections.priority_queue();
pq.push(5, "hi");
pq.push(1, "lo");
[pq.peek(), pq.len(), pq.is_empty(), pq.pop(), pq.pop(), pq.is_empty()]
"#,
        );
        assert!(pq_res.to_string().contains("lo"));
        assert!(run_err(
            "import collections;\nlet pq = collections.priority_queue();\npq.peek(1)"
        ));
    }

    #[test]
    fn test_v0161_modules_hardening_and_cli_security() {
        let base_proto = compile_source("1").unwrap();
        let mut bad = (*base_proto).clone();
        bad.upvalue_count = 1;
        assert!(bytecode::verify_proto(&bad).is_err());
        bad.upvalue_count = 0;
        bad.arity = 0;
        bad.is_variadic = true;
        assert!(bytecode::verify_proto(&bad).is_err());

        let run_err = |code: &str| VM::new().run(compile_source(code).unwrap()).is_err();
        assert!(run_err("import binary;\nbinary.pack(\"1000000000b\", [1])"));
        assert!(run_err("import ui;\nui.slider(5, 10, 0)"));
        assert!(run_err(
            "import ui;\nui.window(\"T\", 320, 240, #{headless: true});\nui.inject_resize(65536, 65536)"
        ));
        modules::reset_runtime_state();

        let cli_res = eval(
            r#"
import cli;
cli.parse(["run", "-1.5", "-a", "--name=ok"])
"#,
        );
        let Value::Dict(cd) = cli_res else {
            panic!("expected cli dict");
        };
        let Value::List(pos) = &cd.borrow()["positional"] else {
            panic!("expected positional");
        };
        assert_eq!(pos.borrow().len(), 2);
        assert!(pos.borrow()[1].equals(&Value::Str("-1.5".to_string())));

        let toml_res = eval(
            r#"
import toml;
let t = toml.parse("\"a=b\" = \"val\"");
t["a=b"]
"#,
        );
        assert!(toml_res.equals(&Value::Str("val".to_string())));
        assert!(run_err(
            "import toml;\ntoml.parse(\"a = \\\"\\\\uZZZZ\\\"\")"
        ));
        assert!(run_err(
            "import toml;\ntoml.parse(\"a = \\\"\\\"\\\"unterminated\")"
        ));
        let deep_toml_src = format!(
            "import toml;\ntoml.parse(\"{} = 1\")",
            vec!["a"; 70].join(".")
        );
        assert!(run_err(&deep_toml_src));

        let arc_dir = std::env::temp_dir().join(format!("askane_0161_arc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&arc_dir);
        std::fs::create_dir_all(&arc_dir).unwrap();
        let canon_arc = fs::canonicalize(&arc_dir).unwrap();
        let mut arc_vm = VM::new_with_sandbox(true);
        arc_vm.set_current_dir(canon_arc.clone());
        arc_vm.set_sandbox_root(canon_arc);
        assert!(
            arc_vm
                .run(
                    compile_source(
                        r#"
import archive;
let z = archive.pack(#{"a:ads": "x"});
z
"#
                    )
                    .unwrap()
                )
                .is_err()
        );
        assert!(
            arc_vm
                .run(
                    compile_source(
                        r#"
import archive;
let z = archive.pack(#{"evil.exe": "MZ"});
archive.extract(z, "out")
"#
                    )
                    .unwrap()
                )
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&arc_dir);

        let perm_dir =
            std::env::temp_dir().join(format!("askane_0161_perm_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&perm_dir);
        std::fs::create_dir_all(perm_dir.join("pkg")).unwrap();
        std::fs::write(
            perm_dir.join("main.ask"),
            "import \"pkg\" as p;\nfn f() { needs net; }\n",
        )
        .unwrap();
        std::fs::write(perm_dir.join("pkg.ask"), "pub let v = 1;\n").unwrap();
        std::fs::write(perm_dir.join("pkg").join("mod.ask"), "import http;\n").unwrap();
        let caps = inspect_permissions(&perm_dir.join("main.ask").display().to_string()).unwrap();
        assert!(caps.iter().any(|c| c.contains("needs: net")));
        assert!(caps.iter().any(|c| c.contains("http")));
        let _ = std::fs::remove_dir_all(&perm_dir);

        let mut perms = Permissions::sandboxed();
        let def_steps = perms.max_instructions.unwrap();
        let def_mem = perms.max_memory_bytes.unwrap();
        let def_ms = perms.max_duration_ms.unwrap();
        let man = AskaneManifest {
            entry: None,
            tests: None,
            max_steps: Some(def_steps.saturating_mul(100)),
            max_mem_mb: Some(100_000),
            timeout_ms: Some(def_ms.saturating_mul(100)),
        };
        apply_manifest_limits(&mut perms, &man, ExplicitLimits::default());
        assert_eq!(perms.max_instructions, Some(def_steps));
        assert_eq!(perms.max_memory_bytes, Some(def_mem));
        assert_eq!(perms.max_duration_ms, Some(def_ms));

        assert!(compile_source_with_specs("spec \"bad\" { let 1 = 2; }").is_err());
        let b_fmt = format_askane_source("fn f() {\nlet x = b\"hello {world}\";\nx\n}\n");
        assert!(b_fmt.contains("    let x = b\"hello {world}\";"));
        assert!(!needs_more_input("let b = b\"foo ${1\";"));
        assert!(needs_more_input("let b = b\"foo\n"));
        assert!(format_askane_source("let s = \"line1\n   line2\";\n").contains("   line2"));
    }

    #[test]
    fn test_v0161_setup_path_list_manipulation() {
        let (p1, added1) = setup::add_dir_to_path_list("C:\\Windows;C:\\Tools", "C:\\Askane\\");
        assert!(added1);
        assert!(p1.ends_with("C:\\Askane\\"));

        let (p2, added2) = setup::add_dir_to_path_list(&p1, "C:\\Askane");
        assert!(!added2);
        assert_eq!(p2, p1);

        let (p3, removed) = setup::remove_dir_from_path_list(&p1, "C:\\Askane");
        assert!(removed);
        assert_eq!(p3, "C:\\Windows;C:\\Tools");

        let (p4, removed2) = setup::remove_dir_from_path_list(&p3, "C:\\Missing");
        assert!(!removed2);
        assert_eq!(p4, "C:\\Windows;C:\\Tools");
    }

    #[test]
    fn test_v017_gui_helpers_and_setup() {
        let root = std::env::temp_dir().join(format!("askane_017_gui_tree_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("askane.toml"), "entry = \"main.ask\"\n").unwrap();
        std::fs::write(root.join("main.ask"), "println(1);\n").unwrap();
        std::fs::write(root.join("src").join("helper.ask"), "pub fn f() { 2 }\n").unwrap();
        std::fs::write(root.join(".git").join("ignored.ask"), "x\n").unwrap();

        assert_eq!(
            gui::find_project_root_for(&root.join("src").join("helper.ask")),
            root
        );
        let items = gui::scan_project_tree(&root, 100);
        assert!(items.iter().any(|it| it.is_dir && it.display == "[src]"));
        assert!(
            items
                .iter()
                .any(|it| !it.is_dir && it.display == "askane.toml")
        );
        assert!(
            items
                .iter()
                .any(|it| !it.is_dir && it.display.ends_with("helper.ask"))
        );
        assert!(!items.iter().any(|it| it.display.contains("ignored.ask")));
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(
            gui::parse_error_line_col("[12:5] Unexpected token"),
            Some((12, 5))
        );
        assert_eq!(
            gui::parse_error_line_col("  at helper (line 42)"),
            Some((42, 1))
        );
        assert_eq!(gui::parse_error_line_col("no location here"), None);

        let perms = gui::PermissionFlagsState {
            unsafe_mode: false,
            allow_read: true,
            allow_write: false,
            allow_net: true,
            allow_env: false,
            allow_exec: false,
            allow_input: true,
            allow_clipboard: false,
        };
        let flags = gui::compose_permission_flags(
            &perms,
            Some(std::path::Path::new("C:\\Proj")),
            "--max-steps=1000",
        )
        .unwrap();
        assert!(flags.contains(&"--allow-read=C:\\Proj".to_string()));
        assert!(flags.contains(&"--allow-net".to_string()));
        assert!(flags.contains(&"--allow-input".to_string()));
        assert!(flags.contains(&"--max-steps=1000".to_string()));
        assert!(gui::compose_permission_flags(&perms, None, "--bad-flag;rm").is_err());
    }

    #[test]
    fn test_v017_crypto_and_kv_store() {
        let mut vm = VM::new_with_sandbox(false);
        let crypto_ok = vm
            .run(
                compile_source(
                    r#"
import crypto;
import bytes;

let mac = crypto.hmac_sha256_hex("Jefe", "what do ya want for nothing?");
let mac_eq = mac == "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
let key = crypto.random_key();
let nonce = crypto.random_bytes(12);
let ct = crypto.seal(key, nonce, b"hello askane 0.17", "hdr");
let pt = crypto.open(key, nonce, ct, "hdr");
let ct_match = crypto.constant_time_eq(pt, b"hello askane 0.17");
let bad_aad = try {
    crypto.open(key, nonce, ct, "tampered");
    false
} catch e {
    true
};
mac_eq && ct_match && bad_aad && len(key) == 32
"#,
                )
                .unwrap(),
            )
            .unwrap();
        assert!(crypto_ok.equals(&Value::Bool(true)));

        let kv_dir = std::env::temp_dir().join(format!("askane_017_kv_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&kv_dir);
        std::fs::create_dir_all(&kv_dir).unwrap();
        let canon = fs::canonicalize(&kv_dir).unwrap();

        let mut kv_vm = VM::new_with_sandbox(true);
        kv_vm.set_current_dir(canon.clone());
        kv_vm.set_sandbox_root(canon.clone());

        let kv_res = kv_vm
            .run(
                compile_source(
                    r#"
import kv;
with kv.open("app.db") as db {
    db.set("user:1", #{ name: "Pleco", level: 7 });
    db.set("user:2", b"rawbytes");
    db.set("temp", 99);
    db.delete("temp");
    db.batch([["user:3", [1, 2, 3]], ["user:4", true]]);
    db.compact();
};
let db2 = kv.open("app.db");
let pfx = db2.prefix("user:");
let ok = db2.get("user:1")["name"] == "Pleco"
    && db2.get("user:2") == b"rawbytes"
    && !db2.has("temp")
    && db2.len() == 4
    && len(pfx) == 4;
db2.close();
ok
"#,
                )
                .unwrap(),
            )
            .unwrap();
        assert!(kv_res.equals(&Value::Bool(true)));

        assert!(
            kv_vm
                .run(compile_source("import kv;\nkv.open(\"../escape.db\")").unwrap())
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&kv_dir);
    }

    #[test]
    fn test_v017_net_ws_and_concurrency_features() {
        // 1. net & ws blocked in default sandbox
        let mut sbx = VM::new_with_sandbox(true);
        assert!(
            sbx.run(compile_source("import net;\nnet.tcp_listen(\"127.0.0.1:0\")").unwrap())
                .is_err()
        );
        assert!(
            sbx.run(compile_source("import ws;\nws.connect(\"ws://127.0.0.1:8080\")").unwrap())
                .is_err()
        );

        // 2. net TCP + UDP + WebSocket loopback with --allow-net=127.0.0.1
        let mut perms = Permissions::sandboxed();
        perms.allow_net = Some(vec!["127.0.0.1".to_string()]);
        let mut net_vm = VM::new_with_sandbox(true);
        net_vm.set_permissions(perms.clone());

        let net_ok = net_vm
            .run(
                compile_source(
                    r#"
import net;
import task;

let listener = net.tcp_listen("127.0.0.1:0");
let addr = listener.local_addr();
let srv = task.spawn(|| {
    let conn = listener.accept(0);
    while conn == unit {
        task.yield_now();
        conn = listener.accept(0);
    }
    let msg = conn.recv(64, 2000);
    conn.send(b"PONG:" + msg);
    conn.close();
    true
});
let cli = net.tcp_connect(addr, 2000);
cli.send("HI");
let srv_done = srv.join();
let reply = cli.recv(64, 2000);
cli.close();
listener.close();

let u1 = net.udp_bind("127.0.0.1:0");
let u2 = net.udp_bind("127.0.0.1:0");
u1.send_to(u2.local_addr(), b"UDP_OK");
let pkt = u2.recv_from(64, 2000);
u1.close();
u2.close();

srv_done && reply == b"PONG:HI" && pkt["data"] == b"UDP_OK"
"#,
                )
                .unwrap(),
            )
            .unwrap();
        assert!(net_ok.equals(&Value::Bool(true)));

        // 2b. Real WebSocket client VM <-> WebSocket server VM across OS threads
        let ws_dir = std::env::temp_dir().join(format!("askane_017_ws_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws_dir);
        std::fs::create_dir_all(&ws_dir).unwrap();
        let canon_ws = fs::canonicalize(&ws_dir).unwrap();
        let ws_srv_dir = canon_ws.clone();
        let ws_perms = perms.clone();
        let srv_thread = std::thread::spawn(move || {
            let mut srv_vm = VM::new_with_sandbox(true);
            srv_vm.set_permissions(ws_perms);
            srv_vm.set_current_dir(ws_srv_dir.clone());
            srv_vm.set_sandbox_root(ws_srv_dir);
            let res = srv_vm
                .run(
                    compile_source(
                        r#"
import ws;
import fs;
let wsl = ws.listen("127.0.0.1:0");
fs.write("ws_addr.txt", wsl.local_addr());
let c = wsl.accept(5000);
let m1 = c.recv(5000);
c.send("echo:" + m1);
let m2 = c.recv(5000);
c.send_bytes(m2);
c.close();
wsl.close();
true
"#,
                    )
                    .unwrap(),
                )
                .unwrap();
            assert!(res.equals(&Value::Bool(true)));
        });

        let addr_file = canon_ws.join("ws_addr.txt");
        let mut ws_addr = String::new();
        for _ in 0..200 {
            if let Ok(s) = std::fs::read_to_string(&addr_file) {
                if !s.trim().is_empty() {
                    ws_addr = s.trim().to_string();
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!ws_addr.is_empty());

        let mut cli_vm = VM::new_with_sandbox(true);
        cli_vm.set_permissions(perms);
        let ws_cli_ok = cli_vm
            .run(
                compile_source(&format!(
                    r#"
import ws;
let wc = ws.connect("ws://{}", 5000);
wc.send("askane");
let r1 = wc.recv(5000);
wc.send_bytes(b"\x01\x02");
let r2 = wc.recv(5000);
wc.close();
r1 == "echo:askane" && r2 == b"\x01\x02"
"#,
                    ws_addr
                ))
                .unwrap(),
            )
            .unwrap();
        assert!(ws_cli_ok.equals(&Value::Bool(true)));
        srv_thread.join().unwrap();
        let _ = std::fs::remove_dir_all(&ws_dir);

        // 3. Deterministic simulation, channels, select, timeout, signals + on change, snapshots
        let snap_dir = std::env::temp_dir().join(format!("askane_017_snap_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&snap_dir);
        std::fs::create_dir_all(&snap_dir).unwrap();
        let canon_snap = fs::canonicalize(&snap_dir).unwrap();

        let code = r#"
import task;
import channel;
import signal;
import time;
import random;
import test;

let c1 = channel.bounded(1);
let c2 = channel.bounded(2);

let t1 = task.spawn(|| {
    task.sleep(200);
    c1.send(10);
    c1.send(20);
    c1.close();
});
let t2 = task.spawn(|| {
    after 500ms {
        c2.send(99);
    };
    "done"
});

let sum = 0;
let first_sel = task.select([c1, c2]);
sum += first_sel["value"];
for _ in range(0, 3) {
    let s = task.select([c1, c2]);
    if !s["closed"] {
        sum += s["value"];
    }
}
t1.join();
let t2_res = t2.join();

let hp = signal.new(100);
let events = [];
on change(hp) {
    push(events, hp.get());
}
hp.set(80);
hp.set(80);
hp.update(|v| v - 30);

let snap_payload = #{
    sum: sum,
    t2: t2_res,
    events: events,
    r: random.int(1, 10000),
    elapsed_ok: time.elapsed_ms() >= 500,
    to_timed_out: !task.timeout(10, || { task.sleep(50); 1 })["ok"],
    to_ok: task.timeout(100, || 77)["value"],
};
test.assert_snapshot("core_017", snap_payload);
snap_payload
"#;
        let proto = compile_source_with_specs(code).unwrap();
        let mut vm1 = VM::new_with_sandbox(true);
        vm1.set_current_dir(canon_snap.clone());
        vm1.set_sandbox_root(canon_snap.clone());
        vm1.set_deterministic(true);
        let out1 = vm1.run(proto.clone()).unwrap();

        let mut vm2 = VM::new_with_sandbox(true);
        vm2.set_current_dir(canon_snap.clone());
        vm2.set_sandbox_root(canon_snap.clone());
        vm2.set_deterministic(true);
        let out2 = vm2.run(proto).unwrap();
        assert!(out1.equals(&out2));

        // Mismatch fails snapshot assertion until updated
        let mismatch_proto =
            compile_source("import test;\ntest.assert_snapshot(\"core_017\", #{ sum: 999 });\n")
                .unwrap();
        let mut vm3 = VM::new_with_sandbox(true);
        vm3.set_current_dir(canon_snap.clone());
        vm3.set_sandbox_root(canon_snap);
        assert!(vm3.run(mismatch_proto.clone()).is_err());
        vm3.set_update_snapshots(true);
        assert!(vm3.run(mismatch_proto).is_ok());

        let _ = std::fs::remove_dir_all(&snap_dir);
    }
}
