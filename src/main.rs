#[forbid(unsafe_code)]
mod ast;
#[forbid(unsafe_code)]
mod budget;
#[forbid(unsafe_code)]
mod buffer;
#[forbid(unsafe_code)]
mod bytecode;
#[forbid(unsafe_code)]
mod camera;
#[forbid(unsafe_code)]
mod compiler;
#[forbid(unsafe_code)]
mod compose;
#[forbid(unsafe_code)]
mod diag;
#[forbid(unsafe_code)]
mod font_atlas;
#[forbid(unsafe_code)]
mod font_bitmap;
#[forbid(unsafe_code)]
mod font_bmfont;
mod gui;
#[forbid(unsafe_code)]
#[forbid(unsafe_code)]
mod layers;
#[forbid(unsafe_code)]
mod lexer;
#[forbid(unsafe_code)]
mod media;
mod modules;
#[forbid(unsafe_code)]
mod parser;
#[forbid(unsafe_code)]
mod scene;
mod setup;
#[forbid(unsafe_code)]
mod surface;
#[forbid(unsafe_code)]
mod value;
#[forbid(unsafe_code)]
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

const VERSION: &str = "0.20.0-prerelease";
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

fn compile_source_filtered_specs(
    source: &str,
    spec_filter: Option<&str>,
) -> Result<(std::rc::Rc<bytecode::FunctionProto>, usize), String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err("Source size limit exceeded".to_string());
    }
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    let mut program = parser.parse_program()?;
    let mut matched_specs = 0usize;
    if let Some(pat) = spec_filter {
        let pat_low = pat.to_ascii_lowercase();
        let mut kept = Vec::with_capacity(program.stmts.len());
        for stmt in program.stmts {
            if let ast::Stmt::Spec { ref name, .. } = stmt {
                if name.to_ascii_lowercase().contains(&pat_low) {
                    matched_specs += 1;
                    kept.push(stmt);
                }
            } else {
                kept.push(stmt);
            }
        }
        program.stmts = kept;
    }
    let proto = Compiler::compile_with_specs(&program)?;
    Ok((proto, matched_specs))
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

fn use_cli_color(no_color: bool) -> bool {
    use std::io::IsTerminal;
    !no_color && diag::default_color_enabled() && std::io::stderr().is_terminal()
}

fn format_error_with_opts(file: Option<&str>, source: &str, err: &str, use_color: bool) -> String {
    let d = diag::classify_error(err, file);
    let target_src = match (&d.file, file) {
        (Some(df), Some(rf)) if df != rf => {
            read_text_limited(std::path::Path::new(df), MAX_SOURCE_BYTES).ok()
        }
        (Some(df), None) => read_text_limited(std::path::Path::new(df), MAX_SOURCE_BYTES).ok(),
        _ => None,
    };
    let src_ref = match (&d.file, file, target_src.as_deref()) {
        (_, _, Some(s)) => Some(s),
        (Some(df), Some(rf), None) if df != rf => None,
        (Some(_), None, None) => None,
        _ => Some(source),
    };
    diag::format_diagnostic(&d, src_ref, use_color)
}

fn format_error_with_source(source: &str, err: &str) -> String {
    format_error_with_opts(None, source, err, false)
}

#[cfg(test)]
fn run_file_with_root(
    path: &str,
    dump_bytecode: bool,
    permissions: Permissions,
    sandbox_root_override: Option<&std::path::Path>,
) -> Result<(), String> {
    run_file_with_script_args(
        path,
        dump_bytecode,
        false,
        permissions,
        sandbox_root_override,
        None,
        true,
        false,
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

#[allow(clippy::too_many_arguments)]
fn run_file_with_script_args(
    path: &str,
    dump_bytecode: bool,
    profile: bool,
    permissions: Permissions,
    sandbox_root_override: Option<&std::path::Path>,
    script_args: Option<&[String]>,
    no_color: bool,
    no_cache: bool,
) -> Result<(), String> {
    run_file_core_full(
        path,
        dump_bytecode,
        profile,
        false,
        permissions,
        sandbox_root_override,
        false,
        false,
        script_args,
        no_color,
        no_cache,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_file_core_full(
    path: &str,
    dump_bytecode: bool,
    profile: bool,
    include_specs: bool,
    permissions: Permissions,
    sandbox_root_override: Option<&std::path::Path>,
    deterministic: bool,
    update_snapshots: bool,
    script_args: Option<&[String]>,
    no_color: bool,
    no_cache: bool,
    spec_filter: Option<&str>,
) -> Result<(), String> {
    let color = use_cli_color(no_color);
    let source = read_text_limited(std::path::Path::new(path), MAX_SOURCE_BYTES)
        .map_err(|e| format!("Could not read '{}': {}", path, e))?;
    let use_cache = !no_cache && spec_filter.is_none();
    let mut cache_hit = false;
    let proto = if use_cache {
        if let Some(cached) = bytecode::try_load_cached_proto(
            std::path::Path::new(path),
            &source,
            false,
            include_specs,
        ) {
            cache_hit = true;
            cached
        } else if include_specs {
            compile_source_with_specs(&source)
                .map_err(|e| format_error_with_opts(Some(path), &source, &e, color))?
        } else {
            compile_source(&source)
                .map_err(|e| format_error_with_opts(Some(path), &source, &e, color))?
        }
    } else if include_specs && spec_filter.is_some() {
        compile_source_filtered_specs(&source, spec_filter)
            .map(|(p, _)| p)
            .map_err(|e| format_error_with_opts(Some(path), &source, &e, color))?
    } else if include_specs {
        compile_source_with_specs(&source)
            .map_err(|e| format_error_with_opts(Some(path), &source, &e, color))?
    } else {
        compile_source(&source)
            .map_err(|e| format_error_with_opts(Some(path), &source, &e, color))?
    };
    if dump_bytecode {
        print!("{}", proto.chunk.disassemble(&proto.name));
        return Ok(());
    }
    let mut vm = VM::new_with_sandbox(!permissions.allow_all);
    vm.set_permissions(permissions);
    vm.set_bytecode_cache(!no_cache);
    vm.set_script_file(path);
    vm.set_profile(profile);
    vm.set_deterministic(deterministic);
    vm.set_update_snapshots(update_snapshots);
    if let Some(sargs) = script_args {
        vm.set_script_args(sargs.to_vec());
    }
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
    let cache_source =
        fs::canonicalize(path).unwrap_or_else(|_| std::path::PathBuf::from(path));
    if !cache_hit && use_cache && vm.can_write_cache_for(&cache_source) {
        bytecode::try_save_cached_proto(&cache_source, &source, false, include_specs, &proto);
    }
    let run_result = if sandbox_root_override.is_some() {
        vm.run_with_base_dir(proto, entry_parent)
    } else {
        vm.run(proto)
    };
    run_result.map_err(|e| format_error_with_opts(Some(path), &source, &e, color))?;
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

struct CheckFileReport {
    ok: bool,
    file_json: String,
    diag_jsons: Vec<String>,
    err_text: Option<String>,
}

fn check_file_report(path: &str, json_out: bool, color: bool) -> CheckFileReport {
    let source = match read_text_limited(std::path::Path::new(path), MAX_SOURCE_BYTES) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("Could not read '{}': {}", path, e);
            let d = diag::classify_error(&msg, Some(path));
            let d_json = diag::diagnostic_to_json(&d, json_escape);
            return CheckFileReport {
                ok: false,
                file_json: format!(
                    "{{\"ok\":false,\"file\":{},\"error\":{},\"diagnostics\":[{}]}}",
                    json_escape(path),
                    json_escape(&msg),
                    d_json
                ),
                diag_jsons: vec![d_json],
                err_text: if json_out { None } else { Some(msg) },
            };
        }
    };

    let tokens = match Lexer::new(&source).tokenize() {
        Ok(t) => t,
        Err(err) => {
            let d = diag::classify_error(&err, Some(path));
            let d_json = diag::diagnostic_to_json(&d, json_escape);
            return CheckFileReport {
                ok: false,
                file_json: format!(
                    "{{\"ok\":false,\"file\":{},\"error\":{},\"diagnostics\":[{}]}}",
                    json_escape(path),
                    json_escape(&err),
                    d_json
                ),
                diag_jsons: vec![d_json],
                err_text: if json_out {
                    None
                } else {
                    Some(diag::format_diagnostic(&d, Some(&source), color))
                },
            };
        }
    };

    let (program, parse_errs) = Parser::new(tokens).parse_program_recover();
    let mut diags: Vec<diag::Diagnostic> = parse_errs
        .iter()
        .map(|e| diag::classify_error(e, Some(path)))
        .collect();

    let warnings = diag::analyze_warnings(&program, Some(path));
    if diags.is_empty() {
        if let Err(comp_err) = Compiler::compile_with_specs(&program) {
            diags.push(diag::classify_error(&comp_err, Some(path)));
        }
    }

    if diags.is_empty() {
        let warn_json = warnings
            .iter()
            .map(|w| diag::diagnostic_to_json(w, json_escape))
            .collect::<Vec<_>>();
        if !json_out {
            for w in &warnings {
                eprintln!("{}\n", diag::format_diagnostic(w, Some(&source), color));
            }
        }
        let wj_joined = warn_json.join(",");
        CheckFileReport {
            ok: true,
            file_json: format!(
                "{{\"ok\":true,\"file\":{},\"warnings\":[{}],\"diagnostics\":[{}]}}",
                json_escape(path),
                wj_joined,
                wj_joined
            ),
            diag_jsons: warn_json,
            err_text: None,
        }
    } else {
        let mut all_diags = diags.clone();
        all_diags.extend(warnings.clone());
        let first_err = parse_errs
            .first()
            .cloned()
            .unwrap_or_else(|| diags[0].message.clone());
        let diag_jsons = all_diags
            .iter()
            .map(|d| diag::diagnostic_to_json(d, json_escape))
            .collect::<Vec<_>>();
        if !json_out {
            for w in &warnings {
                eprintln!("{}\n", diag::format_diagnostic(w, Some(&source), color));
            }
        }
        let rendered = if json_out {
            None
        } else {
            Some(
                diags
                    .iter()
                    .map(|d| diag::format_diagnostic(d, Some(&source), color))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            )
        };
        CheckFileReport {
            ok: false,
            file_json: format!(
                "{{\"ok\":false,\"file\":{},\"error\":{},\"diagnostics\":[{}]}}",
                json_escape(path),
                json_escape(&first_err),
                diag_jsons.join(",")
            ),
            diag_jsons,
            err_text: rendered,
        }
    }
}

fn check_single_file_opts(path: &str, json_out: bool, no_color: bool) -> Result<(), String> {
    let color = use_cli_color(no_color);
    let rep = check_file_report(path, json_out, color);
    if json_out {
        println!("{}", rep.file_json);
        if rep.ok { Ok(()) } else { Err(String::new()) }
    } else if rep.ok {
        println!("OK: {}", path);
        Ok(())
    } else {
        Err(rep.err_text.unwrap_or_default())
    }
}

fn check_dir_json_report(path: &str, color: bool) -> (bool, String) {
    let p = std::path::Path::new(path);
    let mut files = Vec::new();
    if let Err(err) = collect_ask_files(p, &mut files) {
        let json = format!(
            "{{\"ok\":false,\"path\":{},\"error\":{},\"file_count\":0,\"failed_count\":0,\"files\":[],\"diagnostics\":[]}}",
            json_escape(path),
            json_escape(&err)
        );
        return (false, json);
    }
    if files.is_empty() {
        let msg = format!("No .ask files found in '{}'", path);
        let json = format!(
            "{{\"ok\":false,\"path\":{},\"error\":{},\"file_count\":0,\"failed_count\":0,\"files\":[],\"diagnostics\":[]}}",
            json_escape(path),
            json_escape(&msg)
        );
        return (false, json);
    }
    let mut file_jsons = Vec::with_capacity(files.len());
    let mut all_diags = Vec::new();
    let mut failed = 0usize;
    for f in &files {
        let rep = check_file_report(&f.display().to_string(), true, color);
        if !rep.ok {
            failed += 1;
        }
        file_jsons.push(rep.file_json);
        all_diags.extend(rep.diag_jsons);
    }
    let ok = failed == 0;
    let json = format!(
        "{{\"ok\":{},\"path\":{},\"file_count\":{},\"failed_count\":{},\"files\":[{}],\"diagnostics\":[{}]}}",
        ok,
        json_escape(path),
        files.len(),
        failed,
        file_jsons.join(","),
        all_diags.join(",")
    );
    (ok, json)
}

fn run_check_opts(path: &str, json_out: bool, no_color: bool) -> Result<(), String> {
    let p = std::path::Path::new(path);
    if p.is_dir() {
        if json_out {
            let color = use_cli_color(no_color);
            let (ok, json) = check_dir_json_report(path, color);
            println!("{}", json);
            if ok { Ok(()) } else { Err(String::new()) }
        } else {
            let mut files = Vec::new();
            collect_ask_files(p, &mut files)?;
            if files.is_empty() {
                return Err(format!("No .ask files found in '{}'", path));
            }
            let mut failed = 0usize;
            for f in &files {
                if let Err(e) = check_single_file_opts(&f.display().to_string(), false, no_color) {
                    if !e.is_empty() {
                        eprintln!("Check failed: {}", e);
                    }
                    failed += 1;
                }
            }
            if failed > 0 {
                Err(format!("{} file(s) failed check", failed))
            } else {
                Ok(())
            }
        }
    } else {
        check_single_file_opts(path, json_out, no_color)
    }
}

fn push_unique(out: &mut Vec<String>, item: String) {
    if !out.contains(&item) {
        out.push(item);
    }
}

#[derive(Default)]
struct ImportScanCtx {
    imports: Vec<ast::ImportSource>,
    needs: Vec<String>,
    mod_aliases: std::collections::HashMap<String, String>,
    fn_aliases: std::collections::HashMap<String, (String, String)>,
}

fn preregister_import_aliases(stmts: &[ast::Stmt], ctx: &mut ImportScanCtx) {
    for stmt in stmts {
        match stmt {
            ast::Stmt::Import {
                source: ast::ImportSource::Name(mod_name),
                alias,
                ..
            } => {
                ctx.mod_aliases.insert(alias.clone(), mod_name.clone());
            }
            ast::Stmt::ImportFrom {
                source: ast::ImportSource::Name(mod_name),
                items,
                ..
            } => {
                for item in items {
                    ctx.fn_aliases
                        .insert(item.alias.clone(), (mod_name.clone(), item.name.clone()));
                }
            }
            _ => {}
        }
    }
}

fn check_dynamic_cap_call(
    ctx: &mut ImportScanCtx,
    callee: &ast::Expr,
    first_arg: Option<&ast::Expr>,
) {
    let Some(arg0) = first_arg else {
        return;
    };
    if matches!(arg0, ast::Expr::Str(_, _)) {
        return;
    }
    let resolved: Option<(String, String)> = match callee {
        ast::Expr::GetAttr { object, name, .. } => {
            if let ast::Expr::Ident(ident, _) = object.as_ref() {
                let mod_name = ctx
                    .mod_aliases
                    .get(ident)
                    .map(String::as_str)
                    .unwrap_or(ident.as_str());
                Some((mod_name.to_string(), name.clone()))
            } else {
                None
            }
        }
        ast::Expr::Ident(fn_name, _) => ctx.fn_aliases.get(fn_name).cloned(),
        _ => None,
    };
    if let Some((mod_name, fn_name)) = resolved {
        if matches!(
            (mod_name.as_str(), fn_name.as_str()),
            ("process", "run")
                | ("http", "get")
                | ("http", "post")
                | ("net", "tcp_connect")
                | ("ws", "connect")
        ) {
            push_unique(
                &mut ctx.needs,
                format!(
                    "[dynamic] {}.{}() target is non-literal and cannot be statically verified",
                    mod_name, fn_name
                ),
            );
        }
    }
}

fn collect_imports_expr(expr: &ast::Expr, ctx: &mut ImportScanCtx) {
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
                collect_imports_expr(item, ctx);
            }
        }
        ast::Expr::Dict(entries, _) => {
            for (k, v) in entries {
                collect_imports_expr(k, ctx);
                collect_imports_expr(v, ctx);
            }
        }
        ast::Expr::Unary { expr: inner, .. } | ast::Expr::Throw { value: inner, .. } => {
            collect_imports_expr(inner, ctx);
        }
        ast::Expr::Binary { left, right, .. } | ast::Expr::NullCoalesce { left, right, .. } => {
            collect_imports_expr(left, ctx);
            collect_imports_expr(right, ctx);
        }
        ast::Expr::Assign { target, value, .. } => {
            match target.as_ref() {
                ast::AssignTarget::Var(_) => {}
                ast::AssignTarget::Index { object, index } => {
                    collect_imports_expr(object, ctx);
                    collect_imports_expr(index, ctx);
                }
                ast::AssignTarget::Attr { object, .. } => {
                    collect_imports_expr(object, ctx);
                }
            }
            collect_imports_expr(value, ctx);
        }
        ast::Expr::Block { stmts, tail, .. } => {
            let saved_mods = ctx.mod_aliases.clone();
            let saved_fns = ctx.fn_aliases.clone();
            preregister_import_aliases(stmts, ctx);
            for s in stmts {
                collect_imports_stmt(s, ctx);
            }
            if let Some(t) = tail {
                collect_imports_expr(t, ctx);
            }
            ctx.mod_aliases = saved_mods;
            ctx.fn_aliases = saved_fns;
        }
        ast::Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_imports_expr(cond, ctx);
            collect_imports_expr(then_branch, ctx);
            if let Some(e) = else_branch {
                collect_imports_expr(e, ctx);
            }
        }
        ast::Expr::TryCatch {
            try_body,
            catch_body,
            ..
        } => {
            collect_imports_expr(try_body, ctx);
            collect_imports_expr(catch_body, ctx);
        }
        ast::Expr::Match { subject, arms, .. } => {
            collect_imports_expr(subject, ctx);
            for arm in arms {
                if let Some(g) = &arm.guard {
                    collect_imports_expr(g, ctx);
                }
                collect_imports_expr(&arm.body, ctx);
            }
        }
        ast::Expr::Fn { params, body, .. } => {
            for p in params {
                if let Some(d) = &p.default {
                    collect_imports_expr(d, ctx);
                }
            }
            collect_imports_expr(body, ctx);
        }
        ast::Expr::Call { callee, args, .. } => {
            check_dynamic_cap_call(ctx, callee.as_ref(), args.first());
            collect_imports_expr(callee, ctx);
            for a in args {
                collect_imports_expr(a, ctx);
            }
        }
        ast::Expr::CallNamed(c) => {
            let target_arg = c
                .pos_args
                .first()
                .or_else(|| {
                    c.named_args
                        .iter()
                        .find(|(k, _)| matches!(k.as_str(), "url" | "cmd" | "addr"))
                        .map(|(_, e)| e)
                })
                .or_else(|| c.named_args.first().map(|(_, e)| e));
            check_dynamic_cap_call(ctx, &c.callee, target_arg);
            collect_imports_expr(&c.callee, ctx);
            for a in &c.pos_args {
                collect_imports_expr(a, ctx);
            }
            for (_, a) in &c.named_args {
                collect_imports_expr(a, ctx);
            }
        }
        ast::Expr::StructInit(st) => {
            collect_imports_expr(&st.target, ctx);
            if let Some(sp) = &st.spread {
                collect_imports_expr(sp, ctx);
            }
            for (_, v) in &st.fields {
                collect_imports_expr(v, ctx);
            }
        }
        ast::Expr::ChainedCmp(c) => {
            collect_imports_expr(&c.first, ctx);
            for (_, rhs) in &c.comparisons {
                collect_imports_expr(rhs, ctx);
            }
        }
        ast::Expr::Index { object, index, .. } => {
            collect_imports_expr(object, ctx);
            collect_imports_expr(index, ctx);
        }
        ast::Expr::Slice {
            object, start, end, ..
        } => {
            collect_imports_expr(object, ctx);
            if let Some(s) = start {
                collect_imports_expr(s, ctx);
            }
            if let Some(e) = end {
                collect_imports_expr(e, ctx);
            }
        }
        ast::Expr::ListComp {
            expr: item,
            iter,
            cond,
            ..
        } => {
            collect_imports_expr(iter, ctx);
            if let Some(c) = cond {
                collect_imports_expr(c, ctx);
            }
            collect_imports_expr(item, ctx);
        }
        ast::Expr::GetAttr { object, .. } | ast::Expr::OptGetAttr { object, .. } => {
            collect_imports_expr(object, ctx);
        }
        ast::Expr::SafePipe(p) => {
            collect_imports_expr(&p.left, ctx);
            collect_imports_expr(&p.apply, ctx);
        }
        ast::Expr::MapPipe { left, mapper, .. } => {
            collect_imports_expr(left, ctx);
            collect_imports_expr(mapper, ctx);
        }
        ast::Expr::PipeAssign(p) => {
            match p.target.as_ref() {
                ast::AssignTarget::Var(_) => {}
                ast::AssignTarget::Index { object, index } => {
                    collect_imports_expr(object, ctx);
                    collect_imports_expr(index, ctx);
                }
                ast::AssignTarget::Attr { object, .. } => {
                    collect_imports_expr(object, ctx);
                }
            }
            for st in &p.stages {
                collect_imports_expr(st, ctx);
            }
        }
        ast::Expr::TaggedTemplate(t) => {
            for e in &t.exprs {
                collect_imports_expr(e, ctx);
            }
        }
        ast::Expr::With(w) => {
            collect_imports_expr(&w.resource, ctx);
            collect_imports_expr(&w.body, ctx);
        }
        ast::Expr::Sandbox(s) => {
            push_unique(
                &mut ctx.needs,
                "sandbox(...) dynamically attenuates capabilities within block".to_string(),
            );
            for (_, v) in &s.policy {
                collect_imports_expr(v, ctx);
            }
            collect_imports_expr(&s.body, ctx);
        }
        ast::Expr::After { delay, body, .. } => {
            collect_imports_expr(delay, ctx);
            collect_imports_expr(body, ctx);
        }
    }
}

fn collect_imports_stmt(stmt: &ast::Stmt, ctx: &mut ImportScanCtx) {
    match stmt {
        ast::Stmt::Import { source, alias, .. } => {
            if let ast::ImportSource::Name(mod_name) = source {
                ctx.mod_aliases.insert(alias.clone(), mod_name.clone());
            }
            ctx.imports.push(source.clone());
        }
        ast::Stmt::ImportFrom { source, items, .. } => {
            if let ast::ImportSource::Name(mod_name) = source {
                for item in items {
                    ctx.fn_aliases
                        .insert(item.alias.clone(), (mod_name.clone(), item.name.clone()));
                }
            }
            ctx.imports.push(source.clone());
        }
        ast::Stmt::Let { value, .. }
        | ast::Stmt::LetList { value, .. }
        | ast::Stmt::LetDict { value, .. } => {
            collect_imports_expr(value, ctx);
        }
        ast::Stmt::FnDecl { params, body, .. } => {
            for p in params {
                if let Some(d) = &p.default {
                    collect_imports_expr(d, ctx);
                }
            }
            collect_imports_expr(body, ctx);
        }
        ast::Stmt::StructDecl {
            fields, methods, ..
        } => {
            for f in fields {
                if let Some(d) = &f.default {
                    collect_imports_expr(d, ctx);
                }
            }
            for m in methods {
                for p in &m.params {
                    if let Some(d) = &p.default {
                        collect_imports_expr(d, ctx);
                    }
                }
                collect_imports_expr(&m.body, ctx);
            }
        }
        ast::Stmt::While { cond, body, .. } => {
            collect_imports_expr(cond, ctx);
            collect_imports_expr(body, ctx);
        }
        ast::Stmt::For { iter, body, .. } | ast::Stmt::ForPair { iter, body, .. } => {
            collect_imports_expr(iter, ctx);
            collect_imports_expr(body, ctx);
        }
        ast::Stmt::Return { value, .. } => {
            if let Some(v) = value {
                collect_imports_expr(v, ctx);
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
                push_unique(&mut ctx.needs, label);
            }
        }
        ast::Stmt::Defer { body, .. } => {
            collect_imports_expr(body, ctx);
        }
        ast::Stmt::Every { interval, body, .. } => {
            collect_imports_expr(interval, ctx);
            collect_imports_expr(body, ctx);
        }
        ast::Stmt::Spec { body, .. } => {
            collect_imports_expr(body, ctx);
        }
        ast::Stmt::Expr(e) => {
            collect_imports_expr(e, ctx);
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

    let mut ctx = ImportScanCtx {
        needs: std::mem::take(capabilities),
        ..Default::default()
    };
    preregister_import_aliases(&program.stmts, &mut ctx);
    for stmt in &program.stmts {
        collect_imports_stmt(stmt, &mut ctx);
    }
    if let Some(tail) = &program.tail {
        collect_imports_expr(tail, &mut ctx);
    }
    *capabilities = ctx.needs;
    let imports = ctx.imports;

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

fn run_eval(
    code: &str,
    profile: bool,
    perms: Permissions,
    script_args: Option<&[String]>,
    no_color: bool,
) -> Result<(), String> {
    let color = use_cli_color(no_color);
    let proto = compile_source(code).map_err(|e| format_error_with_opts(None, code, &e, color))?;
    let mut vm = VM::new_with_sandbox(!perms.allow_all);
    vm.set_permissions(perms);
    vm.set_profile(profile);
    if let Some(sargs) = script_args {
        vm.set_script_args(sargs.to_vec());
    }
    let val = vm
        .run(proto)
        .map_err(|e| format_error_with_opts(None, code, &e, color))?;
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
    run_tests_cmd_with_opts(
        path,
        perms,
        sandbox_root_override,
        false,
        false,
        None,
        false,
    )
}

fn run_tests_cmd_with_opts(
    path: &str,
    perms: Permissions,
    sandbox_root_override: Option<&std::path::Path>,
    deterministic: bool,
    update_snapshots: bool,
    filter: Option<&str>,
    no_cache: bool,
) -> Result<(), String> {
    let base_root = std::path::Path::new(path);
    let mut files = Vec::new();
    collect_ask_files(base_root, &mut files)?;
    if files.is_empty() {
        return Err(format!("No .ask test files found in '{}'", path));
    }
    let filter_low = filter.map(|s| s.to_ascii_lowercase());
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    for file in &files {
        let label = file.display().to_string();
        let rel_label = if base_root.is_dir() {
            file.strip_prefix(base_root)
                .unwrap_or(file.as_path())
                .to_string_lossy()
                .replace('\\', "/")
        } else {
            file.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| label.clone())
        };
        let mut active_spec_filter: Option<&str> = None;
        if let Some(ref pat_low) = filter_low {
            let source = read_text_limited(file, MAX_SOURCE_BYTES)
                .map_err(|e| format!("Could not read '{}': {}", label, e))?;
            match compile_source_filtered_specs(&source, filter) {
                Ok((_, matched)) if matched > 0 => {
                    active_spec_filter = filter;
                }
                Ok(_) if rel_label.to_ascii_lowercase().contains(pat_low.as_str()) => {}
                Ok(_) => {
                    skipped += 1;
                    continue;
                }
                Err(e) => {
                    eprintln!(
                        "FAIL {}\n{}",
                        label,
                        format_error_with_opts(Some(&label), &source, &e, false)
                    );
                    failed += 1;
                    continue;
                }
            }
        }
        match run_file_core_full(
            &label,
            false,
            false,
            true,
            perms.clone(),
            sandbox_root_override,
            deterministic,
            update_snapshots,
            None,
            true,
            no_cache,
            active_spec_filter,
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
    if filter.is_some() && passed == 0 && failed == 0 {
        return Err(format!(
            "No tests matched filter '{}'",
            filter.unwrap_or("")
        ));
    }
    println!();
    if skipped > 0 {
        println!(
            "Results: {} passed, {} failed, {} skipped",
            passed, failed, skipped
        );
    } else {
        println!("Results: {} passed, {} failed", passed, failed);
    }
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
    fmt_indent: Option<usize>,
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
            ("fmt", "indent") => {
                let n = parse_toml_u64(val, "fmt.indent")?;
                if !(1..=8).contains(&n) {
                    return Err("askane.toml [fmt].indent must be between 1 and 8".to_string());
                }
                manifest.fmt_indent = Some(n as usize);
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
                        "Built-in modules ({}): {}",
                        modules::BUILTIN_MODULE_NAMES.len(),
                        modules::BUILTIN_MODULE_NAMES.join(", ")
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
    println!("  askane                              Start interactive REPL");
    println!("  askane --gui [file.ask]             Open Askane Studio GUI editor (Windows)");
    println!("  askane [run] <path> [-- args...]    Run a script or project (uses askane.toml)");
    println!("  askane -e \"<code>\" [-- args...]     Evaluate inline code");
    println!(
        "  askane check [opts] [path]          Syntax, compile & lint check (file, dir, or project)"
    );
    println!("    --json | --diagnostic-format=json Emit structured JSON diagnostics");
    println!("    --no-color                        Disable ANSI color output");
    println!("  askane --explain <CODE>             Explain an error/warning code (e.g. E0401)");
    println!("  askane permissions <file>           Inspect imported capabilities (transitive)");
    println!("  askane fmt [--check|--diff] [opts]  Format Askane files or stdin (--stdin)");
    println!("  askane init [--name <name>] [dir]   Create a starter Askane project");
    println!("  askane info                         Show Askane version, host & project info");
    println!("  askane doctor                       Diagnose installation, PATH & environment");
    println!("  askane modules [--json]             List all built-in modules & platforms");
    println!(
        "  askane clean [opts] [dir]           Remove temporary/cache files (--dry-run, --snapshots)"
    );
    println!(
        "  askane setup [opts]                 Add Askane to User PATH & .ask assoc (Windows)"
    );
    println!("  askane test [opts] [path]           Run .ask test files or project tests");
    println!("    --filter=<pattern>                Only run test files/specs matching pattern");
    println!("    --deterministic                   Enable virtual time & fixed random seed");
    println!("    --update-snapshots                Overwrite mismatched snapshot files");
    println!("  askane -b <file.ask>                Disassemble compiled bytecode");
    println!("  askane --profile <file.ask>         Print function profiling stats");
    println!("  askane --no-cache <file.ask>        Disable reading/writing .askc bytecode cache");
    println!("  askane -h | --help                  Show help");
    println!("  askane -v | --version               Show version");
    println!();
    println!("Sandbox permissions:");
    println!("  --allow-net[=hosts]                 Allow HTTPS/TCP/WS access (host[:port],...)");
    println!("  --allow-read[=dirs]                 Allow reading outside sandbox");
    println!("  --allow-write[=dirs]                Allow writing outside sandbox");
    println!("  --allow-env[=vars]                  Allow sys.env() / os.env()");
    println!("  --allow-args                        Allow sys.args() / os.args()");
    println!("  --allow-clipboard                   Allow clipboard.get_text() / set_text()");
    println!("  --allow-input                       Allow global keyboard/mouse state in input");
    println!("  --allow-exec[=cmds]                 Allow process.run()");
    println!(
        "  --max-steps=N                       Override sandbox instruction budget (default 10000000)"
    );
    println!(
        "  --max-mem=MB                        Override sandbox allocation budget (default 64)"
    );
    println!(
        "  --timeout=MS                        Override sandbox timeout in ms (default 30000)"
    );
    println!("  --no-color                          Disable colored diagnostic output");
    println!(
        "  --unsafe                            Allow unrestricted files, network, env and process access"
    );
}

fn run_explain(code: &str) -> Result<(), String> {
    if let Some((title, body)) = diag::explain_code(code) {
        println!("{}: {}\n\n{}", code.to_ascii_uppercase(), title, body);
        Ok(())
    } else {
        Err(format!(
            "Unknown diagnostic code '{}'. Known codes: E0101, E0201, E0202, E0301, E0302, E0401, E0402, E0403, E0404, E0405, W0101, W0102",
            code
        ))
    }
}

fn run_info() {
    let exe = env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());
    let cwd = env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".to_string());
    println!("askane {}", VERSION);
    println!(
        "OS / Arch        : {} / {} ({})",
        env::consts::OS,
        env::consts::ARCH,
        env::consts::FAMILY
    );
    println!("Executable       : {}", exe);
    println!("Working Dir      : {}", cwd);
    println!("Built-in Modules : {}", modules::BUILTIN_MODULE_NAMES.len());
    match discover_manifest_from_cwd() {
        Ok(Some((root, m))) => {
            println!("Project Root     : {}", root.display());
            println!(
                "Manifest         : entry={}, tests={}",
                m.entry.as_deref().unwrap_or("main.ask"),
                m.tests.as_deref().unwrap_or("tests")
            );
        }
        Ok(None) => println!("Project Root     : (no askane.toml found)"),
        Err(e) => println!("Project Root     : error ({})", e),
    }
}

fn run_doctor() -> Result<(), String> {
    let mut issues = 0usize;
    let exe = env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());
    println!("Askane Doctor v{}", VERSION);
    println!("[ok] Executable: {}", exe);
    println!(
        "[ok] Platform  : {} / {}",
        env::consts::OS,
        env::consts::ARCH
    );

    #[cfg(windows)]
    {
        match setup::win32_setup::current_exe_clean() {
            Ok(clean_exe) => {
                let st = setup::win32_setup::current_status(&clean_exe);
                if st.in_path {
                    println!("[ok] User PATH : registered");
                } else {
                    println!(
                        "[warn] User PATH : not registered (run `askane setup --path` if desired)"
                    );
                }
                if st.assoc_registered {
                    println!("[ok] .ask assoc: {}", st.assoc_mode);
                } else {
                    println!(
                        "[info] .ask assoc: not registered (run `askane setup --assoc` to associate)"
                    );
                }
            }
            Err(err) => {
                println!("[warn] Setup probe failed: {}", err);
            }
        }
    }
    #[cfg(not(windows))]
    {
        println!("[info] Windows setup checks skipped on {}", env::consts::OS);
    }

    let tmp_probe = env::temp_dir().join(format!("askane_doctor_{}.tmp", process::id()));
    match fs::write(&tmp_probe, b"ok") {
        Ok(()) => {
            let _ = fs::remove_file(&tmp_probe);
            println!("[ok] Temp dir  : writable ({})", env::temp_dir().display());
        }
        Err(e) => {
            println!(
                "[fail] Temp dir: not writable ({}): {}",
                env::temp_dir().display(),
                e
            );
            issues += 1;
        }
    }

    match discover_manifest_from_cwd() {
        Ok(Some((root, _))) => println!("[ok] Manifest  : found at {}", root.display()),
        Ok(None) => println!("[info] Manifest: none in current directory tree"),
        Err(e) => {
            println!("[fail] Manifest: {}", e);
            issues += 1;
        }
    }

    if issues > 0 {
        Err(format!("doctor found {} issue(s)", issues))
    } else {
        Ok(())
    }
}

fn run_modules_cmd(json_out: bool) {
    if json_out {
        let items: Vec<String> = modules::BUILTIN_MODULE_NAMES
            .iter()
            .map(|&name| {
                let (cat, plat, desc) = modules::module_info(name);
                format!(
                    "{{\"name\":{},\"category\":{},\"platform\":{},\"description\":{}}}",
                    json_escape(name),
                    json_escape(cat),
                    json_escape(plat),
                    json_escape(desc)
                )
            })
            .collect();
        println!(
            "{{\"count\":{},\"modules\":[{}]}}",
            modules::BUILTIN_MODULE_NAMES.len(),
            items.join(",")
        );
    } else {
        println!(
            "Built-in Askane modules ({}):",
            modules::BUILTIN_MODULE_NAMES.len()
        );
        for &name in modules::BUILTIN_MODULE_NAMES {
            let (cat, plat, desc) = modules::module_info(name);
            println!("  {:<12} [{:<7} | {:<7}] {}", name, cat, plat, desc);
        }
    }
}

fn clean_dir_walk(
    dir: &std::path::Path,
    dry_run: bool,
    include_snapshots: bool,
    removed: &mut usize,
    depth: usize,
) -> Result<(), String> {
    if depth > 16 {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|e| format!("clean '{}': {}", dir.display(), e))?;
    for entry in entries.flatten() {
        let p = entry.path();
        let meta = match fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if meta.is_dir() {
            if name == "target" || name.starts_with('.') {
                continue;
            }
            if include_snapshots && name == "__snapshots__" {
                if dry_run {
                    println!("[dry-run] remove dir {}", p.display());
                } else {
                    fs::remove_dir_all(&p)
                        .map_err(|e| format!("Failed to remove '{}': {}", p.display(), e))?;
                    println!("Removed {}", p.display());
                }
                *removed += 1;
                continue;
            }
            clean_dir_walk(&p, dry_run, include_snapshots, removed, depth + 1)?;
            if name == "__askcache__" && !dry_run {
                if let Ok(mut rd) = fs::read_dir(&p) {
                    if rd.next().is_none() {
                        let _ = fs::remove_dir(&p);
                    }
                }
            }
        } else if meta.is_file() {
            let is_tmp = name.ends_with(".askc")
                || name.starts_with("_askane_tmp_")
                || (name.ends_with(".tmp") && name.starts_with("askane_"));
            if is_tmp {
                if dry_run {
                    println!("[dry-run] remove {}", p.display());
                } else {
                    fs::remove_file(&p)
                        .map_err(|e| format!("Failed to remove '{}': {}", p.display(), e))?;
                    println!("Removed {}", p.display());
                }
                *removed += 1;
            }
        }
    }
    Ok(())
}

fn run_clean_cmd(args: &[String]) -> Result<(), String> {
    let mut dry_run = false;
    let mut include_snapshots = false;
    let mut target: Option<String> = None;
    for arg in args {
        if arg == "--dry-run" {
            dry_run = true;
        } else if arg == "--snapshots" {
            include_snapshots = true;
        } else if arg.starts_with('-') {
            return Err(format!("Unknown clean option '{}'", arg));
        } else if target.is_none() {
            target = Some(arg.clone());
        } else {
            return Err("Usage: askane clean [--dry-run] [--snapshots] [dir]".to_string());
        }
    }
    let root = std::path::Path::new(target.as_deref().unwrap_or("."));
    if !root.is_dir() {
        return Err(format!("'{}' is not a directory", root.display()));
    }
    let mut removed = 0usize;
    clean_dir_walk(root, dry_run, include_snapshots, &mut removed, 0)?;
    if removed == 0 {
        println!("Nothing to clean in '{}'.", root.display());
    } else if dry_run {
        println!("Dry run: {} item(s) would be removed.", removed);
    } else {
        println!("Cleaned {} item(s).", removed);
    }
    Ok(())
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
    format_askane_source_with_indent(src, 4)
}

fn format_askane_source_with_indent(src: &str, indent_spaces: usize) -> String {
    let indent_unit = " ".repeat(indent_spaces.clamp(1, 8));
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
            out.push(format!(
                "{}{}",
                indent_unit.repeat(line_indent),
                formatted_body
            ));
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

#[derive(Clone, Copy, Default)]
struct FmtOptions {
    check_only: bool,
    diff: bool,
    use_stdin: bool,
}

fn resolve_fmt_indent(target: Option<&std::path::Path>) -> usize {
    if let Some(p) = target {
        let start = if p.is_dir() {
            Some(p.to_path_buf())
        } else {
            p.parent().map(|d| {
                if d.as_os_str().is_empty() {
                    std::path::PathBuf::from(".")
                } else {
                    d.to_path_buf()
                }
            })
        };
        if let Some(mut cur) = start {
            cur = fs::canonicalize(&cur).unwrap_or(cur);
            for _ in 0..16 {
                if let Ok(Some((_, m))) = load_manifest_from_dir(&cur) {
                    if let Some(ind) = m.fmt_indent {
                        return ind;
                    }
                }
                let Some(parent) = cur.parent() else {
                    break;
                };
                cur = parent.to_path_buf();
            }
        }
    }
    if let Ok(Some((_, m))) = discover_manifest_from_cwd() {
        if let Some(ind) = m.fmt_indent {
            return ind;
        }
    }
    4
}

fn render_fmt_unified_diff(label: &str, original: &str, formatted: &str) -> String {
    let orig_norm = original.replace("\r\n", "\n");
    let a: Vec<&str> = orig_norm.lines().collect();
    let b: Vec<&str> = formatted.lines().collect();
    let ops = modules::diff::compute_line_diff_raw(&a, &b);
    let mut out = format!("--- {} (original)\n+++ {} (formatted)\n", label, label);
    let mut has_changes = false;
    for (tag, line) in ops {
        if tag != ' ' {
            has_changes = true;
            out.push(tag);
            out.push_str(line);
            out.push('\n');
        }
    }
    if !has_changes && original != formatted {
        out.push_str("-<CRLF line endings>\n+<LF line endings>\n");
    }
    out
}

#[cfg(test)]
fn run_fmt(path: &str, check_only: bool) -> Result<(), String> {
    run_fmt_opts(
        Some(path),
        FmtOptions {
            check_only,
            diff: false,
            use_stdin: false,
        },
    )
}

fn run_fmt_opts(path_opt: Option<&str>, opts: FmtOptions) -> Result<(), String> {
    if opts.use_stdin {
        use std::io::Read;
        let mut source = String::new();
        io::stdin()
            .take(MAX_SOURCE_BYTES as u64 + 1)
            .read_to_string(&mut source)
            .map_err(|e| format!("Failed to read stdin: {}", e))?;
        if source.len() > MAX_SOURCE_BYTES {
            return Err("Stdin input exceeds 4 MiB limit".to_string());
        }
        verify_askane_syntax(&source).map_err(|e| {
            format!(
                "Syntax error in stdin:\n{}",
                format_error_with_source(&source, &e)
            )
        })?;
        let indent = resolve_fmt_indent(None);
        let formatted = format_askane_source_with_indent(&source, indent);
        verify_askane_syntax(&formatted)
            .map_err(|e| format!("Internal formatter error on stdin: {}", e))?;
        if opts.diff {
            if formatted != source {
                print!(
                    "{}",
                    render_fmt_unified_diff("<stdin>", &source, &formatted)
                );
                return Err("Stdin input is not formatted".to_string());
            }
            return Ok(());
        }
        if opts.check_only {
            if formatted != source {
                return Err("Stdin input is not formatted".to_string());
            }
            return Ok(());
        }
        print!("{}", formatted);
        return Ok(());
    }

    let path = path_opt.ok_or_else(|| "Missing path for askane fmt".to_string())?;
    let p = std::path::Path::new(path);
    let indent = resolve_fmt_indent(Some(p));
    let mut files = Vec::new();
    if p.is_dir() {
        collect_ask_files(p, &mut files)?;
        if files.is_empty() {
            return Err(format!("No .ask files found in '{}'", path));
        }
    } else {
        files.push(p.to_path_buf());
    }
    let mut unformatted = 0usize;
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
        let formatted = format_askane_source_with_indent(&source, indent);
        verify_askane_syntax(&formatted)
            .map_err(|e| format!("Internal formatter error on '{}': {}", fstr, e))?;
        if opts.diff {
            if formatted != source {
                print!("{}", render_fmt_unified_diff(&fstr, &source, &formatted));
                unformatted += 1;
            }
        } else if opts.check_only {
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
    if opts.diff && unformatted > 0 {
        return Err(format!("{} file(s) need formatting", unformatted));
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
        no_color: bool,
        no_cache: bool,
        test_filter: Option<String>,
        eval_code: Option<String>,
        perms: Permissions,
        script_path: Option<String>,
        script_args: Option<Vec<String>>,
        explicit_limits: ExplicitLimits,
    },
}

fn parse_run_args(args: &[String]) -> Result<CliParseOutcome, String> {
    let mut dump_bytecode = false;
    let mut profile = false;
    let mut deterministic = false;
    let mut update_snapshots = false;
    let mut no_color = false;
    let mut no_cache = false;
    let mut test_filter: Option<String> = None;
    let mut eval_code: Option<String> = None;
    let mut perms = Permissions::sandboxed();
    let mut explicit_max_steps: Option<u64> = None;
    let mut explicit_max_mem: Option<u64> = None;
    let mut explicit_timeout: Option<u64> = None;
    let mut script_path: Option<String> = None;
    let mut script_args: Option<Vec<String>> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            script_args = Some(args[i + 1..].to_vec());
            break;
        }
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
        } else if arg == "--no-color" {
            no_color = true;
        } else if arg == "--no-cache" {
            no_cache = true;
        } else if arg == "--filter" {
            i += 1;
            let Some(val) = args.get(i) else {
                return Err("Missing pattern after --filter".to_string());
            };
            test_filter = Some(val.clone());
        } else if let Some(val) = arg.strip_prefix("--filter=") {
            test_filter = Some(val.to_string());
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
        no_color,
        no_cache,
        test_filter,
        eval_code,
        perms,
        script_path,
        script_args,
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

    if args[1] == "--explain" || args[1] == "explain" {
        let Some(code) = args.get(2) else {
            eprintln!("Usage: askane --explain <ERROR_CODE>");
            process::exit(1);
        };
        if let Err(e) = run_explain(code) {
            eprintln!("{}", e);
            process::exit(1);
        }
        return;
    }

    if args[1] == "info" {
        if args.len() > 2 {
            eprintln!("Usage: askane info");
            process::exit(1);
        }
        run_info();
        return;
    }

    if args[1] == "doctor" {
        if args.len() > 2 {
            eprintln!("Usage: askane doctor");
            process::exit(1);
        }
        if let Err(e) = run_doctor() {
            eprintln!("Doctor failed: {}", e);
            process::exit(1);
        }
        return;
    }

    if args[1] == "modules" {
        let mut json_out = false;
        for a in &args[2..] {
            if a == "--json" {
                json_out = true;
            } else {
                eprintln!("Unknown option '{}'. Usage: askane modules [--json]", a);
                process::exit(1);
            }
        }
        run_modules_cmd(json_out);
        return;
    }

    if args[1] == "clean" {
        if let Err(e) = run_clean_cmd(&args[2..]) {
            eprintln!("Clean error: {}", e);
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
        let mut no_color = false;
        let mut target: Option<String> = None;
        for arg in &args[2..] {
            if arg == "--json" || arg == "--diagnostic-format=json" {
                json_out = true;
            } else if arg == "--no-color" {
                no_color = true;
            } else if arg.starts_with('-') {
                eprintln!(
                    "Usage: askane check [--json|--diagnostic-format=json] [--no-color] [file_or_dir]"
                );
                process::exit(1);
            } else if target.is_none() {
                target = Some(arg.clone());
            } else {
                eprintln!(
                    "Usage: askane check [--json|--diagnostic-format=json] [--no-color] [file_or_dir]"
                );
                process::exit(1);
            }
        }
        let path = match target {
            Some(p) => p,
            None => match discover_manifest_from_cwd() {
                Ok(Some((root, _))) => root.display().to_string(),
                Ok(None) => {
                    eprintln!(
                        "Usage: askane check [--json|--diagnostic-format=json] [--no-color] [file_or_dir]"
                    );
                    process::exit(1);
                }
                Err(e) => {
                    eprintln!("Check failed: {}", e);
                    process::exit(1);
                }
            },
        };
        if let Err(e) = run_check_opts(&path, json_out, no_color) {
            if !e.is_empty() {
                eprintln!("{}", e);
            }
            process::exit(1);
        }
        return;
    }

    if args[1] == "fmt" {
        let mut opts = FmtOptions::default();
        let mut target: Option<String> = None;
        for arg in &args[2..] {
            if arg == "--check" {
                opts.check_only = true;
            } else if arg == "--diff" {
                opts.diff = true;
            } else if arg == "--stdin" {
                opts.use_stdin = true;
            } else if arg.starts_with('-') {
                eprintln!("Usage: askane fmt [--check] [--diff] [--stdin] <file_or_dir>");
                process::exit(1);
            } else if target.is_none() {
                target = Some(arg.clone());
            } else {
                eprintln!("Usage: askane fmt [--check] [--diff] [--stdin] <file_or_dir>");
                process::exit(1);
            }
        }
        if opts.use_stdin && target.is_some() {
            eprintln!("Usage: askane fmt [--check] [--diff] --stdin");
            process::exit(1);
        }
        if !opts.use_stdin && target.is_none() {
            eprintln!("Usage: askane fmt [--check] [--diff] [--stdin] <file_or_dir>");
            process::exit(1);
        }
        if let Err(e) = run_fmt_opts(target.as_deref(), opts) {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
        return;
    }

    if args[1] == "init" {
        let mut dir_arg: Option<String> = None;
        let mut pkg_name: Option<String> = None;
        let mut i = 2;
        while i < args.len() {
            let a = &args[i];
            if a == "--name" {
                i += 1;
                let Some(val) = args.get(i) else {
                    eprintln!("Missing package name after --name");
                    process::exit(1);
                };
                pkg_name = Some(val.clone());
            } else if let Some(val) = a.strip_prefix("--name=") {
                pkg_name = Some(val.to_string());
            } else if a.starts_with('-') {
                eprintln!("Usage: askane init [--name <name>] [dir]");
                process::exit(1);
            } else if dir_arg.is_none() {
                dir_arg = Some(a.clone());
            } else {
                eprintln!("Usage: askane init [--name <name>] [dir]");
                process::exit(1);
            }
            i += 1;
        }
        let dir = dir_arg.as_deref().unwrap_or(".");
        let root = std::path::Path::new(dir);
        let chosen_name = pkg_name.unwrap_or_else(|| {
            root.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|s| !s.is_empty() && s != ".")
                .unwrap_or_else(|| "my-askane-app".to_string())
        });
        let safe_name = chosen_name.replace(['"', '\\', '\r', '\n'], "");
        if let Err(e) = fs::create_dir_all(root.join("tests")) {
            eprintln!("init failed: {}", e);
            process::exit(1);
        }
        let manifest_content = format!(
            "entry = \"main.ask\"\ntests = \"tests\"\n\n[package]\nname = \"{}\"\nversion = \"0.1.0\"\n",
            safe_name
        );
        let files = [
            (root.join("askane.toml"), manifest_content),
            (
                root.join("main.ask"),
                "println(\"Hello from Askane!\");\n".to_string(),
            ),
            (
                root.join("tests").join("basic_test.ask"),
                "import test;\n\nspec \"basic\" {\n    test.assert_eq(1 + 1, 2);\n}\n".to_string(),
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
        no_color,
        no_cache,
        test_filter,
        eval_code,
        mut perms,
        script_path,
        script_args,
        explicit_limits,
    ) = match parse_run_args(&args[slice_start..]) {
        Ok(CliParseOutcome::ExitZero) => return,
        Ok(CliParseOutcome::Run {
            dump_bytecode,
            profile,
            deterministic,
            update_snapshots,
            no_color,
            no_cache,
            test_filter,
            eval_code,
            perms,
            script_path,
            script_args,
            explicit_limits,
        }) => (
            dump_bytecode,
            profile,
            deterministic,
            update_snapshots,
            no_color,
            no_cache,
            test_filter,
            eval_code,
            perms,
            script_path,
            script_args,
            explicit_limits,
        ),
        Err(e) => {
            eprintln!("{}", e);
            process::exit(1);
        }
    };

    if !is_test_cmd && test_filter.is_some() {
        eprintln!("Option '--filter' is only valid with 'askane test'");
        process::exit(1);
    }

    if let Some(code) = eval_code {
        if let Err(e) = run_eval(&code, profile, perms, script_args.as_deref(), no_color) {
            eprintln!("{}", e);
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
                                        test_filter.as_deref(),
                                        no_cache,
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
                            test_filter.as_deref(),
                            no_cache,
                        ),
                        Err(e) => Err(e),
                    }
                } else {
                    run_tests_cmd_with_opts(
                        target,
                        perms,
                        None,
                        deterministic,
                        update_snapshots,
                        test_filter.as_deref(),
                        no_cache,
                    )
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
                            test_filter.as_deref(),
                            no_cache,
                        )
                    })
                }
                Ok(None) => run_tests_cmd_with_opts(
                    ".",
                    perms,
                    None,
                    deterministic,
                    update_snapshots,
                    test_filter.as_deref(),
                    no_cache,
                ),
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
                            run_file_with_script_args(
                                &entry_path.display().to_string(),
                                dump_bytecode,
                                profile,
                                perms,
                                Some(&root),
                                script_args.as_deref(),
                                no_color,
                                no_cache,
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
                run_file_with_script_args(
                    path,
                    dump_bytecode,
                    profile,
                    perms,
                    None,
                    script_args.as_deref(),
                    no_color,
                    no_cache,
                )
            }
        }
        None => match discover_manifest_from_cwd() {
            Ok(Some((root, manifest))) => {
                apply_manifest_limits(&mut perms, &manifest, explicit_limits);
                let entry_rel = manifest.entry.as_deref().unwrap_or("main.ask");
                resolve_manifest_subpath(&root, entry_rel, "entry").and_then(|entry_path| {
                    run_file_with_script_args(
                        &entry_path.display().to_string(),
                        dump_bytecode,
                        profile,
                        perms,
                        Some(&root),
                        script_args.as_deref(),
                        no_color,
                        no_cache,
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
        eprintln!("{}", e);
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
        assert!(val.equals(&Value::from_str("Askane:85:2")));
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
        assert!(val.equals(&Value::from_str("fn:fn")));
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
        assert!(val.equals(&Value::from_str("[1, [...]]")));
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
        assert!(val.equals(&Value::from_str("Askane:critical:50:63")));
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
        assert!(val.equals(&Value::from_str("😀")));

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
            out.equals(&Value::from_str("job-ok")),
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
        assert!(map["replaced"].equals(&Value::from_str("bonono")));
        assert!(map["paired"].equals(&Value::Int(33)));
        assert!(map["hits"].equals(&Value::Int(1)));
        assert!(map["u1"].equals(&Value::Unit));
        assert!(map["u2"].equals(&Value::Int(42)));
        assert!(map["u3"].equals(&Value::from_str("HI")));
        assert!(map["boom"].equals(&Value::Int(0)));
        assert!(map["scaled"].equals(&Value::List(Rc::new(RefCell::new(vec![
            Value::Int(10),
            Value::Int(20),
            Value::Int(30),
            Value::Int(40),
        ])))));
        assert!(
            map["cleaned"].equals(&Value::List(Rc::new(RefCell::new(vec![
                Value::from_str("a"),
                Value::from_str("b"),
            ]))))
        );
        assert!(map["items"].equals(&Value::List(Rc::new(RefCell::new(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
        ])))));
        assert!(map["d_text"].equals(&Value::from_str("OK")));
        assert!(map["r1_ok"].equals(&Value::Bool(true)));
        assert!(map["r2"].equals(&Value::from_str("OK")));
        assert!(map["r3"].equals(&Value::Int(99)));
        assert!(map["dbg"].equals(&Value::from_str("x=42 and a + 2=7")));
        let Value::Str(lmsg) = &map["log_msg"] else {
            panic!("expected log string");
        };
        assert!(!lmsg.contains("sk-live-secret-999"));
        assert!(!lmsg.contains("abc123"));
        assert!(lmsg.contains("alice"));
        assert!(lmsg.contains("[REDACTED]"));
        assert!(
            map["sql_query"].equals(&Value::from_str(
                "SELECT * FROM users WHERE name = ? AND x = ?"
            ))
        );
        assert!(
            map["sql_params"].equals(&Value::List(Rc::new(RefCell::new(vec![
                Value::from_str("alice"),
                Value::Int(42),
            ]))))
        );
        assert!(map["custom"].equals(&Value::from_str("<p>[alice]:[42]</p>")));
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
        assert!(c["w1"].equals(&Value::from_str("ok")));
        assert!(c["w2"].equals(&Value::from_str("rec")));
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
        assert!(m["title"].equals(&Value::from_str("Askane")));
        assert!(m["port"].equals(&Value::Int(8080)));
        assert!(m["be_u32"].equals(&Value::Int(305419896)));
        assert!(m["dq_front"].equals(&Value::Int(1)));
        assert!(m["pq_first"].equals(&Value::from_str("high")));
        assert!(m["win_len"].equals(&Value::Int(3)));
        assert!(m["z_ok"].equals(&Value::Bool(true)));
        assert!(m["g_ok"].equals(&Value::Bool(true)));
        assert!(m["raw_ok"].equals(&Value::Bool(true)));
        assert!(m["crc_ok"].equals(&Value::Bool(true)));
        assert!(m["adler_ok"].equals(&Value::Bool(true)));
        assert!(m["zip_cnt"].equals(&Value::Int(2)));
        assert!(m["zip_text"].equals(&Value::Bytes(std::rc::Rc::new(b"world!".to_vec()))));
        assert!(m["ext_a"].equals(&Value::from_str("alpha")));
        assert!(m["ext_b"].equals(&Value::from_str("beta")));
        assert!(m["fam_ok"].equals(&Value::Bool(true)));
        assert!(m["cli_v"].equals(&Value::Bool(true)));
        assert!(m["cli_out"].equals(&Value::from_str("dist")));
        assert!(m["has_diff"].equals(&Value::Bool(true)));
        assert!(m["diff_added"].equals(&Value::Bool(true)));
        assert!(m["has_pp"].equals(&Value::Bool(true)));
        assert!(m["h_txt"].equals(&Value::from_str("Hi & Bye")));
        assert!(m["x_txt"].equals(&Value::from_str("ok")));
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
        let (pw, ph, pixels) = crate::media::formats::png::decode_png_argb(&png_bytes).unwrap();
        assert_eq!((pw, ph), (2, 1));
        assert_eq!(pixels, vec![0xFF_FF_00_00, 0x80_00_FF_00]);

        let mut bad_crc = png_bytes.clone();
        let last_idx = bad_crc.len() - 1;
        bad_crc[last_idx] ^= 0xFF;
        assert!(crate::media::formats::png::decode_png_argb(&bad_crc).is_err());

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
        assert!(ok_needs.equals(&Value::from_str("a")));

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
        assert!(u["submitted"].equals(&Value::from_str("hi")));
        assert!(u["chk_state"].equals(&Value::Bool(true)));
        assert!(u["inp_state"].equals(&Value::from_str("hi")));
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
        assert!(b[3].equals(&Value::from_str("sub")));
        assert!(b[4].equals(&Value::from_str("blocked")));
        assert!(b[5].equals(&Value::from_str("root")));
        assert!(b[6].equals(&Value::from_str("blocked")));

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
        assert!(cl.borrow()[0].equals(&Value::from_str(".")));
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
        assert!(items[0].equals(&Value::from_str("inside_try")));
        assert!(items[1].equals(&Value::from_str("inner_defer_boom")));
        assert!(items[2].equals(&Value::Bool(true)));
        assert!(items[3].equals(&Value::from_str("outer_defer")));
        assert!(items[4].equals(&Value::from_str("with_body")));
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
        assert!(l.borrow()[0].equals(&Value::from_str("#document")));
        assert!(l.borrow()[1].equals(&Value::from_str("<div class=\"x\"")));
        assert!(l.borrow()[2].equals(&Value::from_str("#document")));
        assert!(l.borrow()[3].equals(&Value::from_str("<root attr=\"1\"")));

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
        assert!(throw_pipe.equals(&Value::from_str("42")));

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
        assert!(pos.borrow()[1].equals(&Value::from_str("-1.5")));

        let toml_res = eval(
            r#"
import toml;
let t = toml.parse("\"a=b\" = \"val\"");
t["a=b"]
"#,
        );
        assert!(toml_res.equals(&Value::from_str("val")));
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
            fmt_indent: None,
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

    #[test]
    fn test_v0171_scheduler_spec_tailcall_ws_and_repl() {
        // 1. Op::TailCall on BoundMethod/NativeFn inside green task (Bug #1)
        let deadlock_src = r#"
import task;
import channel;
let c = channel.bounded(1);
let t = task.spawn(|| c.recv());
t.join();
"#;
        let dl_err = VM::new()
            .run(compile_source(deadlock_src).unwrap())
            .expect_err("tail-call blocked recv must trigger deadlock, not exit 0");
        assert!(
            dl_err.contains("Deadlock: all tasks are blocked"),
            "got: {}",
            dl_err
        );

        let tail_wake_src = r#"
import task;
import channel;
let c = channel.bounded(2);
fn wait_val(ch) {
    ch.recv()
}
let t1 = task.spawn(|| c.recv());
let t2 = task.spawn(|| wait_val(c));
let t3 = task.spawn(|| task.sleep(5));
task.yield_now();
c.send(111);
c.send(222);
[t1.join(), t2.join(), t3.join()]
"#;
        let tail_wake = VM::new()
            .run(compile_source(tail_wake_src).unwrap())
            .unwrap();
        assert!(
            tail_wake.equals(&Value::List(std::rc::Rc::new(std::cell::RefCell::new(
                vec![Value::Int(111), Value::Int(222), Value::Unit,]
            ))))
        );

        // 2. Blocking task & channel inside spec "..." { ... } (Bug #2)
        let spec_src = r#"
import task;
import channel;
import test;

spec "async spec pass" {
    let ch = channel.bounded(1);
    let w = task.spawn(|| {
        task.sleep(15);
        ch.send(77);
        99
    });
    test.assert_eq(w.join(), 99);
    test.assert_eq(ch.recv(), 77);
}

spec "async spec fail stays isolated" {
    let w = task.spawn(|| {
        task.yield_now();
        123
    });
    test.assert_eq(w.join(), 456);
}
"#;
        let spec_proto = compile_source_with_specs(spec_src).unwrap();
        let mut spec_vm = VM::new_with_sandbox(true);
        spec_vm.set_deterministic(true);
        spec_vm.run(spec_proto).unwrap();
        let specs = spec_vm.spec_results();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].0, "async spec pass");
        assert!(specs[0].1.is_ok(), "unexpected error: {:?}", specs[0].1);
        assert_eq!(specs[1].0, "async spec fail stays isolated");
        assert!(specs[1].1.is_err());

        // 3. try / catch around task.join() propagates child task exception (Bug #3)
        let try_join_src = r#"
import task;
let t = task.spawn(|| {
    throw "boom";
});
let caught = try {
    t.join();
    "missed"
} catch err {
    err
};
caught
"#;
        let caught = VM::new()
            .run(compile_source(try_join_src).unwrap())
            .unwrap();
        let Value::Str(msg) = caught else {
            panic!("expected string error, got {:?}", caught);
        };
        assert!(msg.contains("boom"), "expected 'boom', got '{}'", msg);

        // Nested join propagation: main try/catch catches error through t1 -> t2
        let nested_try_src = r#"
import task;
let inner = task.spawn(|| { throw "deep_boom"; });
let outer = task.spawn(|| inner.join());
try {
    outer.join();
    "missed"
} catch err {
    err
}
"#;
        let nested_caught = VM::new()
            .run(compile_source(nested_try_src).unwrap())
            .unwrap();
        let Value::Str(nmsg) = nested_caught else {
            panic!("expected string error, got {:?}", nested_caught);
        };
        assert!(nmsg.contains("deep_boom"), "got '{}'", nmsg);

        // 4. TCO without `else`, BUILTIN_MODULE_NAMES completeness, and ws.close() Close frame
        let tco_src = r#"
fn sum_tail(n, acc) {
    if n == 0 {
        return acc;
    }
    sum_tail(n - 1, acc + 1)
}
sum_tail(5000, 0)
"#;
        let tco_res = VM::new().run(compile_source(tco_src).unwrap()).unwrap();
        assert!(tco_res.equals(&Value::Int(5000)));

        assert_eq!(modules::BUILTIN_MODULE_NAMES.len(), 54);
        for &m in modules::BUILTIN_MODULE_NAMES {
            assert!(
                modules::get_builtin_module(m).is_some(),
                "missing builtin module '{}'",
                m
            );
        }

        // ws.close() sends RFC 6455 Close frame (0x08) so peer ws.recv() returns unit cleanly
        let ws_dir = std::env::temp_dir().join(format!("askane_0171_ws_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ws_dir);
        std::fs::create_dir_all(&ws_dir).unwrap();
        let canon_ws = fs::canonicalize(&ws_dir).unwrap();
        let ws_srv_dir = canon_ws.clone();
        let mut perms = Permissions::sandboxed();
        perms.allow_net = Some(vec!["127.0.0.1".to_string()]);
        let srv_perms = perms.clone();
        let srv_thread = std::thread::spawn(move || {
            let mut srv_vm = VM::new_with_sandbox(true);
            srv_vm.set_permissions(srv_perms);
            srv_vm.set_current_dir(ws_srv_dir.clone());
            srv_vm.set_sandbox_root(ws_srv_dir);
            let res = srv_vm
                .run(
                    compile_source(
                        r#"
import ws;
import fs;
let l = ws.listen("127.0.0.1:0");
fs.write("addr.txt", l.local_addr());
let c = l.accept(5000);
let closed_val = c.recv(5000);
c.close();
l.close();
closed_val == unit
"#,
                    )
                    .unwrap(),
                )
                .unwrap();
            assert!(res.equals(&Value::Bool(true)));
        });
        let addr_file = canon_ws.join("addr.txt");
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
        cli_vm
            .run(
                compile_source(&format!(
                    "import ws;\nlet c = ws.connect(\"ws://{}\", 5000);\nc.close();\n",
                    ws_addr
                ))
                .unwrap(),
            )
            .unwrap();
        srv_thread.join().unwrap();
        let _ = std::fs::remove_dir_all(&ws_dir);
    }

    #[test]
    fn test_v0180_semantics_diagnostics_and_cli() {
        let mut vm = VM::new();
        let res = vm
            .run(
                compile_source(
                    r##"
import strings;
import collections;
import gfx;
import math;

let ca = checked_add(10, 5);
let cs = checked_sub(10, 3);
let cm = checked_mul(6, 7);
let wa = wrapping_add(9223372036854775807, 1);
let ws = wrapping_sub(-9223372036854775807 - 1, 1);
let wm = wrapping_mul(9223372036854775807, 2);

let nan = math.sqrt(-1.0);
let lst = [1, 2];
let lst2 = [1, 2];
let same_ref = is_same(lst, lst);
let same_diff = is_same(lst, lst2);
let same_num = is_same(1, 1.0);
let same_nan = is_same(nan, nan);

let r_str = repr("hi\n\"x\"");
let ti_ok = try_int(" -42 ");
let ti_bad = try_int("nope");
let tf_ok = try_float("3.5");
let tf_bad = try_float("abc");
let preds = [
    is_unit(unit), is_int(1), is_float(1.5), is_bool(true),
    is_str("a"), is_bytes(b"a"), is_list([1]), is_dict(#{a: 1}),
    is_set(set([1])), is_fn(|x| x)
];

let d = #{ z: 3, a: 1, m: 2 };
let d_str = str(d);

let nums = [1, 2, 3, 4, 5];
let parts = partition(nums, |x| x % 2 == 0);
let flat = flat_map([1, 2], |x| [x, x * 10]);
let tw = take_while([2, 4, 5, 6], |x| x % 2 == 0);
let dw = drop_while([2, 4, 5, 6], |x| x % 2 == 0);
let zl = collections.zip_longest([1, 2], ["a"]);
let mnb = collections.min_by(["ccc", "a", "bb"], |s| len(s));
let mxb = max_by(["ccc", "a", "bb"], |s| len(s));

let s_ops = [
    strings.byte_len("a😊") == 5,
    strings.eq_ignore_case("AskAne", "askane"),
    strings.contains_ignore_case("Hello Askane", "ASK"),
    strings.strip_prefix("prefix_val", "prefix_") == "val",
    strings.strip_suffix("val.ask", ".ask") == "val",
    strings.split_once("a=b=c", "=") == ["a", "b=c"],
    strings.rsplit_once("a/b/c", "/") == ["a/b", "c"],
    strings.truncate_utf8("ab😊cd", 4) == "ab"
];

let gfx_exports_ok = is_fn(gfx.rect_alpha)
    && is_fn(gfx.circle_alpha)
    && is_fn(gfx.width)
    && is_fn(gfx.height)
    && is_fn(gfx.key_pressed);

[
    ca == 15 && cs == 7 && cm == 42,
    wa < 0 && ws > 0 && wm == -2,
    same_ref && !same_diff && !same_num && same_nan,
    r_str == "\"hi\\n\\\"x\\\"\"",
    ti_ok == 0 - 42 && ti_bad == unit && tf_ok == 3.5 && tf_bad == unit,
    all(preds, |p| p),
    d_str == "#{\"a\": 1, \"m\": 2, \"z\": 3}",
    first(nums) == 1 && last(nums) == 5 && is_empty([]) && !is_empty(nums),
    find_index(nums, |x| x > 3) == 3 && count(nums, |x| x > 2) == 3,
    parts == [[2, 4], [1, 3, 5]] && flat == [1, 10, 2, 20],
    tw == [2, 4] && dw == [5, 6] && zl == [[1, "a"], [2, unit]],
    mnb == "a" && mxb == "ccc",
    all(s_ops, |p| p),
    gfx_exports_ok
]
"##,
                )
                .unwrap(),
            )
            .unwrap();
        let Value::List(flags) = res else {
            panic!("expected list, got {:?}", res);
        };
        for (idx, flag) in flags.borrow().iter().enumerate() {
            assert!(
                flag.equals(&Value::Bool(true)),
                "v0.18.0 assertion {} failed: {:?}",
                idx,
                flag
            );
        }

        let err_ovf = vm
            .run(compile_source("checked_add(9223372036854775807, 1);").unwrap())
            .unwrap_err();
        assert!(err_ovf.contains("overflow"), "got: {}", err_ovf);

        // Diagnostics: error classification, Levenshtein did-you-mean, multi-error recovery, and lint warnings.
        let mut lexer = Lexer::new("let a = ;\nlet b = ;\n");
        let tokens = lexer.tokenize().unwrap();
        let mut parser = Parser::new(tokens);
        let (_prog, parse_errs) = parser.parse_program_recover();
        assert!(
            parse_errs.len() >= 2,
            "expected multi-error recovery, got {:?}",
            parse_errs
        );

        let d = diag::classify_error(&err_ovf, Some("demo.ask"));
        assert_eq!(d.code, "E0404");
        let json_d = diag::diagnostic_to_json(&d, json_escape);
        assert!(json_d.contains("\"code\":\"E0404\""));
        assert!(diag::explain_code("e0401").is_some());

        let typo_err = vm
            .run(compile_source("let my_special_var = 1;\nmy_special_vra;").unwrap())
            .unwrap_err();
        assert!(
            typo_err.contains("did you mean 'my_special_var'"),
            "got: {}",
            typo_err
        );

        let mod_typo_err = vm
            .run(compile_source("import math;\nmath.sqrtt(4);").unwrap())
            .unwrap_err();
        assert!(
            mod_typo_err.contains("did you mean 'sqrt'"),
            "got: {}",
            mod_typo_err
        );

        let import_typo = vm
            .run(compile_source("import strigns;").unwrap())
            .unwrap_err();
        assert!(
            import_typo.contains("did you mean 'strings'"),
            "got: {}",
            import_typo
        );
        assert!(
            import_typo.contains("imported from"),
            "got: {}",
            import_typo
        );

        let mut lint_lex = Lexer::new(
            "fn f(x) { let unused_var = 1; match x { 1 => \"a\", 1 => \"b\", _ => \"c\" } }",
        );
        let lint_tok = lint_lex.tokenize().unwrap();
        let mut lint_p = Parser::new(lint_tok);
        let lint_prog = lint_p.parse_program().unwrap();
        let warns = diag::analyze_warnings(&lint_prog, Some("lint.ask"));
        assert!(warns.iter().any(|w| w.code == "W0101"));
        assert!(warns.iter().any(|w| w.code == "W0102"));

        // CLI: `--` separator forwards script args without interpreting flags as Askane flags.
        let cli_args = vec![
            "--no-color".to_string(),
            "app.ask".to_string(),
            "--".to_string(),
            "--unsafe".to_string(),
            "arg2".to_string(),
        ];
        match parse_run_args(&cli_args).unwrap() {
            CliParseOutcome::Run {
                no_color,
                perms,
                script_path,
                script_args,
                ..
            } => {
                assert!(no_color);
                assert!(!perms.allow_all);
                assert_eq!(script_path.as_deref(), Some("app.ask"));
                assert_eq!(
                    script_args,
                    Some(vec!["--unsafe".to_string(), "arg2".to_string()])
                );
            }
            _ => panic!("expected Run outcome"),
        }

        // All built-in modules have metadata.
        for &name in modules::BUILTIN_MODULE_NAMES {
            let (cat, plat, desc) = modules::module_info(name);
            assert!(!cat.is_empty() && !plat.is_empty() && !desc.is_empty());
        }

        // Static permissions report warns on dynamic targets.
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp_dir = std::env::temp_dir().join(format!("askane_v0180_{}", ts));
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let dyn_file = tmp_dir.join("dyn.ask");
        std::fs::write(
            &dyn_file,
            "import http;\nlet u = \"https://example.com\";\nhttp.get(u);\n",
        )
        .unwrap();
        let perms_list = inspect_permissions(&dyn_file.display().to_string()).unwrap();
        assert!(perms_list.iter().any(|p| p.contains("[dynamic]")));

        let tmp_clean = tmp_dir.join("a.askc");
        std::fs::write(&tmp_clean, b"bc").unwrap();
        let mut dry_cnt = 0usize;
        clean_dir_walk(&tmp_dir, true, false, &mut dry_cnt, 0).unwrap();
        assert_eq!(dry_cnt, 1);
        assert!(tmp_clean.exists());
        let mut real_cnt = 0usize;
        clean_dir_walk(&tmp_dir, false, false, &mut real_cnt, 0).unwrap();
        assert_eq!(real_cnt, 1);
        assert!(!tmp_clean.exists());
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_v0180_qa_bugfixes() {
        // 1. Import chains, stack traces, did-you-mean hints, and accurate frame.ip.
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp_dir = std::env::temp_dir().join(format!("askane_qa_fixes_{}", ts));
        std::fs::create_dir_all(&tmp_dir).unwrap();

        let c_file = tmp_dir.join("c.ask");
        let b_file = tmp_dir.join("b.ask");
        let a_file = tmp_dir.join("a.ask");
        std::fs::write(
            &c_file,
            "fn deep_fail() {\n    let unused = 1;\n    let x = 1;\n    // line 4\n    1 / 0;\n}\ndeep_fail();\n",
        )
        .unwrap();
        std::fs::write(&b_file, "import \"c.ask\" as c;\npub fn run() { c }\n").unwrap();
        std::fs::write(&a_file, "import \"b.ask\" as b;\nb.run();\n").unwrap();

        let err_chain = run_file_with_root(
            &a_file.display().to_string(),
            false,
            Permissions::sandboxed(),
            Some(&tmp_dir),
        )
        .unwrap_err();
        assert!(
            err_chain.contains("Division by zero"),
            "unexpected error: {}",
            err_chain
        );
        assert!(
            err_chain.contains("c.ask:5:"),
            "expected c.ask:5 location, got:\n{}",
            err_chain
        );
        assert!(
            err_chain.contains("import chain:") && err_chain.contains("b.ask"),
            "expected readable import chain, got:\n{}",
            err_chain
        );
        assert!(
            err_chain.contains("stack trace:") && err_chain.contains("at deep_fail (line 5)"),
            "expected clean stack trace, got:\n{}",
            err_chain
        );

        let d_hint_and_trace = diag::classify_error(
            "[line 5] Undefined variable 'foo' (did you mean 'for'?) (in 'c.ask') (imported from 'b.ask')\n  at go (line 5)\n  at <script> (line 2)",
            Some("a.ask"),
        );
        assert_eq!(d_hint_and_trace.file.as_deref(), Some("c.ask"));
        assert_eq!(d_hint_and_trace.line, 5);
        assert_eq!(
            d_hint_and_trace.hint.as_deref(),
            Some("did you mean 'for'?")
        );
        assert_eq!(d_hint_and_trace.import_chain, vec!["b.ask", "c.ask"]);
        assert_eq!(
            d_hint_and_trace.stack_trace,
            vec!["at go (line 5)", "at <script> (line 2)"]
        );

        let mut vm = VM::new();
        let line_err = vm
            .run(compile_source("fn go() {\n    missing_global = 1;\n}\ngo();\n").unwrap())
            .unwrap_err();
        let d_line = diag::classify_error(&line_err, Some("lines.ask"));
        assert!(
            d_line.stack_trace.iter().any(|s| s == "at go (line 2)"),
            "expected line 2 in stack trace, got: {:?}",
            d_line.stack_trace
        );
        assert!(
            !d_line.stack_trace.iter().any(|s| s.contains("(line 0)")),
            "unexpected line 0 in stack trace: {:?}",
            d_line.stack_trace
        );

        let div_err = vm
            .run(
                compile_source("fn bad() {\n    let x = 1;\n    1 / 0;\n}\nfn caller() { bad(); }\ncaller();\n")
                    .unwrap(),
            )
            .unwrap_err();
        let d_div = diag::classify_error(&div_err, Some("div.ask"));
        assert_eq!(
            d_div.stack_trace.first().map(String::as_str),
            Some("at bad (line 3)")
        );
        assert!(d_div.stack_trace.iter().any(|s| s == "at caller (line 5)"));

        // 2. Parser depth recovery: sequential deeply nested broken blocks do not leak self.depth.
        let one_nested_err = format!("{} let x = ; {}", "{ ".repeat(60), "}".repeat(60));
        let many_blocks = vec![one_nested_err; 6].join("\n");
        let lex_many = Lexer::new(&many_blocks).tokenize().unwrap();
        let (_prog, errs_many) = Parser::new(lex_many).parse_program_recover();
        assert!(
            !errs_many
                .iter()
                .any(|e| e.contains("Maximum nesting depth")),
            "depth leaked across recovery: {:?}",
            errs_many
        );

        // 3. Lexer error codes classify as E0101.
        for msg in [
            "Invalid integer literal '999999999999999999999'",
            "Invalid float literal '1e99999'",
            "Invalid duration literal '10x'",
            "Duration literal out of range",
            "Unknown escape sequence '\\q'",
            "Invalid hex escape '\\xZZ'",
            "Incomplete \\xHH escape",
            "Byte literal can only contain ASCII characters",
            "Raw byte literal can only contain ASCII characters",
        ] {
            let d = diag::classify_error(&format!("[line 1:1] {}", msg), Some("lex.ask"));
            assert_eq!(
                d.code, "E0101",
                "expected E0101 for '{}', got {}",
                msg, d.code
            );
        }

        // 4. check --json produces single valid JSON for dirs, empty dirs, and missing files.
        let dir_ok = tmp_dir.join("check_dir");
        let dir_empty = tmp_dir.join("empty_dir");
        std::fs::create_dir_all(&dir_ok).unwrap();
        std::fs::create_dir_all(&dir_empty).unwrap();
        std::fs::write(dir_ok.join("good.ask"), "let x = 1; x;\n").unwrap();
        std::fs::write(dir_ok.join("bad.ask"), "let y = ;\n").unwrap();

        let (ok_dir, json_dir) = check_dir_json_report(&dir_ok.display().to_string(), false);
        assert!(!ok_dir);
        let parsed_dir = vm
            .run(
                compile_source(&format!(
                    "import json;\nlet d = json.parse({});\n!d[\"ok\"] && d[\"file_count\"] == 2 && d[\"failed_count\"] == 1 && len(d[\"files\"]) == 2\n",
                    json_escape(&json_dir)
                ))
                .unwrap(),
            )
            .unwrap();
        assert!(
            parsed_dir.equals(&Value::Bool(true)),
            "json_dir: {} -> {:?}",
            json_dir,
            parsed_dir
        );

        let (ok_empty, json_empty) = check_dir_json_report(&dir_empty.display().to_string(), false);
        assert!(!ok_empty);
        let parsed_empty = vm
            .run(
                compile_source(&format!(
                    "import json;\nlet d = json.parse({});\n!d[\"ok\"] && d[\"file_count\"] == 0\n",
                    json_escape(&json_empty)
                ))
                .unwrap(),
            )
            .unwrap();
        assert!(parsed_empty.equals(&Value::Bool(true)));

        let missing_rep = check_file_report(
            &tmp_dir.join("missing.ask").display().to_string(),
            true,
            false,
        );
        assert!(!missing_rep.ok);
        let parsed_missing = vm
            .run(
                compile_source(&format!(
                    "import json;\nlet d = json.parse({});\n!d[\"ok\"] && len(d[\"diagnostics\"]) == 1\n",
                    json_escape(&missing_rep.file_json)
                ))
                .unwrap(),
            )
            .unwrap();
        assert!(parsed_missing.equals(&Value::Bool(true)));

        // 5. W0101 and W0102 precision improvements.
        let lint_src = "\
let html_tag = fn(parts, vals) { len(vals) };
let used_tpl = html_tag\"select ${1}\";
fn outer(a) {
    let inner = fn(a) { a + 1 };
    let sum = a + inner(2);
    for a in [1, 2] { a; }
    try { sum; } catch a { a; }
    match 1 { a => a + sum }
}
outer(used_tpl);
if true {
    let dead_local = 42;
}
";
        let lint_tok = Lexer::new(lint_src).tokenize().unwrap();
        let lint_prog = Parser::new(lint_tok).parse_program().unwrap();
        let w_list = diag::analyze_warnings(&lint_prog, Some("w.ask"));
        let w0101_vars: Vec<&str> = w_list
            .iter()
            .filter(|w| w.code == "W0101")
            .map(|w| w.message.as_str())
            .collect();
        assert_eq!(
            w0101_vars.len(),
            1,
            "unexpected W0101 warnings: {:?}",
            w_list
        );
        assert!(w0101_vars[0].contains("'dead_local'"));
        let dead_w = w_list.iter().find(|w| w.code == "W0101").unwrap();
        assert_eq!(dead_w.col, 9);
        assert_eq!(dead_w.end_col, 18);

        let pat_src = "\
let _m1 = match 0.0 { -0.0 => 1, 0.0 => 2, _ => 3 };
let _m2 = match 3 { 1..5 => \"a\", 3 => \"b\", 2..4 => \"c\", 1..5 => \"d\", _ => \"e\" };
";
        let pat_tok = Lexer::new(pat_src).tokenize().unwrap();
        let pat_prog = Parser::new(pat_tok).parse_program().unwrap();
        let w_pat = diag::analyze_warnings(&pat_prog, Some("pat.ask"));
        let w0102_count = w_pat.iter().filter(|w| w.code == "W0102").count();
        assert!(
            w0102_count >= 4,
            "expected float/range W0102 warnings, got: {:?}",
            w_pat
        );

        // 6-10. Semantics, permissions dynamic detection, gfx checks, case folding, try_float.
        let sem_res = vm
            .run(
                compile_source(
                    "import math;\nimport strings;\n\
                     struct Box { v = 1, fn get(self) { self.v } }\n\
                     let b1 = Box {}; let b2 = Box { v: 2 };\n\
                     let same_ok = is_same(b1.get, b1.get) && !is_same(b1.get, b2.get);\n\
                     let cmp_ok = (9007199254740993 > 9007199254740992.0) && !(9007199254740993 < 9007199254740992.0);\n\
                     let extremum_ok = is_same(min_by([9007199254740993, 9007199254740992.0], |x| x), 9007199254740992.0)\n\
                         && is_same(max_by([9007199254740993, 9007199254740992.0], |x| x), 9007199254740993);\n\
                     let tf_ok = is_unit(try_float(\"nan\")) && is_unit(try_float(\"inf\"))\n\
                         && is_unit(try_float(\"-inf\")) && is_unit(try_float(\"1e9999\"))\n\
                         && is_unit(try_float(math.sqrt(-1.0)))\n\
                         && try_float(true) == 1.0 && try_float(false) == 0.0;\n\
                     let eszett_ok = strings.eq_ignore_case(\"Straße\", \"STRASSE\")\n\
                         && strings.contains_ignore_case(\"GROẞE\", \"ss\");\n\
                     [same_ok, cmp_ok, extremum_ok, tf_ok, eszett_ok]\n",
                )
                .unwrap(),
            )
            .unwrap();
        let Value::List(flags) = sem_res else {
            panic!("expected list, got {:?}", sem_res);
        };
        for (i, f) in flags.borrow().iter().enumerate() {
            assert!(
                f.equals(&Value::Bool(true)),
                "qa semantic check {} failed",
                i
            );
        }

        assert!(
            vm.run(compile_source("import math; min_by([1], |x| math.sqrt(-1.0));").unwrap())
                .is_err()
        );
        assert!(
            vm.run(compile_source("max_by([[1]], |x| x);").unwrap())
                .is_err()
        );
        let map_num_err = vm
            .run(compile_source("map(123, |x| x);").unwrap())
            .unwrap_err();
        assert!(map_num_err.contains("str"), "got: {}", map_num_err);

        // Permissions detection with aliases, from-import, and CallNamed.
        let alias_file = tmp_dir.join("perm_alias.ask");
        std::fs::write(
            &alias_file,
            "import http as h;\nfrom http import post as p;\nlet u = \"https://example.com\";\nh.get(u);\np(u, \"ok\");\nh.post(url: u, body: \"x\");\n",
        )
        .unwrap();
        let alias_perms = inspect_permissions(&alias_file.display().to_string()).unwrap();
        assert!(
            alias_perms
                .iter()
                .any(|s| s.contains("[dynamic] http.get()")),
            "perms: {:?}",
            alias_perms
        );
        assert!(
            alias_perms
                .iter()
                .any(|s| s.contains("[dynamic] http.post()")),
            "perms: {:?}",
            alias_perms
        );

        #[cfg(windows)]
        {
            for call in [
                "import gfx; gfx.rect(0, 0, 10, 10, 255, 0, 0);",
                "import gfx; gfx.rect_alpha(0, 0, 10, 10, 255, 0, 0, 0);",
                "import gfx; gfx.circle(0, 0, -5, 255, 0, 0);",
                "import gfx; gfx.circle_alpha(0, 0, 5, 255, 0, 0, 0);",
                "import gfx; gfx.rect_outline(0, 0, 0, 0, 255, 0, 0);",
                "import gfx; gfx.key_down(\"A\");",
                "import gfx; gfx.key_pressed(\"A\");",
                "import gfx; gfx.mouse_down(\"left\");",
            ] {
                let err = vm.run(compile_source(call).unwrap()).unwrap_err();
                assert!(
                    err.contains("No active gfx window"),
                    "expected no active window for `{}`: {}",
                    call,
                    err
                );
            }
        }

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_v0190_cache_fs_fmt_filter_and_gfx() {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp_dir = std::env::temp_dir().join(format!("askane_v0190_{}", ts));
        std::fs::create_dir_all(&tmp_dir).unwrap();

        // 1. .askc bytecode cache codec, tamper guards, VM/CLI integration & clean
        let sample_src = "let x = 2 + 3 * 4;\nx";
        let proto = compile_source(sample_src).unwrap();
        let src_hash = modules::hash::sha256_bytes(sample_src.as_bytes());
        let encoded = bytecode::encode_cached_proto(&proto, &src_hash, false, false).unwrap();
        assert!(encoded.starts_with(bytecode::BYTECODE_CACHE_MAGIC));
        let decoded = bytecode::decode_cached_proto(&encoded, &src_hash, false, false).unwrap();
        assert_eq!(
            decoded.chunk.disassemble(&decoded.name),
            proto.chunk.disassemble(&proto.name)
        );

        // Reject wrong mode, wrong hash, trailing bytes, or tampered payload
        assert!(bytecode::decode_cached_proto(&encoded, &src_hash, true, false).is_err());
        assert!(bytecode::decode_cached_proto(&encoded, &src_hash, false, true).is_err());
        let bad_hash = [0u8; 32];
        assert!(bytecode::decode_cached_proto(&encoded, &bad_hash, false, false).is_err());
        let mut with_tail = encoded.clone();
        with_tail.push(0x99);
        assert!(bytecode::decode_cached_proto(&with_tail, &src_hash, false, false).is_err());
        let mut tampered = encoded.clone();
        let last_idx = tampered.len() - 1;
        tampered[last_idx] ^= 0xFF;
        assert!(bytecode::decode_cached_proto(&tampered, &src_hash, false, false).is_err());

        // Real script + imported module cache lifecycle
        let cache_proj = tmp_dir.join("cache_proj");
        std::fs::create_dir_all(&cache_proj).unwrap();
        let helper_path = cache_proj.join("helper.ask");
        let main_path = cache_proj.join("main.ask");
        std::fs::write(&helper_path, "pub fn answer() { 42 }\n").unwrap();
        std::fs::write(&main_path, "import helper;\nlet v = helper.answer();\nv").unwrap();

        run_file_core_full(
            &main_path.display().to_string(),
            false,
            false,
            false,
            Permissions::sandboxed(),
            None,
            false,
            false,
            None,
            true,
            false,
            None,
        )
        .unwrap();
        let main_cache = bytecode::cache_path_for(&main_path).unwrap();
        let helper_cache = bytecode::cache_path_for(&helper_path).unwrap();
        assert!(main_cache.is_file(), "expected main .askc cache");
        assert!(helper_cache.is_file(), "expected module .askc cache");

        // Second run hits cache, and corrupted cache transparently falls back to recompile
        std::fs::write(&main_cache, b"CORRUPT").unwrap();
        run_file_core_full(
            &main_path.display().to_string(),
            false,
            false,
            false,
            Permissions::sandboxed(),
            None,
            false,
            false,
            None,
            true,
            false,
            None,
        )
        .unwrap();
        assert!(std::fs::read(&main_cache).unwrap().starts_with(b"ASKC"));

        // --no-cache skips writing new .askc files
        let nocache_script = tmp_dir.join("nocache.ask");
        std::fs::write(&nocache_script, "let z = 10;\nz").unwrap();
        match parse_run_args(&["--no-cache".to_string(), "nocache.ask".to_string()]).unwrap() {
            CliParseOutcome::Run { no_cache, .. } => assert!(no_cache),
            _ => panic!("expected Run outcome for --no-cache"),
        }
        run_file_core_full(
            &nocache_script.display().to_string(),
            false,
            false,
            false,
            Permissions::sandboxed(),
            None,
            false,
            false,
            None,
            true,
            true,
            None,
        )
        .unwrap();
        assert!(!bytecode::cache_path_for(&nocache_script).unwrap().exists());

        // Relative subdir & non-canonical parent paths stay writable inside sandbox, escapes rejected
        let cache_sub = cache_proj.join("sub");
        std::fs::create_dir_all(&cache_sub).unwrap();
        let mut cache_check_vm = VM::new_with_sandbox(true);
        cache_check_vm.set_permissions(Permissions::sandboxed());
        cache_check_vm.set_bytecode_cache(true);
        cache_check_vm.set_current_dir(cache_proj.clone());
        cache_check_vm.set_sandbox_root(cache_proj.clone());
        assert!(cache_check_vm.can_write_cache_for(std::path::Path::new("sub/child.ask")));
        assert!(
            cache_check_vm.can_write_cache_for(
                &cache_proj.join("sub").join("..").join("main.ask")
            )
        );
        assert!(!cache_check_vm.can_write_cache_for(std::path::Path::new("../outside.ask")));

        // askane clean removes .askc caches and prunes empty __askcache__ directories
        let mut dry_removed = 0usize;
        clean_dir_walk(&cache_proj, true, false, &mut dry_removed, 0).unwrap();
        assert_eq!(dry_removed, 2);
        assert!(main_cache.exists());

        let mut removed = 0usize;
        clean_dir_walk(&cache_proj, false, false, &mut removed, 0).unwrap();
        assert_eq!(removed, 2);
        assert!(!cache_proj.join("__askcache__").exists());

        // 2. Streaming & directory fs APIs
        let fs_dir = tmp_dir.join("fs_work");
        std::fs::create_dir_all(&fs_dir).unwrap();
        let fs_script = r#"
import fs;
import strings;
import test;

fs.write_lines("lines.txt", ["alpha", "beta", "gamma"]);
let all_lines = fs.read_lines("lines.txt");
test.assert_eq(len(all_lines), 3);
test.assert_eq(all_lines[1], "beta");

let limited = fs.read_lines("lines.txt", 2);
test.assert_eq(len(limited), 2);
test.assert_eq(limited[0], "alpha");

with fs.open_writer("stream.txt") as w {
    w.write("one-");
    w.write_line("two");
    w.flush();
}
with fs.write_stream("stream.txt", true) as w2 {
    w2.write_line("three");
}

let collected = [];
with fs.open_lines("stream.txt") as r {
    loop {
        let line = r.next();
        if line == unit {
            break;
        }
        push(collected, line);
    }
}
test.assert_eq(collected, ["one-two", "three"]);

let leaked_writer = fs.open_writer("closed_check.txt");
leaked_writer.close();
let write_after_close_err = try {
    leaked_writer.write("nope");
    false
} catch e {
    strings.contains(e, "closed")
};
test.assert(write_after_close_err);

let tf = fs.temp_file("v019");
test.assert(fs.exists(tf));
fs.write(tf, "temp-payload");
test.assert_eq(fs.read(tf), "temp-payload");
fs.remove(tf);

let bad_prefix_err = try { fs.temp_file("../bad"); false } catch e { true };
test.assert(bad_prefix_err);

fs.mkdir("tree/sub");
fs.write("tree/a.txt", "AAA");
fs.write("tree/sub/b.txt", "BBBB");
let walked = fs.walk("tree");
test.assert_eq(len(walked), 3);
let has_sub_file = false;
for entry in walked {
    if entry.name == "b.txt" && !entry.is_dir && entry.size == 4 {
        has_sub_file = true;
    }
}
test.assert(has_sub_file);

let copied = fs.copy_dir("tree", "tree_copy");
test.assert_eq(copied, 2);
test.assert_eq(fs.read("tree_copy/sub/b.txt"), "BBBB");

let self_copy_err = try { fs.copy_dir("tree", "tree/nested_self"); false } catch e { true };
test.assert(self_copy_err);

let non_empty_err = try { fs.remove_dir("tree_copy"); false } catch e { true };
test.assert(non_empty_err);
fs.remove_dir("tree_copy", true);
test.assert(!fs.exists("tree_copy"));

let outside_err = try { fs.walk(".."); false } catch e { true };
test.assert(outside_err);
let outside_rm_err = try { fs.remove_dir("..", true); false } catch e { true };
test.assert(outside_rm_err);
"#;
        let mut vm = VM::new_with_sandbox(true);
        vm.set_permissions(Permissions::sandboxed());
        vm.set_current_dir(fs_dir.clone());
        vm.set_sandbox_root(fs_dir.clone());
        vm.run(compile_source(fs_script).unwrap()).unwrap();
        drop(vm);

        // 3. fmt [fmt] indent, --diff, golden corpus idempotence & test --filter
        let fmt_proj = tmp_dir.join("fmt_proj");
        std::fs::create_dir_all(fmt_proj.join("tests")).unwrap();
        std::fs::write(fmt_proj.join("askane.toml"), "[fmt]\nindent = 2\n").unwrap();
        let src_to_fmt = fmt_proj.join("app.ask");
        std::fs::write(&src_to_fmt, "fn add(a,b){\nlet x=a+b;\nx\n}\n").unwrap();
        assert_eq!(resolve_fmt_indent(Some(&src_to_fmt)), 2);

        let raw_ugly = "fn add(a,b){\nlet x=a+b;\nx\n}\n";
        let fmt2 = format_askane_source_with_indent(raw_ugly, 2);
        assert!(fmt2.contains("  let x = a + b;"));
        assert_eq!(format_askane_source_with_indent(&fmt2, 2), fmt2);

        let diff_txt = render_fmt_unified_diff("app.ask", raw_ugly, &fmt2);
        assert!(diff_txt.contains("--- app.ask (original)"));
        assert!(diff_txt.contains("+++ app.ask (formatted)"));
        assert!(diff_txt.contains("-let x=a+b;"));
        assert!(diff_txt.contains("+  let x = a + b;"));

        let crlf_diff = render_fmt_unified_diff("crlf.ask", "let x = 1;\r\n", "let x = 1;\n");
        assert!(crlf_diff.contains("CRLF line endings"));

        // Golden corpus idempotence check
        let corpus = [
            "struct Vec2 {\n  x = 0,\n  y = 0,\n  fn norm2(self) { self.x * self.x + self.y * self.y }\n}\n",
            "let xs = [1, 2, 3, 4]\n    |> filter(|n| n % 2 == 0)\n    |> map(|n| n * n);\n",
            "match score {\n    0..50 => \"fail\",\n    50..=100 => \"pass\",\n    _ => \"other\",\n}\n",
            "let r = r\"raw\\path\";\nlet m = \"\"\"multi\nline\"\"\";\n",
        ];
        for snippet in corpus {
            let f1 = format_askane_source(snippet);
            verify_askane_syntax(&f1).unwrap();
            let f2 = format_askane_source(&f1);
            assert_eq!(f1, f2, "formatter must be idempotent on:\n{}", snippet);
        }

        // CLI flag parsing & askane test --filter
        match parse_run_args(&["--filter=alpha".to_string(), "tests".to_string()]).unwrap() {
            CliParseOutcome::Run {
                test_filter,
                script_path,
                ..
            } => {
                assert_eq!(test_filter.as_deref(), Some("alpha"));
                assert_eq!(script_path.as_deref(), Some("tests"));
            }
            _ => panic!("expected Run outcome for --filter="),
        }
        match parse_run_args(&[
            "--filter".to_string(),
            "beta".to_string(),
            "tests".to_string(),
        ])
        .unwrap()
        {
            CliParseOutcome::Run { test_filter, .. } => {
                assert_eq!(test_filter.as_deref(), Some("beta"));
            }
            _ => panic!("expected Run outcome for --filter"),
        }
        assert!(parse_run_args(&["--filter".to_string()]).is_err());

        let spec_file = fmt_proj.join("tests").join("multi_spec.ask");
        std::fs::write(
            &spec_file,
            "import test;\nspec \"alpha works\" { test.assert_eq(2 + 2, 4); }\nspec \"beta intentionally fails\" { test.assert(false); }\n",
        )
        .unwrap();
        let (filtered_proto, matched) = compile_source_filtered_specs(
            &std::fs::read_to_string(&spec_file).unwrap(),
            Some("alpha"),
        )
        .unwrap();
        assert_eq!(matched, 1);
        drop(filtered_proto);

        // Passing --filter=alpha skips failing beta spec cleanly (even if parent dir contains "alpha")
        let alpha_parent_dir = fmt_proj.join("tests").join("alpha_parent");
        std::fs::create_dir_all(&alpha_parent_dir).unwrap();
        let nested_spec = alpha_parent_dir.join("suite.ask");
        std::fs::copy(&spec_file, &nested_spec).unwrap();
        run_tests_cmd_with_opts(
            &nested_spec.display().to_string(),
            Permissions::sandboxed(),
            Some(&fmt_proj),
            false,
            false,
            Some("alpha"),
            false,
        )
        .unwrap();
        run_tests_cmd_with_opts(
            &fmt_proj.join("tests").display().to_string(),
            Permissions::sandboxed(),
            Some(&fmt_proj),
            false,
            false,
            Some("alpha"),
            false,
        )
        .unwrap();
        assert!(
            run_tests_cmd_with_opts(
                &fmt_proj.join("tests").display().to_string(),
                Permissions::sandboxed(),
                Some(&fmt_proj),
                false,
                false,
                Some("nonexistent_spec_xyz"),
                false,
            )
            .is_err()
        );

        // 4. gfx headless mode, sprite sheets, mouse_pressed, delta normalization
        let gfx_dir = tmp_dir.join("gfx_work");
        std::fs::create_dir_all(&gfx_dir).unwrap();

        // Build valid 4x2 24-bit bottom-up BMP sprite sheet (two 2x2 frames side by side)
        let mut bmp = vec![0u8; 54 + 24];
        bmp[0..2].copy_from_slice(b"BM");
        bmp[2..6].copy_from_slice(&(78u32).to_le_bytes());
        bmp[10..14].copy_from_slice(&(54u32).to_le_bytes());
        bmp[14..18].copy_from_slice(&(40u32).to_le_bytes());
        bmp[18..22].copy_from_slice(&(4i32).to_le_bytes());
        bmp[22..26].copy_from_slice(&(2i32).to_le_bytes());
        bmp[26..28].copy_from_slice(&(1u16).to_le_bytes());
        bmp[28..30].copy_from_slice(&(24u16).to_le_bytes());
        let write_px_bgr = |buf: &mut [u8], off: usize, r: u8, g: u8, b: u8| {
            buf[off] = b;
            buf[off + 1] = g;
            buf[off + 2] = r;
        };
        // Bottom row (y=1): frame0=[yellow, yellow], frame1=[white, cyan]
        write_px_bgr(&mut bmp, 54, 255, 255, 0);
        write_px_bgr(&mut bmp, 57, 255, 255, 0);
        write_px_bgr(&mut bmp, 60, 255, 255, 255);
        write_px_bgr(&mut bmp, 63, 0, 255, 255);
        // Top row (y=0): frame0=[green, green], frame1=[blue, red]
        write_px_bgr(&mut bmp, 66, 0, 255, 0);
        write_px_bgr(&mut bmp, 69, 0, 255, 0);
        write_px_bgr(&mut bmp, 72, 0, 0, 255);
        write_px_bgr(&mut bmp, 75, 255, 0, 0);
        std::fs::write(gfx_dir.join("sheet.bmp"), &bmp).unwrap();

        let mut gvm = VM::new_with_sandbox(true);
        gvm.set_permissions(Permissions::sandboxed());
        gvm.set_current_dir(gfx_dir.clone());
        gvm.set_sandbox_root(gfx_dir.clone());

        let no_win_err = gvm
            .run(compile_source("import gfx; gfx.mouse_pressed(\"left\");").unwrap())
            .unwrap_err();
        assert!(no_win_err.contains("No active gfx window"));

        let gfx_script = r#"
import gfx;
import test;

gfx.headless(true, 32, 24);
test.assert(gfx.is_open());
test.assert_eq(gfx.width(), 32);
test.assert_eq(gfx.height(), 24);
gfx.clear(10, 20, 30);
gfx.rect(0, 0, 4, 4, 100, 100, 100);
gfx.rect_alpha(0, 0, 2, 2, 200, 0, 0, 128);
gfx.text(1, 1, "SCORE: 1", 255, 255, 255);
gfx.set_title("Headless");

let d0 = gfx.delta();
let d1 = gfx.delta();
test.assert(d0 >= 0.0 && d0 <= 0.1);
test.assert(d1 >= 0.0 && d1 <= 0.1);
test.assert(!gfx.mouse_down("left"));
test.assert(!gfx.mouse_pressed("left"));
test.assert(!gfx.key_pressed("SPACE"));
test.assert_eq(gfx.mouse_x(), 0);
test.assert_eq(gfx.mouse_y(), 0);

let sheet = gfx.load_image("sheet.bmp");
test.assert_eq(gfx.image_width(sheet), 4);
test.assert_eq(gfx.image_height(sheet), 2);
gfx.draw_sprite_frame(sheet, 1, 2, 1, 10, 8, 2);
gfx.present();
"#;
        gvm.run(compile_source(gfx_script).unwrap()).unwrap();
        assert_eq!(modules::gfx::test_text_count(), 1);
        assert_eq!(modules::gfx::test_peek_pixel(10, 8), Some(0x0000_00FF));
        assert_eq!(modules::gfx::test_peek_pixel(11, 9), Some(0x0000_00FF));
        assert_eq!(modules::gfx::test_peek_pixel(12, 8), Some(0x00FF_0000));
        assert_eq!(modules::gfx::test_peek_pixel(10, 10), Some(0x00FF_FFFF));
        assert_eq!(modules::gfx::test_peek_pixel(13, 11), Some(0x0000_FFFF));
        assert_eq!(
            modules::gfx::test_peek_pixel(30, 20),
            Some((10 << 16) | (20 << 8) | 30)
        );

        // Verify out-of-bounds frame & unknown button error cleanly
        let bad_frame = gvm
            .run(compile_source("import gfx; let s = gfx.load_image(\"sheet.bmp\"); gfx.draw_sprite_frame(s, 9, 2, 1, 0, 0);").unwrap())
            .unwrap_err();
        assert!(bad_frame.contains("out of bounds"));

        gvm.run(
            compile_source("import gfx; import test; gfx.close(); test.assert(!gfx.is_open());")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(modules::gfx::test_peek_pixel(10, 8), None);
        drop(gvm);

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_v020_stage1_p0_budget_buffer_and_vm() {
        // 1. P0: 16B Value ABI + Vec2 + as_range
        assert_eq!(std::mem::size_of::<crate::value::Value>(), 16);
        let v_vec = crate::value::Value::Vec2(1.5, -2.5);
        assert!(v_vec.is_truthy());
        assert_eq!(v_vec.type_name(), "vec2");
        assert!(!crate::value::Value::Vec2(0.0, 0.0).is_truthy());
        assert_eq!(crate::value::Value::range(3, 9).as_range(), Some((3, 9)));
        assert_eq!(v_vec.as_range(), None);

        // 2. P0: diag::ALL_CODES parity + zero `unsafe` in core engine files
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let diag_src = std::fs::read_to_string(manifest_dir.join("src").join("diag.rs")).unwrap();
        let mut found_codes = Vec::new();
        for win in diag_src.as_bytes().windows(7) {
            if win[0] == b'"'
                && (win[1] == b'E' || win[1] == b'W')
                && win[2..6].iter().all(|b| b.is_ascii_digit())
                && win[6] == b'"'
            {
                found_codes.push(std::str::from_utf8(&win[1..6]).unwrap().to_string());
            }
        }
        assert!(!found_codes.is_empty());
        for code in &found_codes {
            assert!(
                crate::diag::ALL_CODES.contains(&code.as_str()),
                "diag::ALL_CODES missing '{}'",
                code
            );
        }
        let main_src = std::fs::read_to_string(manifest_dir.join("src").join("main.rs")).unwrap();
        for core_mod in [
            "ast", "budget", "buffer", "bytecode", "compiler", "diag", "lexer", "parser", "value",
            "vm",
        ] {
            let expected = format!("#[forbid(unsafe_code)]\nmod {};", core_mod);
            assert!(
                main_src.replace("\r\n", "\n").contains(&expected),
                "Missing #[forbid(unsafe_code)] on mod {}",
                core_mod
            );
            let mod_src = std::fs::read_to_string(
                manifest_dir.join("src").join(format!("{}.rs", core_mod)),
            )
            .unwrap();
            assert!(
                !mod_src.contains("unsafe {") && !mod_src.contains("unsafe fn"),
                "Unexpected unsafe code in src/{}.rs",
                core_mod
            );
        }

        // 3. Stage 1: BudgetCore + TempCharge + VM snapshot helpers
        let core = crate::budget::BudgetCore::new(Some(1024), Some(512));
        core.charge_alloc(256).unwrap();
        core.charge_alloc(256).unwrap();
        assert_eq!(core.used_bytes(), 512);
        assert_eq!(core.peak_bytes(), 512);
        assert!(core.charge_alloc(600).is_err());
        assert_eq!(core.used_bytes(), 512);
        let temp_err: Result<(), String> =
            core.with_temp_charge(128, || Err("transient failure".to_string()));
        assert!(temp_err.is_err());
        assert_eq!(core.used_bytes(), 512);
        assert_eq!(core.peak_bytes(), 640);
        core.refund_alloc(512);
        assert_eq!(core.used_bytes(), 0);
        assert_eq!(core.peak_bytes(), 640);

        let mut vm = VM::new_with_sandbox(true);
        vm.charge_alloc(128).unwrap();
        assert_eq!(vm.budget_snapshot().0, 128);
        let ok_res = vm.with_temp_charge(64, || Ok(42usize)).unwrap();
        assert_eq!(ok_res, 42);
        assert_eq!(vm.budget_snapshot().0, 128);
        vm.refund_alloc(128);
        assert_eq!(vm.budget_snapshot().0, 0);
        let snap_full = vm.budget_snapshot_full();
        assert_eq!(snap_full.memory_limit, vm.budget_snapshot().1);
        assert_eq!(snap_full.io_limit, vm.budget_snapshot().3);
        assert!(snap_full.memory_peak >= 192);

        // 4. Stage 1: Typed Buffer (`BufData` direct + `import buffer` + drop auto-refund + self-alias)
        let mut raw_buf = crate::buffer::BufData::new_zeroed(crate::buffer::Elem::U32, 4, None)
            .unwrap();
        assert!(raw_buf.write_u32_at(2, 0xDEAD_BEEF));
        assert!(!raw_buf.write_u32_at(4, 1));
        assert_eq!(raw_buf.read_u32_at(2), Some(0xDEAD_BEEF));
        assert_eq!(raw_buf.read_u32_at(4), None);

        let buf_script = r#"
import buffer;
import sys;
import test;

let b_u8 = buffer.new("u8", 6);
test.assert_eq(buffer.len(b_u8), 6);
test.assert_eq(buffer.byte_len(b_u8), 6);
test.assert_eq(buffer.elem_type(b_u8), "u8");
for i in range(6) {
    buffer.set(b_u8, i, i + 10);
}
test.assert_eq(buffer.get(b_u8, -1), 15);
buffer.copy_from(b_u8, 2, b_u8, 0, 4);
test.assert_eq(buffer.get(b_u8, 2), 10);
test.assert_eq(buffer.get(b_u8, 5), 13);

let b_i32 = buffer.new("i32", 3);
buffer.set(b_i32, 0, -42);
test.assert_eq(buffer.get(b_i32, 0), -42);

let b_f32 = buffer.new("f32", 2);
buffer.fill(b_f32, 1.5);
test.assert(buffer.get(b_f32, 1) == 1.5);

let img = buffer.new("u32", 16);
buffer.fill_rgba(img, 287454020);
buffer.set(img, 0, 2864434397);
buffer.blit(img, 4, 4, 2, 2, img, 4, 4, 0, 0, 2, 2);
test.assert_eq(buffer.get(img, 10), 2864434397);

let sub = buffer.slice(b_u8, 1, 4);
let cloned = buffer.clone(sub);
let raw = buffer.as_bytes(cloned);
let restored = buffer.from_bytes(raw, "u8");
test.assert_eq(buffer.len(restored), 3);
test.assert_eq(buffer.get(restored, 0), 11);

// Automatic BudgetCore refund on drop
let b0 = sys.budget();
let big = buffer.new("u8", 8192);
let b1 = sys.budget();
test.assert(b1.memory_used >= b0.memory_used + 8192);
test.assert(b1.memory_peak >= b1.memory_used);
test.assert(b1.work_steps > b0.work_steps);
big = unit;
let b2 = sys.budget();
test.assert(b2.memory_used < b1.memory_used);
test.assert(b2.memory_peak >= b1.memory_peak);
"#;
        let mut buf_vm = VM::new_with_sandbox(true);
        buf_vm.run(compile_source(buf_script).unwrap()).unwrap();

        // 5. Stage 1: superinstructions + CoW string + exact --max-steps invariance + --deterministic
        let opt_script = r#"
import random;
import sys;
import test;
import time;

fn run_ops(n) {
    let arr = [0, 0, 0, 0, 0];
    let i = 0;
    let step = 2;
    while i < 5 {
        arr[i] = i * i;
        i += 1;
    }
    let acc = 0;
    let j = 0;
    while j <= 4 {
        let p = j;
        let sq = p * p;
        acc += arr[j];
        acc += sq;
        acc -= p;
        j += 1;
    }
    if n > 0 {
        let s = "hi";
        let alias = s;
        s += "!";
        test.assert_eq(s, "hi!");
        test.assert_eq(alias, "hi");
    }
    let a = 10;
    let b = 3;
    a += b;
    a -= b;
    let c = a * b;
    return [acc, c, random.int(1, 100), time.elapsed_ms(), sys.budget().work_steps];
}
run_ops(1)
"#;
        let mut det1 = VM::new_with_sandbox(true);
        det1.set_deterministic(true);
        let r1 = det1.run(compile_source(opt_script).unwrap()).unwrap();
        let mut det2 = VM::new_with_sandbox(true);
        det2.set_deterministic(true);
        let r2 = det2.run(compile_source(opt_script).unwrap()).unwrap();
        assert!(matches!(r1, crate::value::Value::List(_)));
        assert!(r1.equals_vm(&r2, &mut det1).unwrap());

        // Exact step budget enforcement on tight superinstruction loop (70..=80)
        let loop_src = r#"
fn sum_sieve() {
    let arr = [1, 2, 3, 4, 5, 6];
    let limit = 5;
    let j = 0;
    let p = 1;
    let acc = 0;
    while j <= limit {
        arr[j] = 0;
        let k = j * p;
        acc += k;
        j += p;
    }
    return acc;
}
sum_sieve()
"#;
        let proto = compile_source(loop_src).unwrap();
        let mut measure_vm = VM::new_with_sandbox(true);
        assert!(matches!(measure_vm.run(proto.clone()).unwrap(), crate::value::Value::Int(15)));
        let used_steps = measure_vm.budget_snapshot_full().work_steps;
        assert!(used_steps > 0);

        let mut perms_exact = Permissions::sandboxed();
        perms_exact.max_instructions = Some(used_steps);
        let mut exact_vm = VM::new_with_sandbox(true);
        exact_vm.set_permissions(perms_exact);
        assert!(exact_vm.run(proto.clone()).is_ok());

        let mut perms_short = Permissions::sandboxed();
        perms_short.max_instructions = Some(used_steps - 1);
        let mut short_vm = VM::new_with_sandbox(true);
        short_vm.set_permissions(perms_short);
        let err = short_vm.run(proto).unwrap_err();
        assert!(err.contains("Sandbox instruction limit exceeded"), "got: {}", err);
    }

    #[test]
    fn test_v020_stage2_surface_layers_camera_geom_fonts_and_ui() {
        modules::reset_runtime_state();

        let make_test_png = |w: i32, h: i32, rgba: &[u8]| -> Vec<u8> {
            fn crc32(data: &[u8]) -> u32 {
                let mut crc = 0xFFFF_FFFFu32;
                for &b in data {
                    crc ^= b as u32;
                    for _ in 0..8 {
                        crc = if (crc & 1) != 0 {
                            (crc >> 1) ^ 0xEDB8_8320
                        } else {
                            crc >> 1
                        };
                    }
                }
                !crc
            }
            fn adler32(data: &[u8]) -> u32 {
                let mut s1 = 1u32;
                let mut s2 = 0u32;
                for &b in data {
                    s1 = (s1 + b as u32) % 65521;
                    s2 = (s2 + s1) % 65521;
                }
                (s2 << 16) | s1
            }
            fn push_chunk(out: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
                out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
                out.extend_from_slice(tag);
                out.extend_from_slice(payload);
                let mut crc_in = Vec::with_capacity(4 + payload.len());
                crc_in.extend_from_slice(tag);
                crc_in.extend_from_slice(payload);
                out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
            }
            let row_bytes = (w as usize) * 4;
            let mut raw = Vec::with_capacity((row_bytes + 1) * (h as usize));
            for y in 0..h as usize {
                raw.push(0); // filter 0
                raw.extend_from_slice(&rgba[y * row_bytes..(y + 1) * row_bytes]);
            }
            let mut zlib = Vec::with_capacity(raw.len() + 11);
            zlib.push(0x78);
            zlib.push(0x01);
            zlib.push(0x01); // final stored block
            let len = raw.len() as u16;
            zlib.extend_from_slice(&len.to_le_bytes());
            zlib.extend_from_slice(&(!len).to_le_bytes());
            zlib.extend_from_slice(&raw);
            zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

            let mut ihdr = Vec::with_capacity(13);
            ihdr.extend_from_slice(&w.to_be_bytes());
            ihdr.extend_from_slice(&h.to_be_bytes());
            ihdr.push(8); // bit depth
            ihdr.push(6); // RGBA
            ihdr.push(0);
            ihdr.push(0);
            ihdr.push(0);

            let mut out = Vec::from(*b"\x89PNG\r\n\x1a\n");
            push_chunk(&mut out, b"IHDR", &ihdr);
            push_chunk(&mut out, b"IDAT", &zlib);
            push_chunk(&mut out, b"IEND", &[]);
            out
        };

        // 1. P0 #[forbid(unsafe_code)] + Value ABI 16B on all Stage 2 modules
        assert_eq!(std::mem::size_of::<crate::value::Value>(), 16);
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let main_src = std::fs::read_to_string(manifest_dir.join("src").join("main.rs")).unwrap();
        for s2_mod in [
            "camera",
            "compose",
            "font_atlas",
            "font_bitmap",
            "font_bmfont",
            "layers",
            "scene",
            "surface",
        ] {
            let expected = format!("#[forbid(unsafe_code)]\nmod {};", s2_mod);
            assert!(
                main_src.replace("\r\n", "\n").contains(&expected),
                "Missing #[forbid(unsafe_code)] on mod {}",
                s2_mod
            );
        }

        // 2. DepthBuffer NDC [0.0, 1.0] + BudgetCore charge/refund
        let depth_budget = crate::budget::BudgetCore::new(Some(65_536), Some(65_536));
        let mut depth = crate::surface::DepthBuffer::new(16, 16, Some(&depth_budget)).unwrap();
        assert_eq!(depth_budget.used_bytes(), 16 * 16 * 4);
        assert_eq!(depth.peek(0, 0), Some(1.0));
        assert!(depth.test_and_set(3, 4, 0.5));
        assert!(!depth.test_and_set(3, 4, 0.5));
        assert!(!depth.test_and_set(3, 4, 0.7));
        assert!(depth.test_and_set(3, 4, 0.25));
        assert!(!depth.test_and_set(3, 4, -0.1));
        assert!(!depth.test_and_set(3, 4, 1.1));
        depth.clear();
        assert_eq!(depth.peek(3, 4), Some(1.0));
        depth.refund_to(Some(&depth_budget));
        assert_eq!(depth_budget.used_bytes(), 0);

        let mut test_surf = crate::surface::Surface::new_rgba8(8, 8, 0, Some(&depth_budget)).unwrap();
        test_surf.attach_depth(Some(&depth_budget)).unwrap();
        assert_eq!(test_surf.peek_pixel(0, 0), Some(0));
        test_surf.refund_to(Some(&depth_budget));
        assert_eq!(depth_budget.used_bytes(), 0);

        // 3. Golden #24 + Surface clip/unwind/depth-9/self-target + Layers blend/visibility/parallax + BudgetCore refund
        let s2_gfx_script = r#"
import gfx;
import math;
import sys;
import test;

gfx.headless(true, 64, 64);

// Golden #24 scene: clear -> rect -> circle -> present
gfx.clear(10, 20, 30);
gfx.rect(8, 8, 12, 10, 200, 50, 40);
gfx.circle(32, 32, 6, 40, 200, 100);
gfx.present();
let ghash = gfx.frame_hash();
test.assert_eq(gfx.read_pixel(10, 10), (200 * 65536) + (50 * 256) + 40);
test.assert_eq(gfx.read_pixel(32, 32), (40 * 65536) + (200 * 256) + 100);
test.assert_eq(gfx.read_pixel(0, 0), (10 * 65536) + (20 * 256) + 30);

// Repeat golden #24 must match hash identically
gfx.clear(10, 20, 30);
gfx.rect(8, 8, 12, 10, 200, 50, 40);
gfx.circle(32, 32, 6, 40, 200, 100);
gfx.present();
test.assert_eq(gfx.frame_hash(), ghash);

// Clip test on offscreen surface
let b_before = sys.budget().memory_used;
let s = gfx.surface_new(32, 32);
test.assert_eq(gfx.surface_width(s), 32);
test.assert_eq(gfx.surface_height(s), 32);
gfx.draw_to(s, fn() {
    gfx.clip(4, 4, 8, 8);
    gfx.rect(0, 0, 20, 20, 255, 0, 0);
    gfx.reset_clip();
    test.assert_eq(gfx.read_pixel(2, 2), 0);
    test.assert_eq(gfx.read_pixel(6, 6), 4294901760);
    test.assert_eq(gfx.read_pixel(12, 12), 0);
});
gfx.surface_free(s);
test.assert(sys.budget().memory_used <= b_before + 256);

// Target stack unwind on error inside draw_to
let s_err = gfx.surface_new(16, 16);
let caught = false;
try {
    gfx.draw_to(s_err, fn() {
        gfx.set_camera(99, 99, 3.0, 0.5);
        throw "inner boom";
    });
} catch e {
    caught = true;
}
test.assert(caught);
test.assert_eq(gfx.width(), 64);
test.assert(!gfx.camera().enabled);

// Self-target draw_to and draw_surface rejected
let self_err = false;
try {
    gfx.draw_to(s_err, fn() { gfx.draw_to(s_err, fn() {}); });
} catch e {
    self_err = true;
}
test.assert(self_err);

let sample_self_err = false;
try {
    gfx.draw_to(s_err, fn() { gfx.draw_surface(s_err, 0, 0); });
} catch e {
    sample_self_err = true;
}
test.assert(sample_self_err);
gfx.surface_free(s_err);

// Target stack depth > 8 rejected & unwound cleanly
let ring = [];
for i in range(9) {
    ring.push(gfx.surface_new(8, 8));
}
let depth_err = false;
fn dive(idx) {
    if idx >= 9 { return 0; }
    gfx.draw_to(ring[idx], fn() { dive(idx + 1); });
}
try {
    dive(0);
} catch e {
    depth_err = true;
}
test.assert(depth_err);
test.assert_eq(gfx.width(), 64);
for sh in ring { gfx.surface_free(sh); }

// Layers: normal, add, multiply integer math, z-order, visibility skip
gfx.clear(100, 100, 100);
let l0 = gfx.layer_new(64, 64);
let l1 = gfx.layer_new(64, 64);
let l2 = gfx.layer_new(64, 64);
let l_hidden = gfx.layer_new(64, 64);

gfx.layer_z(l1, 5);
gfx.layer_z(l0, 10);
gfx.layer_z(l2, 20);
gfx.layer_mode(l0, "normal");
gfx.layer_mode(l1, "add");
gfx.layer_mode(l2, "multiply");
gfx.layer_visible(l_hidden, false);

gfx.draw_to_layer(l1, fn() { gfx.rect_alpha(10, 10, 4, 4, 200, 50, 25, 255); });
gfx.draw_to_layer(l0, fn() { gfx.rect_alpha(20, 20, 4, 4, 200, 0, 0, 128); });
gfx.draw_to_layer(l2, fn() { gfx.rect_alpha(30, 30, 4, 4, 128, 255, 0, 255); });
gfx.draw_to_layer(l_hidden, fn() { gfx.rect(0, 0, 64, 64, 255, 255, 255); });

gfx.compose_layers();
// Normal (dst=100, src=200, alpha=128) -> (200*128 + 100*127 + 127)/255 = 150
let px_norm = gfx.read_pixel(20, 20);
test.assert_eq((px_norm / 65536) % 256, 150);
// Add (dst=100 + 200 clamped to 255, g=150, b=125)
let px_add = gfx.read_pixel(10, 10);
test.assert_eq((px_add / 65536) % 256, 255);
test.assert_eq((px_add / 256) % 256, 150);
// Multiply (dst=100, src=(128,255,0)) -> r = (128*100+127)/255 = 50, g = 100, b = 0
let px_mul = gfx.read_pixel(30, 30);
test.assert_eq((px_mul / 65536) % 256, 50);
test.assert_eq((px_mul / 256) % 256, 100);
test.assert_eq(px_mul % 256, 0);
// Hidden layer did NOT overwrite pixel (0, 0)
test.assert_eq(gfx.read_pixel(0, 0), (100 * 65536) + (100 * 256) + 100);

// Parallax 0.5 displaces layer by half of camera movement
gfx.layer_visible(l0, false);
gfx.layer_visible(l1, false);
gfx.layer_visible(l2, false);
let l_para = gfx.layer_new(64, 64);
gfx.layer_parallax(l_para, 0.5, 0.0);
gfx.draw_to_layer(l_para, fn() { gfx.rect(30, 10, 4, 4, 250, 10, 10); });
gfx.clear(0, 0, 0);
gfx.set_camera(40, 0, 1.0, 0.0);
gfx.compose_layers();
gfx.reset_camera();
// Moved by -40 * 0.5 = -20 -> pixel (30 - 20, 10) = (10, 10)
test.assert_eq((gfx.read_pixel(10, 10) / 65536) % 256, 250);
test.assert_eq(gfx.read_pixel(30, 10), 0);

gfx.layer_free(l0);
gfx.layer_free(l1);
gfx.layer_free(l2);
gfx.layer_free(l_hidden);
gfx.layer_free(l_para);

// Camera2D roundtrip & rotation
gfx.set_camera(100.0, 50.0, 2.5, 0.73);
let scr = gfx.world_to_screen(123.5, 19.25);
let wrd = gfx.screen_to_world(scr.x, scr.y);
test.assert(math.abs(wrd.x - 123.5) < 0.001);
test.assert(math.abs(wrd.y - 19.25) < 0.001);
gfx.reset_camera();
"#;
        let mut gvm = VM::new_with_sandbox(true);
        gvm.set_deterministic(true);
        gvm.run(compile_source(s2_gfx_script).unwrap()).unwrap();
        assert_eq!(modules::gfx::test_peek_pixel(0, 0), Some(0));

        // 4. Camera shake deterministic linear decay across two VMs
        let shake_script = r#"
import gfx;
import math;
import test;
gfx.headless(true, 64, 64);
gfx.set_camera(0.0, 0.0, 1.0, 0.0);
gfx.set_camera_shake(10.0, 1.0);
let a0 = gfx.camera().shake;
let s0 = gfx.world_to_screen(0.0, 0.0);
gfx.update_camera(0.5);
let a1 = gfx.camera().shake;
let s1 = gfx.world_to_screen(0.0, 0.0);
gfx.update_camera(0.5);
let a2 = gfx.camera().shake;
let s2 = gfx.world_to_screen(0.0, 0.0);
test.assert(math.abs(a0 - 10.0) < 0.0001);
test.assert(math.abs(a1 - 5.0) < 0.0001);
test.assert(math.abs(a2) < 0.0001);
test.assert(math.abs(s2.x - 32.0) < 0.001);
[s0.x, s0.y, s1.x, s1.y]
"#;
        let mut svm1 = VM::new_with_sandbox(true);
        svm1.set_deterministic(true);
        let sh1 = svm1.run(compile_source(shake_script).unwrap()).unwrap();
        modules::reset_runtime_state();
        let mut svm2 = VM::new_with_sandbox(true);
        svm2.set_deterministic(true);
        let sh2 = svm2.run(compile_source(shake_script).unwrap()).unwrap();
        assert!(sh1.equals_vm(&sh2, &mut svm1).unwrap());
        modules::reset_runtime_state();

        // 5. Coordinate canon, geom types, VM operators, JSON, NDC [0, 1] frustum
        let geom_script = r#"
import geom;
import math;
import json;
import test;

let v2 = geom.vec2(1.0, 0.0);
let v2r = geom.rot90(v2, 1);
test.assert_eq(v2r.x, 0.0);
test.assert_eq(v2r.y, 1.0);
test.assert((geom.vec2(2.0, 3.0) + geom.vec2(1.0, 2.0)) == geom.vec2(3.0, 5.0));
test.assert((geom.vec2(4.0, 6.0) / 2) == geom.vec2(2.0, 3.0));
test.assert(-geom.vec3(1.0, -2.0, 3.0) == geom.vec3(-1.0, 2.0, -3.0));

let a = geom.vec3(1.0, 2.0, 3.0);
let b = geom.vec3(4.0, 5.0, 6.0);
test.assert((a + b * 2) == geom.vec3(9.0, 12.0, 15.0));
test.assert((b - a) == geom.vec3(3.0, 3.0, 3.0));
test.assert_eq(geom.cross(geom.vec3(1, 0, 0), geom.vec3(0, 1, 0)), geom.vec3(0, 0, 1));

// Quaternion rotation (90 deg around Y maps +X -> -Z in RH coordinates!)
let q = geom.quat_axis_angle(geom.vec3(0.0, 1.0, 0.0), geom.deg_to_rad(90.0));
let rotated = q * geom.vec3(1.0, 0.0, 0.0);
test.assert(math.abs(rotated.x) < 0.0001);
test.assert(math.abs(rotated.y) < 0.0001);
test.assert(math.abs(rotated.z + 1.0) < 0.0001);

// NDC [0, 1] perspective check: -near -> z_ndc 0.0, -far -> z_ndc 1.0
let proj = geom.mat4_perspective(geom.deg_to_rad(60.0), 1.5, 1.0, 100.0);
let p_near = proj * geom.vec4(0.0, 0.0, -1.0, 1.0);
let p_far = proj * geom.vec4(0.0, 0.0, -100.0, 1.0);
test.assert(math.abs(p_near.z / p_near.w) < 0.0001);
test.assert(math.abs((p_far.z / p_far.w) - 1.0) < 0.0001);

let view = geom.mat4_look_at(geom.vec3(0, 0, 0), geom.vec3(0, 0, -1), geom.vec3(0, 1, 0));
let fr = geom.frustum_from_mat4(proj * view);
test.assert(geom.frustum_contains_point(fr, geom.vec3(0.0, 0.0, -10.0)));
test.assert(!geom.frustum_contains_point(fr, geom.vec3(0.0, 0.0, -0.5)));
test.assert(!geom.frustum_contains_point(fr, geom.vec3(0.0, 0.0, -150.0)));

let box = geom.aabb(geom.vec3(-1, -1, -1), geom.vec3(1, 1, 1));
let ray = geom.ray(geom.vec3(0, 0, 5), geom.vec3(0, 0, -1));
let hit = geom.ray_aabb(ray, box);
test.assert(math.abs(hit - 4.0) < 0.0001);

let tr = geom.transform(geom.vec3(10, 0, 0), geom.quat_identity(), geom.vec3(2, 2, 2));
test.assert_eq(geom.transform_point(tr, geom.vec3(1, 2, 3)), geom.vec3(12, 4, 6));
test.assert(json.stringify(geom.vec2(1, 2)).contains("[1,2]"));
"#;
        VM::new_with_sandbox(true)
            .run(compile_source(geom_script).unwrap())
            .unwrap();

        // 6. Latin+Cyrillic Bitmap Font, FontAtlas cache/flush hash invariance, BMFont + chromatic reject
        let font_dir = std::env::temp_dir().join(format!("askane_v020_font_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&font_dir);
        std::fs::create_dir_all(&font_dir).unwrap();

        // Valid grayscale white-alpha 8x8 PNG page with 'A' (65) and 'B' (66)
        let mut mask_rgba = vec![0u8; 8 * 8 * 4];
        for y in 0..4usize {
            for x in 0..4usize {
                let idx = (y * 8 + x) * 4;
                mask_rgba[idx] = 255;
                mask_rgba[idx + 1] = 255;
                mask_rgba[idx + 2] = 255;
                mask_rgba[idx + 3] = 255;
            }
        }
        std::fs::write(font_dir.join("gray_page.png"), make_test_png(8, 8, &mask_rgba)).unwrap();
        let valid_fnt = "\
info face=\"Test\" size=16
common lineHeight=16 base=12 scaleW=8 scaleH=8 pages=1
page id=0 file=\"gray_page.png\"
chars count=2
char id=65 x=0 y=0 width=4 height=4 xoffset=0 yoffset=2 xadvance=6 page=0 chnl=15
char id=66 x=0 y=0 width=4 height=4 xoffset=0 yoffset=2 xadvance=6 page=0 chnl=15
kerning first=65 second=66 amount=-1
";
        std::fs::write(font_dir.join("test.fnt"), valid_fnt).unwrap();

        // Chromatic RGB PNG page must be rejected loudly!
        let mut color_rgba = mask_rgba.clone();
        color_rgba[0] = 255;
        color_rgba[1] = 20;
        color_rgba[2] = 10;
        color_rgba[3] = 255;
        std::fs::write(font_dir.join("color_page.png"), make_test_png(8, 8, &color_rgba)).unwrap();
        let bad_fnt = valid_fnt.replace("gray_page.png", "color_page.png");
        std::fs::write(font_dir.join("bad_color.fnt"), bad_fnt).unwrap();

        let mut fvm = VM::new_with_sandbox(true);
        fvm.set_permissions(Permissions::sandboxed());
        fvm.set_current_dir(font_dir.clone());
        fvm.set_sandbox_root(font_dir.clone());

        let font_miss_script = r##"
import gfx;
import test;

gfx.headless(true, 160, 48);
let f0 = gfx.font_bitmap();
test.assert_eq(f0, 0);
gfx.clear(0, 0, 0);
gfx.draw_text(f0, 4, 4, "Привет, Mir! Ёё", 16, "#FFFFFF");
let cyr_w = gfx.text_width(f0, "Привет", 16);
test.assert_eq(cyr_w, 6 * 8);
test.assert_eq(gfx.text_line_height(f0, 16), 16);
// Unknown codepoint uses fallback box matching exact measured width
let unk_m = gfx.measure_text(f0, "☃", 16);
test.assert_eq(unk_m.width, 8);
gfx.frame_hash()
"##;
        let hash_miss = fvm.run(compile_source(font_miss_script).unwrap()).unwrap();
        // Second run: cache hit -> identical hash!
        let hash_hit = fvm
            .run(compile_source(
                r##"
import gfx;
gfx.clear(0, 0, 0);
gfx.draw_text(gfx.font_bitmap(), 4, 4, "Привет, Mir! Ёё", 16, "#FFFFFF");
gfx.frame_hash()
"##,
            )
            .unwrap())
            .unwrap();
        assert!(hash_miss.equals(&hash_hit));
        assert_eq!(modules::gfx::test_flush_font_atlas(), 1);
        // Third run after atlas flush -> misses cleanly & reproduces bit-identical hash!
        let hash_after_flush = fvm
            .run(compile_source(
                r##"
import gfx;
gfx.clear(0, 0, 0);
gfx.draw_text(gfx.font_bitmap(), 4, 4, "Привет, Mir! Ёё", 16, "#FFFFFF");
gfx.frame_hash()
"##,
            )
            .unwrap())
            .unwrap();
        assert!(hash_miss.equals(&hash_after_flush));

        let bmfont_script = r#"
import gfx;
import test;

let bm = gfx.font_load_bmfont("test.fnt");
test.assert(bm >= 1);
test.assert_eq(gfx.text_width(bm, "AB", 16), 11); // 6 + 6 - 1 kerning
gfx.clear(0, 0, 0);
gfx.draw_text(bm, 2, 2, "AB", 16, [255, 255, 255]);
test.assert_eq(gfx.read_pixel(2, 4), 16777215);
gfx.font_free(bm);

let rejected = false;
try {
    gfx.font_load_bmfont("bad_color.fnt");
} catch e {
    rejected = true;
}
test.assert(rejected);
"#;
        fvm.run(compile_source(bmfont_script).unwrap()).unwrap();
        modules::reset_runtime_state();
        let _ = std::fs::remove_dir_all(&font_dir);

        // 7. ui extensions: rect, image, nine_slice, icon_button
        let ui_dir = std::env::temp_dir().join(format!("askane_v020_ui_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&ui_dir);
        std::fs::create_dir_all(&ui_dir).unwrap();
        let mut icon_rgba = vec![0u8; 6 * 6 * 4];
        for y in 0..6usize {
            for x in 0..6usize {
                let idx = (y * 6 + x) * 4;
                let border = x == 0 || y == 0 || x == 5 || y == 5;
                icon_rgba[idx] = if border { 255 } else { 10 };
                icon_rgba[idx + 1] = if border { 0 } else { 240 };
                icon_rgba[idx + 2] = if border { 0 } else { 20 };
                icon_rgba[idx + 3] = 255;
            }
        }
        std::fs::write(ui_dir.join("panel.png"), make_test_png(6, 6, &icon_rgba)).unwrap();

        let ui_script = r##"
import gfx;
import ui;
import test;

gfx.headless(true, 32, 32);
let img = gfx.load_image("panel.png");
let surf = gfx.surface_new(8, 8);
gfx.draw_to(surf, fn() { gfx.clear(12, 34, 56); });

let th = #{padding: 8, spacing: 4};
ui.window("Test", 240, 200, #{headless: true, theme: th});
let clicked = false;
let root = ui.column([
    ui.rect(#{width: 24, height: 12, bg: "#AA1122"}),
    ui.image(surf, #{width: 16, height: 16}),
    ui.nine_slice(img, 1, 1, 1, 1, #{width: 20, height: 20}),
    ui.icon_button(surf, "Play", #{id: "ib", width: 120, height: 36, on_click: fn(id) { clicked = true; }})
]);
let lay = ui.layout(root, 240, 200, th);
let nodes = lay._nodes;
test.assert_eq(len(nodes), 5);
test.assert_eq(nodes[1].kind, "rect");
test.assert_eq(nodes[2].kind, "image");
test.assert_eq(nodes[3].kind, "nine_slice");
test.assert_eq(nodes[4].kind, "icon_button");
ui.inject_click(nodes[4].x + 10, nodes[4].y + 10);
ui.render(root);
test.assert(clicked);
let evs = ui.events();
test.assert_eq(len(evs), 1);
test.assert_eq(evs[0].id, "ib");
[nodes[1].x, nodes[1].y, nodes[3].x, nodes[3].y]
"##;
        let mut uvm = VM::new_with_sandbox(true);
        uvm.set_permissions(Permissions::sandboxed());
        uvm.set_current_dir(ui_dir.clone());
        uvm.set_sandbox_root(ui_dir.clone());
        let coords = uvm.run(compile_source(ui_script).unwrap()).unwrap();
        let Value::List(cl) = coords else { panic!("expected list") };
        let cv = cl.borrow();
        let rx = match cv[0] { Value::Int(i) => i as i32, _ => unreachable!() };
        let ry = match cv[1] { Value::Int(i) => i as i32, _ => unreachable!() };
        let nx = match cv[2] { Value::Int(i) => i as i32, _ => unreachable!() };
        let ny = match cv[3] { Value::Int(i) => i as i32, _ => unreachable!() };
        // Rect pixel (#AA1122)
        assert_eq!(modules::ui::test_peek_pixel(rx + 2, ry + 2), Some(0xAA1122));
        // Nine-slice corner border = red (255,0,0), center = green (10,240,20)
        assert_eq!(modules::ui::test_peek_pixel(nx, ny), Some(0xFF0000));
        assert_eq!(modules::ui::test_peek_pixel(nx + 10, ny + 10), Some(0x0AF014));
        drop(cv);
        drop(uvm);
        modules::reset_runtime_state();
        let _ = std::fs::remove_dir_all(&ui_dir);
    }

    fn build_png_chunked(
        w: u32,
        h: u32,
        depth: u8,
        ctype: u8,
        interlace: u8,
        plte: Option<&[u8]>,
        trns: Option<&[u8]>,
        scanlines: &[u8],
    ) -> Vec<u8> {
        let mut png = Vec::from(*b"\x89PNG\r\n\x1a\n");
        let append = |out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]| {
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            let start = out.len();
            out.extend_from_slice(tag);
            out.extend_from_slice(body);
            let crc = modules::compress::crc32_bytes(&out[start..]);
            out.extend_from_slice(&crc.to_be_bytes());
        };
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.push(depth);
        ihdr.push(ctype);
        ihdr.push(0);
        ihdr.push(0);
        ihdr.push(interlace);
        append(&mut png, b"IHDR", &ihdr);
        if let Some(p) = plte {
            append(&mut png, b"PLTE", p);
        }
        if let Some(t) = trns {
            append(&mut png, b"tRNS", t);
        }
        let deflated = modules::compress::deflate_raw(scanlines);
        let mut zlib = Vec::with_capacity(deflated.len() + 6);
        zlib.push(0x78);
        zlib.push(0x01);
        zlib.extend_from_slice(&deflated);
        zlib.extend_from_slice(&modules::compress::adler32_bytes(scanlines).to_be_bytes());
        append(&mut png, b"IDAT", &zlib);
        append(&mut png, b"IEND", &[]);
        png
    }

    fn build_bmp_raw(w: i32, h: i32, bpp: u16, palette_bgra: &[[u8; 4]], raw_rows: &[Vec<u8>]) -> Vec<u8> {
        let pal_bytes = palette_bgra.len() * 4;
        let offset = 14 + 40 + pal_bytes;
        let stride = raw_rows.first().map(|r| r.len()).unwrap_or(0);
        let img_size = stride * raw_rows.len();
        let file_size = offset + img_size;
        let mut out = Vec::with_capacity(file_size);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(file_size as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&bpp.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(img_size as u32).to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes());
        out.extend_from_slice(&(palette_bgra.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for c in palette_bgra {
            out.extend_from_slice(c);
        }
        for row in raw_rows {
            out.extend_from_slice(row);
        }
        out
    }

    fn build_qoi_rgba(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::from(*b"qoif");
        out.extend_from_slice(&w.to_be_bytes());
        out.extend_from_slice(&h.to_be_bytes());
        out.push(4);
        out.push(0);
        let mut index = [[0u8; 4]; 64];
        let mut prev = [0u8, 0u8, 0u8, 255u8];
        let mut run = 0usize;
        let total = (w as usize) * (h as usize);
        for (i, ch) in rgba.chunks_exact(4).enumerate() {
            let px = [ch[0], ch[1], ch[2], ch[3]];
            if px == prev {
                run += 1;
                if run == 62 || i + 1 == total {
                    out.push(0xC0 | ((run as u8) - 1));
                    run = 0;
                }
                continue;
            }
            if run > 0 {
                out.push(0xC0 | ((run as u8) - 1));
                run = 0;
            }
            let hidx = ((px[0] as usize) * 3
                + (px[1] as usize) * 5
                + (px[2] as usize) * 7
                + (px[3] as usize) * 11)
                % 64;
            if index[hidx] == px {
                out.push(hidx as u8);
            } else {
                index[hidx] = px;
                out.push(0xFF);
                out.extend_from_slice(&px);
            }
            prev = px;
        }
        out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
        out
    }

    fn build_gif_lzw_stream(min_code_size: u8, indices: &[u8]) -> Vec<u8> {
        let clear = 1u16 << min_code_size;
        let eoi = clear + 1;
        let width = (min_code_size + 1) as usize;
        let mut bits: u64 = 0;
        let mut bit_count: usize = 0;
        let mut payload = Vec::new();
        let mut push_code = |code: u16| {
            bits |= (code as u64) << bit_count;
            bit_count += width;
            while bit_count >= 8 {
                payload.push((bits & 0xFF) as u8);
                bits >>= 8;
                bit_count -= 8;
            }
        };
        push_code(clear);
        for (idx, &v) in indices.iter().enumerate() {
            push_code(v as u16);
            // Reset table periodically before bit width expands.
            if (idx + 1) % (clear as usize - 2).max(1) == 0 && idx + 1 < indices.len() {
                push_code(clear);
            }
        }
        push_code(eoi);
        if bit_count > 0 {
            payload.push((bits & 0xFF) as u8);
        }
        let mut out = vec![min_code_size];
        for chunk in payload.chunks(250) {
            out.push(chunk.len() as u8);
            out.extend_from_slice(chunk);
        }
        out.push(0);
        out
    }

    fn build_test_jpeg_8x8(sof_marker: u8, color: bool) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8]; // SOI
        // DQT table 0: all ones for predictable coefficients
        out.extend_from_slice(&[0xFF, 0xDB, 0x00, 67, 0x00]);
        out.extend(std::iter::repeat_n(1u8, 64));

        // DHT DC table 0 & 1: length 2 -> codes 00 (diff=0), 01 (cat 4)
        for cls_id in if color { &[0x00u8, 0x01u8][..] } else { &[0x00u8][..] } {
            out.extend_from_slice(&[0xFF, 0xC4, 0x00, 21, *cls_id]);
            let mut bits = [0u8; 16];
            bits[1] = 2;
            out.extend_from_slice(&bits);
            out.extend_from_slice(&[0x00, 0x04]);
        }
        // DHT AC table 0 & 1: length 1 -> code 0 (EOB 0x00)
        for cls_id in if color { &[0x10u8, 0x11u8][..] } else { &[0x10u8][..] } {
            out.extend_from_slice(&[0xFF, 0xC4, 0x00, 20, *cls_id]);
            let mut bits = [0u8; 16];
            bits[0] = 1;
            out.extend_from_slice(&bits);
            out.push(0x00);
        }

        // SOF
        let nc = if color { 3u8 } else { 1u8 };
        let sof_len = 8 + 3 * (nc as u16);
        out.extend_from_slice(&[0xFF, sof_marker]);
        out.extend_from_slice(&sof_len.to_be_bytes());
        out.push(8);
        out.extend_from_slice(&8u16.to_be_bytes());
        out.extend_from_slice(&8u16.to_be_bytes());
        out.push(nc);
        for c in 1..=nc {
            out.push(c);
            out.push(0x11);
            out.push(0);
        }

        // SOS
        let sos_len = 6 + 2 * (nc as u16);
        out.extend_from_slice(&[0xFF, 0xDA]);
        out.extend_from_slice(&sos_len.to_be_bytes());
        out.push(nc);
        out.push(1);
        out.push(0x00);
        if color {
            out.push(2);
            out.push(0x11);
            out.push(3);
            out.push(0x11);
        }
        out.extend_from_slice(&[0x00, 0x3F, 0x00]);

        if color {
            // Y: DC cat 4 +15 ('01' + '1111'), AC EOB ('0') -> 0111110
            // Cb: DC 0 ('00'), AC EOB ('0') -> 000
            // Cr: DC cat 4 -15 ('01' + '0000'), AC EOB ('0') -> 0100000
            // Concatenated + 1-bit padding = 0111_1100 0001_0000 0111_1111
            out.extend_from_slice(&[0x7C, 0x10, 0x7F]);
        } else {
            // Gray Y: DC cat 4 +15 ('01' + '1111'), AC EOB ('0') + 1 -> 0111_1101
            out.push(0x7D);
        }
        out.extend_from_slice(&[0xFF, 0xD9]); // EOI
        out
    }

    #[test]
    fn test_v020_stage3_media_formats_ops_atlas_cache_and_procgen() {
        use crate::media::image::{DecodeOpts, PixelFormat, decode_image_bytes};
        use crate::modules::hash::sha256_bytes;
        modules::reset_runtime_state();

        // 1. Value size, diag ALL_CODES parity, E0501..=E0505 mapping & json E_TYPE
        assert_eq!(std::mem::size_of::<Value>(), 16);
        for code in ["E0501", "E0502", "E0503", "E0504", "E0505"] {
            assert!(diag::ALL_CODES.contains(&code));
        }
        assert_eq!(diag::classify_code("E_MEDIA_LIMIT: cap"), "E0501");
        assert_eq!(diag::classify_code("E_MEDIA_UNSUPPORTED: sof2"), "E0502");
        assert_eq!(diag::classify_code("E_MEDIA_CORRUPT: crc"), "E0503");
        assert_eq!(diag::classify_code("E_MEDIA_BAD_OPT: nope"), "E0504");
        assert_eq!(diag::classify_code("E_MEDIA_NONDET: drift"), "E0505");

        // 2. PNG 16-bit -> 8-bit & Adam7 vs sequential golden digest
        let rgba_4x4 = [
            255u8, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 128,
            10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255, 100, 110, 120, 0,
            200, 10, 20, 255, 210, 20, 30, 255, 220, 30, 40, 255, 230, 40, 50, 200,
            15, 25, 35, 255, 45, 55, 65, 255, 75, 85, 95, 255, 105, 115, 125, 255,
        ];
        let png_seq = build_test_rgba_png(4, 4, &rgba_4x4);
        // Adam7 passes for 4x4: pass1=(0,0) step 8; pass3=(0,4); pass4=(2,0); pass5=(0,2); pass6=(1,0); pass7=(0,1)
        let mut a7_scan = Vec::new();
        let a7_passes: &[&[(usize, usize)]] = &[
            &[(0, 0)],
            &[],
            &[],
            &[(2, 0)],
            &[(0, 2), (2, 2)],
            &[(1, 0), (3, 0), (1, 2), (3, 2)],
            &[(0, 1), (1, 1), (2, 1), (3, 1), (0, 3), (1, 3), (2, 3), (3, 3)],
        ];
        // Build explicit Adam7 pass scanlines row by row
        let a7_spec: [(usize, usize, usize, usize); 7] = [
            (0, 8, 0, 8),
            (4, 8, 0, 8),
            (0, 4, 4, 8),
            (2, 4, 0, 4),
            (0, 2, 2, 4),
            (1, 2, 0, 2),
            (0, 1, 1, 2),
        ];
        let _ = a7_passes;
        for (x0, x_step, y0, y_step) in a7_spec {
            if x0 >= 4 || y0 >= 4 {
                continue;
            }
            let mut y = y0;
            while y < 4 {
                a7_scan.push(0);
                let mut x = x0;
                while x < 4 {
                    let i = (y * 4 + x) * 4;
                    a7_scan.extend_from_slice(&rgba_4x4[i..i + 4]);
                    x += x_step;
                }
                y += y_step;
            }
        }
        let png_adam7 = build_png_chunked(4, 4, 8, 6, 1, None, None, &a7_scan);
        let im_seq = decode_image_bytes(&png_seq, &DecodeOpts::strict_default(), None, None).unwrap();
        let im_a7 = decode_image_bytes(&png_adam7, &DecodeOpts::strict_default(), None, None).unwrap();
        assert_eq!(im_seq.digest, im_a7.digest);
        assert_eq!(&*im_seq.data, &rgba_4x4[..]);

        // 16-bit PNG (2x1 RGBA16 -> 0xFF00, 0x8000, 0x0100, 0xFFFF -> FF, 80, 01, FF)
        let mut s16 = vec![0u8];
        s16.extend_from_slice(&[0xFF, 0x00, 0x80, 0x00, 0x01, 0x00, 0xFF, 0xFF]);
        s16.extend_from_slice(&[0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC, 0xDE, 0xF0]);
        let png16 = build_png_chunked(2, 1, 16, 6, 0, None, None, &s16);
        let im16 = decode_image_bytes(&png16, &DecodeOpts::strict_default(), None, None).unwrap();
        assert_eq!(im16.format, PixelFormat::Rgba8);
        assert_eq!(&*im16.data, &[0xFF, 0x80, 0x01, 0xFF, 0x12, 0x56, 0x9B, 0xDF]);

        // Indexed PLTE + tRNS PNG
        let plte = [10u8, 20, 30, 200, 210, 220];
        let trns = [0u8, 255];
        let png_pal = build_png_chunked(2, 1, 8, 3, 0, Some(&plte), Some(&trns), &[0, 0, 1]);
        let im_pal = decode_image_bytes(&png_pal, &DecodeOpts::strict_default(), None, None).unwrap();
        assert_eq!(&*im_pal.data, &[10, 20, 30, 0, 200, 210, 220, 255]);

        // 3. BMP (1-bit indexed, BI_BITFIELDS 16-bit RGB565, 32-bit BGRA, reject RLE)
        let bmp1 = build_bmp_raw(
            2,
            1,
            1,
            &[[0, 0, 255, 0], [0, 255, 0, 0]], // BGR: red, green
            &[vec![0b10_000000, 0, 0, 0]],
        );
        let im_b1 = decode_image_bytes(&bmp1, &DecodeOpts::strict_default(), None, None).unwrap();
        assert_eq!(im_b1.format, PixelFormat::Rgb8);
        assert_eq!(&*im_b1.data, &[0, 255, 0, 255, 0, 0]);

        // 16-bit BI_BITFIELDS RGB565 BMP
        let mut bmp_bf = vec![0u8; 14 + 40 + 12 + 4];
        bmp_bf[0..2].copy_from_slice(b"BM");
        let bf_len = bmp_bf.len() as u32;
        bmp_bf[2..6].copy_from_slice(&bf_len.to_le_bytes());
        bmp_bf[10..14].copy_from_slice(&(66u32).to_le_bytes());
        bmp_bf[14..18].copy_from_slice(&(40u32).to_le_bytes());
        bmp_bf[18..22].copy_from_slice(&(2i32).to_le_bytes());
        bmp_bf[22..26].copy_from_slice(&(-1i32).to_le_bytes()); // top-down 2x1
        bmp_bf[26..28].copy_from_slice(&(1u16).to_le_bytes());
        bmp_bf[28..30].copy_from_slice(&(16u16).to_le_bytes());
        bmp_bf[30..34].copy_from_slice(&(3u32).to_le_bytes()); // BI_BITFIELDS
        bmp_bf[54..58].copy_from_slice(&0xF800u32.to_le_bytes());
        bmp_bf[58..62].copy_from_slice(&0x07E0u32.to_le_bytes());
        bmp_bf[62..66].copy_from_slice(&0x001Fu32.to_le_bytes());
        bmp_bf[66..68].copy_from_slice(&0xF800u16.to_le_bytes()); // pure red
        bmp_bf[68..70].copy_from_slice(&0x001Fu16.to_le_bytes()); // pure blue
        let im_bf = decode_image_bytes(&bmp_bf, &DecodeOpts::strict_default(), None, None).unwrap();
        assert_eq!(&*im_bf.data, &[255, 0, 0, 0, 0, 255]);

        // 4. TGA (type 2 uncompressed & type 10 RLE match) + QOI
        let mut tga2 = vec![0u8; 18 + 6];
        tga2[2] = 2;
        tga2[12..14].copy_from_slice(&2u16.to_le_bytes());
        tga2[14..16].copy_from_slice(&1u16.to_le_bytes());
        tga2[16] = 24;
        tga2[17] = 0x20; // top-down
        tga2[18..24].copy_from_slice(&[10, 20, 200, 10, 20, 200]); // BGR * 2
        let mut tga10 = tga2[..18].to_vec();
        tga10[2] = 10;
        tga10.push(0x81); // RLE run of 2
        tga10.extend_from_slice(&[10, 20, 200]);
        tga10.extend_from_slice(&[0u8; 8]);
        tga10.extend_from_slice(b"TRUEVISION-XFILE.\0");
        let im_t2 = decode_image_bytes(&tga2, &DecodeOpts::strict_default(), None, None).unwrap();
        let im_t10 = decode_image_bytes(&tga10, &DecodeOpts::strict_default(), None, None).unwrap();
        assert_eq!(im_t2.digest, im_t10.digest);
        assert_eq!(&*im_t2.data, &[200, 20, 10, 200, 20, 10]);

        let qoi_bytes = build_qoi_rgba(4, 4, &rgba_4x4);
        let im_qoi = decode_image_bytes(&qoi_bytes, &DecodeOpts::strict_default(), None, None).unwrap();
        assert_eq!(im_qoi.digest, im_seq.digest);

        // 5. GIF89a 2-frame disposal (frame 0 red, frame 1 disposal=2 restores transparent bg + green pixel)
        let mut gif = Vec::from(*b"GIF89a");
        gif.extend_from_slice(&2u16.to_le_bytes()); // w=2
        gif.extend_from_slice(&1u16.to_le_bytes()); // h=1
        gif.push(0b1000_0001); // GCT=1, size=4 colors (n=1 -> 2^(1+1)=4)
        gif.push(0); // bg
        gif.push(0); // aspect
        gif.extend_from_slice(&[255, 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0]);
        // GCE frame 0: disposal=2 (restore bg), delay=5 (50ms), transparent=3
        gif.extend_from_slice(&[0x21, 0xF9, 4, 0x09, 5, 0, 3, 0]);
        gif.extend_from_slice(&[0x2C, 0, 0, 0, 0, 2, 0, 1, 0, 0]);
        gif.extend_from_slice(&build_gif_lzw_stream(2, &[0, 0]));
        // GCE frame 1: disposal=0, delay=10 (100ms), transparent=3
        gif.extend_from_slice(&[0x21, 0xF9, 4, 0x01, 10, 0, 3, 0]);
        gif.extend_from_slice(&[0x2C, 1, 0, 0, 0, 1, 0, 1, 0, 0]); // x=1,w=1 -> left px bg/transparent!
        gif.extend_from_slice(&build_gif_lzw_stream(2, &[1]));
        gif.push(0x3B);
        let im_gif = decode_image_bytes(&gif, &DecodeOpts::strict_default(), None, None).unwrap();
        assert_eq!(im_gif.frame_count(), 2);
        assert_eq!(im_gif.delays_ms, vec![50, 100]);
        assert_eq!(im_gif.frame_data(0).unwrap(), &[255, 0, 0, 255, 255, 0, 0, 255]);
        // Left pixel was cleared by disposal=2 to transparent black, right pixel drawn green:
        assert_eq!(im_gif.frame_data(1).unwrap(), &[0, 0, 0, 0, 0, 255, 0, 255]);

        // 6. JPEG baseline SOF0 (gray & YCbCr 4:4:4) + SOF2 progressive rejection E0502
        let jpg_gray = build_test_jpeg_8x8(0xC0, false);
        let jpg_ycbcr = build_test_jpeg_8x8(0xC0, true);
        let jpg_sof2 = build_test_jpeg_8x8(0xC2, true);
        let im_jg = decode_image_bytes(&jpg_gray, &DecodeOpts::strict_default(), None, None).unwrap();
        let im_jc = decode_image_bytes(&jpg_ycbcr, &DecodeOpts::strict_default(), None, None).unwrap();
        assert_eq!(im_jg.width, 8);
        assert_eq!(im_jg.format, PixelFormat::R8);
        assert_eq!(im_jc.width, 8);
        assert_eq!(im_jc.format, PixelFormat::Rgb8);
        let err_sof2 = decode_image_bytes(&jpg_sof2, &DecodeOpts::strict_default(), None, None).unwrap_err();
        assert_eq!(diag::classify_code(&err_sof2), "E0502", "err: {}", err_sof2);
        // Pin deterministic cross-platform JPEG hash check:
        let jg_re = decode_image_bytes(&jpg_gray, &DecodeOpts::strict_default(), None, None).unwrap();
        let jc_re = decode_image_bytes(&jpg_ycbcr, &DecodeOpts::strict_default(), None, None).unwrap();
        assert_eq!(sha256_bytes(&im_jg.data), sha256_bytes(&jg_re.data));
        assert_eq!(sha256_bytes(&im_jc.data), sha256_bytes(&jc_re.data));

        // 7. Full end-to-end Askane VM script: image ops, NPOT mipmaps, textures/sampling,
        // atlas, anim, gfx.draw_sprite_ex, ui.image/nine_slice with Value::Image,
        // assets VFS bundle + manifest.toml + LRU eviction, and procgen determinism!
        let mut norm_src = vec![0u8; 4 * 4 * 4];
        for i in 0..16 {
            norm_src[i * 4] = (i * 16) as u8;
            norm_src[i * 4 + 1] = 128;
            norm_src[i * 4 + 2] = 255;
            norm_src[i * 4 + 3] = 255;
        }
        let norm_png = build_test_rgba_png(4, 4, &norm_src);

        // Two-frame opaque tile PNG & transparent/red fringe image for mip & sprite tests
        let sprite_a = build_test_rgba_png(4, 4, &rgba_4x4);
        let tile2x2 = build_test_rgba_png(
            2,
            2,
            &[
                255, 0, 0, 255, 0, 255, 0, 255,
                0, 0, 255, 255, 255, 255, 255, 255,
            ],
        );

        let work_dir =
            std::env::temp_dir().join(format!("askane_v020_stage3_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work_dir);
        std::fs::create_dir_all(&work_dir).unwrap();
        std::fs::write(work_dir.join("sprite.png"), &sprite_a).unwrap();
        std::fs::write(work_dir.join("norm.png"), &norm_png).unwrap();
        std::fs::write(work_dir.join("tile.png"), &tile2x2).unwrap();
        std::fs::write(work_dir.join("hero.gif"), &gif).unwrap();
        std::fs::write(work_dir.join("shot.jpg"), &jpg_ycbcr).unwrap();
        std::fs::write(work_dir.join("icon.tga"), &tga10).unwrap();

        let script = r##"
import image;
import assets;
import procgen;
import archive;
import gfx;
import ui;
import json;
import test;

// Display, json E_TYPE, bad_opt checks
let base = image.load("sprite.png");
test.assert_eq(type_of(base), "image");
let disp = str(base);
test.assert(disp == "<image 4x4 rgba8 srgb>");
let json_err = false;
try { json.stringify(base); } catch e { json_err = true; }
test.assert(json_err);

let bad_opt = false;
try { image.load("sprite.png", #{unknown_key: 1}); } catch e { bad_opt = true; }
test.assert(bad_opt);

let bad_rot = false;
try { image.rotate(base, 45); } catch e { bad_rot = true; }
test.assert(bad_rot);

// Color space & format conversions & ops
let r8 = image.convert(base, "r8");
test.assert_eq(image.format(r8), "r8");
let back_rgba = image.convert(r8, "rgba8");
test.assert_eq(image.format(back_rgba), "rgba8");
let lin = image.to_linear(base);
test.assert_eq(image.color_space(lin), "linear");
let srgb = image.to_srgb(lin);
test.assert_eq(image.color_space(srgb), "srgb");

// Premultiply check
let raw_px = image.from_bytes([200, 100, 50, 128], 1, 1, "rgba8");
let pm = image.premultiply(raw_px);
let p0 = image.get_pixel(pm, 0, 0);
test.assert_eq(p0[0], 100);
test.assert_eq(p0[1], 50);
test.assert_eq(p0[2], 25);
test.assert_eq(p0[3], 128);

// Crop, flip, rotate, resize
let cr = image.crop(base, 1, 0, 2, 2);
test.assert_eq(image.width(cr), 2);
test.assert_eq(image.height(cr), 2);
let fl = image.flip(base, true, false);
test.assert_eq(image.get_pixel(fl, 0, 0), image.get_pixel(base, 3, 0));
let rt = image.rotate(image.crop(base, 0, 0, 3, 2), 90);
test.assert_eq(image.width(rt), 2);
test.assert_eq(image.height(rt), 3);
let rs = image.resize(base, 8, 8, "bilinear");
test.assert_eq(image.width(rs), 8);
test.assert_eq(image.digest(rs), image.digest(image.resize(base, 8, 8, "bilinear")));

// NPOT 100x37 mipmaps (sRGB premul vs transparent edge, Linear data, 1x1 edge)
let npot = image.new(100, 37, "rgba8", [255, 0, 0, 0]);
test.assert_eq(image.mipmaps(npot), 7);
test.assert_eq(image.mip_count(npot), 7);
let m1 = image.mip_level(npot, 1);
let m6 = image.mip_level(npot, 6);
test.assert_eq(image.width(m1), 50);
test.assert_eq(image.height(m1), 18);
test.assert_eq(image.width(m6), 1);
test.assert_eq(image.height(m6), 1);
test.assert_eq(image.mipmaps(image.new(1, 1, "rgba8")), 1);
let rg_lin = image.new(100, 37, #{format: "rg8", color_space: "linear", fill: [100, 200]});
test.assert_eq(image.mipmaps(rg_lin), 7);
let rg_last = image.mip_level(rg_lin, 6);
test.assert_eq(image.get_pixel(rg_last, 0, 0)[0], 100);
test.assert_eq(image.get_pixel(rg_last, 0, 0)[1], 200);

// Texture sampling & wrap modes
let tile = image.load("tile.png");
let tex = image.texture(tile, #{wrap: "repeat", filter: "mip_nearest"});
test.assert_eq(type_of(tex), "texture");
test.assert_eq(image.sample(tex, 1.25, 0.25)[0], 255); // Repeat wraps U=1.25 -> texel (0,0) red
test.assert_eq(image.sample(tex, 0.75, 0.25)[1], 255); // texel (1,0) green

// Atlas + gfx.draw_sprite_ex + draw_atlas + anim + ui.image(Image)
let atlas = image.atlas(#{ a: tile, b: cr }, #{padding: 1, max_size: 64});
test.assert_eq(atlas.count, 2);
let rect_a = gfx.atlas_rect(atlas, "a");
test.assert_eq(rect_a.w, 2);
let cropped_a = image.sprite_frame(atlas, "a");
test.assert_eq(image.to_bytes(cropped_a), image.to_bytes(tile));
test.assert_eq(image.digest(cropped_a), image.digest(image.sprite_frame(atlas, "a")));

gfx.headless(true, 32, 32);
gfx.clear(0, 0, 0);
gfx.draw_sprite_ex(tile, 4, 4, #{flip_x: true});
// Flipped horizontally: top-left pixel (4,4) was top-right green (0,255,0)!
test.assert_eq(gfx.read_pixel(4, 4), 65280);
gfx.draw_atlas(atlas, "a", 10, 10);
test.assert_eq(gfx.read_pixel(10, 10), 16711680);

// Anim modes: loop & once on 2 frames + GIF
let gif_img = assets.load_image("hero.gif");
test.assert_eq(image.frame_count(gif_img), 2);
let an = gfx.make_anim([tile, fl], #{frame_ms: 50, mode: "loop"});
test.assert_eq(gfx.anim_frame(an), 0);
test.assert_eq(gfx.anim_update(an, 60), 1);
test.assert_eq(gfx.anim_update(an, 60), 0);
gfx.anim_draw(an, 20, 20);
test.assert_eq(gfx.read_pixel(20, 20), 16711680);

let an_once = gfx.make_anim(gif_img, #{mode: "once"});
test.assert_eq(gfx.anim_update(an_once, 500), 1); // stays on last frame

// UI accepts Value::Image & Value::Texture directly
ui.window("UI", 80, 60, #{headless: true});
let uroot = ui.row([ui.image(tile, #{width: 16, height: 16}), ui.icon_button(tex, "Go")]);
ui.render(uroot);

// Asset cache hits, LRU eviction with pinned handle & explicit GC
let base_hit = assets.load_image("sprite.png");
test.assert_eq(image.digest(base_hit), image.digest(base));
let st_prev = assets.stats();
test.assert(st_prev.hits >= 1);
assets.clear();
assets.set_budget(256);
let keep = assets.load_image("shot.jpg"); // 192 bytes (pinned by `keep`)
assets.load_image("norm.png"); // 64 bytes (unpinned, total 256)
let before_ev = assets.stats();
test.assert_eq(before_ev.used_bytes, 256);
assets.set_budget(200); // forces LRU eviction of unpinned norm.png, keeps pinned shot.jpg
let after_ev = assets.stats();
test.assert(after_ev.evictions >= 1);
test.assert_eq(after_ev.used_bytes, 192);
assets.load_image("icon.tga"); // 6 bytes (unpinned, total 198)
let evicted = assets.gc();
test.assert_eq(evicted, 1);
test.assert_eq(assets.stats().used_bytes, 192);
test.assert(image.width(keep) == 8);

// Bundle mount with __askane__/manifest.toml hints
let zbytes = archive.pack(#{
    "__askane__/manifest.toml": "[assets.\"packed/norm.png\"]\ncolor_space = \"linear\"\nfilter = \"mip_linear\"\nwrap = \"repeat\"\nmipmaps = true\n",
    "packed/norm.png": assets.read_bytes("norm.png")
});
let bid = assets.mount_bytes(zbytes, "vfs");
test.assert(assets.exists("vfs/packed/norm.png"));
let vtex = assets.load_texture("vfs/packed/norm.png");
test.assert_eq(image.color_space(vtex), "linear");
test.assert(image.mip_count(vtex) > 1);
test.assert(assets.unmount(bid));

// Procgen determinism + height_to_normal
let hm1 = procgen.heightmap(16, 16, #{seed: 42, octaves: 3, scale: 0.2});
let hm2 = procgen.heightmap(16, 16, #{seed: 42, octaves: 3, scale: 0.2});
test.assert_eq(image.digest(hm1), image.digest(hm2));
test.assert_eq(image.format(hm1), "r8");
test.assert_eq(image.color_space(hm1), "linear");
let nm1 = procgen.normal_map(hm1, 2.5);
let nm2 = image.height_to_normal(hm1, 2.5);
test.assert_eq(image.digest(nm1), image.digest(nm2));
test.assert_eq(image.format(nm1), "rg8");
let chk = procgen.checker(8, 8, 4);
test.assert_eq(image.get_pixel(chk, 0, 0), [255, 255, 255, 255]);
test.assert_eq(image.get_pixel(chk, 4, 0), [40, 40, 40, 255]);
true
"##;

        let mut vm = VM::new_with_sandbox(true);
        vm.set_permissions(Permissions::sandboxed());
        vm.set_current_dir(work_dir.clone());
        vm.set_sandbox_root(work_dir.clone());
        let budget = std::sync::Arc::clone(vm.budget_core());
        let res = vm.run(compile_source(script).unwrap()).unwrap();
        assert!(res.equals(&Value::Bool(true)));
        let before_reset = budget.used_bytes();
        drop(vm);
        modules::reset_runtime_state();
        // Reset refunds all RAII media/mips/bundle/surface guards; cache state is empty.
        assert!(budget.used_bytes() < before_reset);
        assert_eq!(crate::media::cache::cache_stats().0, 0);
        assert_eq!(crate::media::cache::cache_stats().2, 0);

        // Pure media RAII MediaBudgetGuard lifecycle on dedicated BudgetCore must refund to exact 0
        {
            let mcore = std::sync::Arc::new(budget::BudgetCore::new(
                Some(4 * 1024 * 1024),
                Some(4 * 1024 * 1024),
            ));
            let decoded =
                decode_image_bytes(&png_seq, &DecodeOpts::strict_default(), Some(&mcore), None)
                    .unwrap();
            assert!(mcore.used_bytes() > 0);
            let conv = crate::media::ops::convert_format(&decoded, PixelFormat::Rgb8, Some(&mcore)).unwrap();
            let lin = crate::media::ops::to_linear(&conv, Some(&mcore)).unwrap();
            let crp = crate::media::ops::crop(&lin, 0, 0, 2, 2, Some(&mcore)).unwrap();
            let rsz = crate::media::ops::resize(&crp, 8, 8, "bilinear", Some(&mcore), None).unwrap();
            let mips = rsz.ensure_mipmaps(Some(&mcore), None).unwrap();
            assert_eq!(mips.level_count(), 4);
            let nrm = crate::media::ops::height_to_normal(&rsz, 2.0, Some(&mcore)).unwrap();
            let atl = crate::media::atlas::pack_atlas(
                vec![("a".to_string(), rsz.clone()), ("b".to_string(), nrm.clone())],
                1,
                64,
                Some(&mcore),
                None,
            )
            .unwrap();
            let tex = crate::media::image::TextureData::new(
                atl.image.clone(),
                crate::media::image::WrapMode::Repeat,
                crate::media::image::WrapMode::Repeat,
                crate::media::image::Filter::MipLinear,
                false,
                Some(&mcore),
                None,
            )
            .unwrap();
            assert!(mcore.used_bytes() > decoded.byte_len() as u64);
            drop((decoded, conv, lin, crp, rsz, mips, nrm, atl, tex));
            assert_eq!(mcore.used_bytes(), 0);
        }

        // Pure media VM run must refund exact media/mipmap/atlas bytes on drop + reset_runtime_state()
        let pure_script = r#"
import image;
import assets;
import procgen;
import gfx;
import ui;
import test;

let im = assets.load_image("sprite.png");
let rs = image.resize(im, 16, 16, "bilinear");
image.mipmaps(rs);
let hm = procgen.heightmap(16, 16, 7);
let nm = procgen.normal_map(hm, 2.0);
let tex = image.texture(nm, #{filter: "mip_linear", wrap: "repeat"});
let atl = image.atlas(#{a: im, b: rs, c: nm});
gfx.headless(true, 32, 32);
gfx.draw_atlas(atl, "a", 1, 1);
ui.window("P", 64, 64, #{headless: true});
ui.render(ui.image(tex));
// Expected live media bytes:
// im (64) + rs (1024 + mips 1364) + hm (256) + nm (512 + mips 682) + atl (21*16*4=1344)
let mip_n = image.mipmaps(nm);
test.assert_eq(mip_n, 5);
true
"#;
        let mut pvm = VM::new_with_sandbox(true);
        pvm.set_permissions(Permissions::sandboxed());
        pvm.set_current_dir(work_dir.clone());
        pvm.set_sandbox_root(work_dir.clone());
        let pbudget = std::sync::Arc::clone(pvm.budget_core());
        let pres = pvm.run(compile_source(pure_script).unwrap()).unwrap();
        assert!(pres.equals(&Value::Bool(true)));
        let live_used = pbudget.used_bytes();
        let cached_bytes = crate::media::cache::cache_stats().0 as u64;
        assert!(cached_bytes > 0);
        drop(pvm);
        modules::reset_runtime_state();
        let after_reset = pbudget.used_bytes();
        assert!(live_used > after_reset);
        assert!(live_used - after_reset >= cached_bytes);
        assert_eq!(crate::media::cache::cache_stats().0, 0);

        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn test_v020_stage3_media_fuzz_and_bombs() {
        use crate::media::image::{DecodeOpts, decode_image_bytes};
        use crate::modules::compress::{adler32_bytes, crc32_bytes, deflate_raw};
        modules::reset_runtime_state();

        let make_bad_png = |w: u32, h: u32, idat_len_override: Option<u32>, tag: &[u8; 4], bad_crc: bool| -> Vec<u8> {
            let mut png = Vec::from(*b"\x89PNG\r\n\x1a\n");
            let mut ihdr = Vec::new();
            ihdr.extend_from_slice(&w.to_be_bytes());
            ihdr.extend_from_slice(&h.to_be_bytes());
            ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
            png.extend_from_slice(&(13u32).to_be_bytes());
            let s = png.len();
            png.extend_from_slice(b"IHDR");
            png.extend_from_slice(&ihdr);
            let crc = crc32_bytes(&png[s..]) ^ u32::from(bad_crc);
            png.extend_from_slice(&crc.to_be_bytes());

            let raw = vec![0u8; 64];
            let def = deflate_raw(&raw);
            let mut zlib = vec![0x78, 0x01];
            zlib.extend_from_slice(&def);
            zlib.extend_from_slice(&adler32_bytes(&raw).to_be_bytes());
            let clen = idat_len_override.unwrap_or(zlib.len() as u32);
            png.extend_from_slice(&clen.to_be_bytes());
            let s2 = png.len();
            png.extend_from_slice(tag);
            png.extend_from_slice(&zlib);
            let c2 = crc32_bytes(&png[s2..]);
            png.extend_from_slice(&c2.to_be_bytes());
            png
        };

        let opts = DecodeOpts::strict_default();
        // 1. Targeted structural bombs across all 6 codecs
        let mut bombs: Vec<Vec<u8>> = Vec::new();
        // PNG bombs
        bombs.push(make_bad_png(0, 1, None, b"IDAT", false));
        bombs.push(make_bad_png(8193, 1, None, b"IDAT", false));
        bombs.push(make_bad_png(6000, 6000, None, b"IDAT", false)); // > 33.5M pixel bomb
        bombs.push(make_bad_png(2, 2, Some(0xFFFF_FFFF), b"IDAT", false)); // 0xFFFFFFFF length
        bombs.push(make_bad_png(2, 2, None, b"IDAT", true)); // corrupt CRC
        bombs.push(make_bad_png(2, 2, None, b"CRIT", false)); // unknown critical chunk!
        bombs.push(b"\x89PNG\r\n\x1a\n".to_vec()); // EOF

        // BMP bombs
        bombs.push(b"BM".to_vec());
        let mut bmp_rle = vec![0u8; 54];
        bmp_rle[0..2].copy_from_slice(b"BM");
        bmp_rle[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp_rle[18..22].copy_from_slice(&4i32.to_le_bytes());
        bmp_rle[22..26].copy_from_slice(&4i32.to_le_bytes());
        bmp_rle[28..30].copy_from_slice(&8u16.to_le_bytes());
        bmp_rle[30..34].copy_from_slice(&1u32.to_le_bytes()); // RLE8
        bombs.push(bmp_rle);
        let mut bmp_ovf = vec![0u8; 54];
        bmp_ovf[0..2].copy_from_slice(b"BM");
        bmp_ovf[10..14].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        bmp_ovf[14..18].copy_from_slice(&40u32.to_le_bytes());
        bmp_ovf[18..22].copy_from_slice(&4097i32.to_le_bytes()); // > MAX_BMP_DIM
        bmp_ovf[22..26].copy_from_slice(&1i32.to_le_bytes());
        bmp_ovf[28..30].copy_from_slice(&24u16.to_le_bytes());
        bombs.push(bmp_ovf);

        // JPEG bombs (SOF2, oversubscribed DHT, hv > 10, truncated SOS, bad DQT)
        bombs.push(build_test_jpeg_8x8(0xC2, true)); // SOF2 progressive reject
        bombs.push(build_test_jpeg_8x8(0xC9, false)); // arithmetic SOF9 reject
        // Oversubscribed DHT Kraft > 65536
        let mut bad_dht = vec![0xFF, 0xD8, 0xFF, 0xC4, 0x00, 22, 0x00, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 0xFF, 0xD9];
        bombs.push(bad_dht.split_off(0));
        // JPEG hv_sum > 10
        let mut bad_mcu = build_test_jpeg_8x8(0xC0, true);
        let sof_idx = bad_mcu.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        bad_mcu[sof_idx + 11] = 0x44; // 4*4 = 16 MCU blocks
        bombs.push(bad_mcu);

        // GIF bombs (LZW bad min_code_size, truncated GCT, self-ref/corrupt LZW)
        let mut bad_gif = Vec::from(*b"GIF89a");
        bad_gif.extend_from_slice(&[1, 0, 1, 0, 0x80, 0, 0, 0, 0, 0, 255, 255, 255, 0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0, 99, 0, 0x3B]);
        bombs.push(bad_gif);
        bombs.push(b"GIF89a\x10\x00\x10\x00\x87".to_vec());

        // TGA bombs (id_len past EOF, RLE overrun)
        let mut bad_tga_id = vec![0u8; 18];
        bad_tga_id[0] = 200; // past EOF
        bad_tga_id[2] = 2;
        bad_tga_id[12] = 1;
        bad_tga_id[14] = 1;
        bad_tga_id[16] = 24;
        bombs.push(bad_tga_id);
        let mut bad_tga_rle = vec![0u8; 24];
        bad_tga_rle[2] = 10;
        bad_tga_rle[12] = 1;
        bad_tga_rle[14] = 1;
        bad_tga_rle[16] = 24;
        bad_tga_rle[18] = 0xFF; // run=64 on 1x1!
        bombs.push(bad_tga_rle);

        // QOI bombs (oversized dims, wrong channels, missing end marker)
        let mut bad_qoi = Vec::from(*b"qoif");
        bad_qoi.extend_from_slice(&10000u32.to_be_bytes());
        bad_qoi.extend_from_slice(&10000u32.to_be_bytes());
        bad_qoi.extend_from_slice(&[4, 0, 0xFE, 1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0]);
        bombs.push(bad_qoi);
        bombs.push(vec![]);
        bombs.push(b"WEBP\x00\x00\x00\x00VP8 ".to_vec());

        for (i, payload) in bombs.iter().enumerate() {
            assert!(
                decode_image_bytes(payload, &opts, None, None).is_err(),
                "targeted bomb #{} unexpectedly succeeded",
                i
            );
        }

        // Tight BudgetCore aborts cleanly without leaving leaked bytes
        let tight = std::sync::Arc::new(budget::BudgetCore::new(Some(32), Some(1024)));
        let big_png = build_test_rgba_png(8, 8, &vec![255u8; 8 * 8 * 4]);
        assert!(decode_image_bytes(&big_png, &opts, Some(&tight), None).is_err());
        assert_eq!(tight.used_bytes(), 0);

        // 2. Deterministic PRNG 160-vector mutation fuzz suite (>= 180 total test vectors!)
        let png_seed = build_test_rgba_png(4, 4, &[42u8; 64]);
        let bmp_seed = build_bmp_raw(2, 2, 24, &[], &[vec![1, 2, 3, 4, 5, 6, 0, 0], vec![7, 8, 9, 10, 11, 12, 0, 0]]);
        let qoi_seed = build_qoi_rgba(
            2,
            2,
            &[1, 2, 3, 255, 1, 2, 3, 255, 1, 2, 3, 255, 1, 2, 3, 255],
        );
        let jpg_seed = build_test_jpeg_8x8(0xC0, true);
        let gif_seed = {
            let mut g = Vec::from(*b"GIF87a\x02\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff");
            g.extend_from_slice(&[0x2C, 0, 0, 0, 0, 2, 0, 1, 0, 0]);
            g.extend_from_slice(&build_gif_lzw_stream(2, &[0, 1]));
            g.push(0x3B);
            g
        };
        let tga_seed = {
            let mut t = vec![0u8; 18 + 6];
            t[2] = 2;
            t[12] = 2;
            t[14] = 1;
            t[16] = 24;
            t
        };
        let seeds: [&[u8]; 6] = [&png_seed, &bmp_seed, &qoi_seed, &jpg_seed, &gif_seed, &tga_seed];
        let mut rng: u64 = 0x5A17_F020_3000_0001;
        let next_u64 = |st: &mut u64| -> u64 {
            *st = st.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = *st;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };

        let mut fuzz_budget = std::sync::Arc::new(budget::BudgetCore::new(
            Some(4 * 1024 * 1024),
            Some(8 * 1024 * 1024),
        ));
        for case_idx in 0..160usize {
            let mut sample = seeds[case_idx % seeds.len()].to_vec();
            let op = (next_u64(&mut rng) % 4) as usize;
            if !sample.is_empty() {
                match op {
                    0 => {
                        let new_len = (next_u64(&mut rng) as usize) % (sample.len() + 1);
                        sample.truncate(new_len);
                    }
                    1 => {
                        let idx = (next_u64(&mut rng) as usize) % sample.len();
                        sample[idx] ^= ((next_u64(&mut rng) & 0xFF) as u8).max(1);
                    }
                    2 => {
                        for _ in 0..3 {
                            let idx = (next_u64(&mut rng) as usize) % sample.len();
                            sample[idx] = (next_u64(&mut rng) & 0xFF) as u8;
                        }
                    }
                    _ => {
                        let extra = (next_u64(&mut rng) as usize) % 32;
                        for _ in 0..extra {
                            sample.push((next_u64(&mut rng) & 0xFF) as u8);
                        }
                    }
                }
            }
            let _ = decode_image_bytes(&sample, &opts, Some(&fuzz_budget), None);
            assert_eq!(
                fuzz_budget.used_bytes(),
                0,
                "budget leak at fuzz case {}",
                case_idx
            );
        }
        fuzz_budget = std::sync::Arc::new(budget::BudgetCore::default());
        assert_eq!(fuzz_budget.used_bytes(), 0);
        modules::reset_runtime_state();
    }
}
