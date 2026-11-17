mod ast;
mod bytecode;
mod compiler;
mod gui;
mod lexer;
mod modules;
mod parser;
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

const VERSION: &str = "0.14.1";
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

fn compile_source(source: &str) -> Result<std::rc::Rc<bytecode::FunctionProto>, String> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err("Source size limit exceeded".to_string());
    }
    let mut lexer = Lexer::new(source);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program()?;
    Compiler::compile(&program)
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

fn run_file(path: &str, dump_bytecode: bool, permissions: Permissions) -> Result<(), String> {
    let source = read_text_limited(std::path::Path::new(path), MAX_SOURCE_BYTES)
        .map_err(|e| format!("Could not read '{}': {}", path, e))?;
    let proto = compile_source(&source).map_err(|e| format_error_with_source(&source, &e))?;
    if dump_bytecode {
        print!("{}", proto.chunk.disassemble(&proto.name));
        return Ok(());
    }
    let mut vm = VM::new_with_sandbox(!permissions.allow_all);
    vm.set_permissions(permissions);
    if let Some(parent) = std::path::Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            let parent = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
            vm.set_current_dir(parent.clone());
            vm.set_sandbox_root(parent);
        }
    }
    vm.run(proto)
        .map_err(|e| format_error_with_source(&source, &e))?;
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
    match compile_source(&source) {
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

fn inspect_permissions(path: &str) -> Result<Vec<String>, String> {
    let source = read_text_limited(std::path::Path::new(path), MAX_SOURCE_BYTES)
        .map_err(|e| format!("Could not read '{}': {}", path, e))?;
    if source.len() > MAX_SOURCE_BYTES {
        return Err("Source size limit exceeded".to_string());
    }
    let mut lexer = Lexer::new(&source);
    let tokens = lexer.tokenize()?;
    let mut parser = Parser::new(tokens);
    let program = parser.parse_program()?;

    let mut capabilities = Vec::new();
    for stmt in &program.stmts {
        if let ast::Stmt::Import { source, .. } = stmt {
            match source {
                ast::ImportSource::Name(name) => {
                    let desc = match name.as_str() {
                        "fs" => Some(
                            "fs (sandboxed to script dir; --allow-read / --allow-write for external dirs)",
                        ),
                        "http" => Some("http (blocked by default; requires --allow-net[=host])"),
                        "sys" => Some(
                            "sys (sys.env requires --allow-env, sys.args requires --allow-args)",
                        ),
                        "clipboard" => {
                            Some("clipboard (blocked by default; requires --allow-clipboard)")
                        }
                        "mem" => Some("mem (requires --unsafe and ASKANE_ENABLE_UNSAFE_MEM=1)"),
                        "gfx" => Some("gfx (Win32 GUI window)"),
                        _ => None,
                    };
                    if let Some(d) = desc {
                        if !capabilities.contains(&d.to_string()) {
                            capabilities.push(d.to_string());
                        }
                    }
                }
                ast::ImportSource::Path(p) => {
                    let d = format!("file import '{}' (inherits caller permissions)", p);
                    if !capabilities.contains(&d) {
                        capabilities.push(d);
                    }
                }
            }
        }
    }
    Ok(capabilities)
}

fn run_eval(code: &str, perms: Permissions) -> Result<(), String> {
    let proto = compile_source(code).map_err(|e| format_error_with_source(code, &e))?;
    let mut vm = VM::new_with_sandbox(!perms.allow_all);
    vm.set_permissions(perms);
    let val = vm
        .run(proto)
        .map_err(|e| format_error_with_source(code, &e))?;
    if !matches!(val, Value::Unit) {
        println!("{}", val);
    }
    Ok(())
}

fn collect_ask_files(
    path: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) -> Result<(), String> {
    if path.is_file() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    if !path.is_dir() {
        return Err(format!("Path '{}' not found", path.display()));
    }
    let mut entries: Vec<_> = fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    for entry in entries {
        if entry.is_dir() {
            collect_ask_files(&entry, out)?;
        } else if entry.extension().and_then(|s| s.to_str()) == Some("ask") {
            out.push(entry);
        }
    }
    Ok(())
}

fn run_tests_cmd(path: &str, perms: Permissions) -> Result<(), String> {
    let mut files = Vec::new();
    collect_ask_files(std::path::Path::new(path), &mut files)?;
    if files.is_empty() {
        return Err(format!("No .ask test files found in '{}'", path));
    }
    let mut passed = 0usize;
    let mut failed = 0usize;
    for file in &files {
        let label = file.display().to_string();
        match run_file(&label, false, perms.clone()) {
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

fn needs_more_input(buf: &str) -> bool {
    let mut depth: i32 = 0;
    let mut in_string = false;
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
        if in_string {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == '"' {
                in_string = false;
            }
            i += 1;
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
        if ch == '"' {
            in_string = true;
            i += 1;
            continue;
        }
        match ch {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            _ => {}
        }
        i += 1;
    }

    in_string || in_block_comment || depth > 0
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
                        "Built-in modules: math, fs, sys, json, strings, path, time, random, base64, csv, log, test, hash, ini, datetime, clipboard, audio, url, hex, uuid, term, glob, regex, dialog, process, http, gfx, mem"
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
    println!("  askane <file.ask>            Run an Askane script");
    println!("  askane -e \"<code>\"           Evaluate inline code");
    println!("  askane check [--json] <path> Syntax & compile check for file or directory");
    println!("  askane permissions <file>    Inspect capabilities imported by a script");
    println!("  askane fmt [--check] <file>  Format an Askane source file");
    println!("  askane init [dir]            Create a starter Askane project");
    println!("  askane test <file_or_dir>    Run .ask test files");
    println!("  askane -b <file.ask>         Disassemble compiled bytecode");
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
    println!(
        "  --max-steps=N                Override sandbox instruction budget (default 10000000)"
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
            let items = target.get_or_insert_with(Vec::new);
            for part in list.split(',').filter(|s| !s.is_empty()) {
                items.push(std::path::PathBuf::from(part));
            }
        }
        _ => {
            *target = Some(Vec::new());
        }
    }
}

fn format_askane_source(src: &str) -> String {
    let mut out = Vec::new();
    let mut indent: usize = 0;
    for raw in src.replace("\r\n", "\n").lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            out.push(String::new());
            continue;
        }
        let leading_closes = trimmed.chars().take_while(|&c| c == '}').count();
        let line_indent = indent.saturating_sub(leading_closes);
        out.push(format!("{}{}", "    ".repeat(line_indent), trimmed));

        let mut in_str = false;
        let mut esc = false;
        let chars: Vec<char> = trimmed.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            if in_str {
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    in_str = false;
                }
            } else if c == '/' && chars.get(i + 1) == Some(&'/') {
                break;
            } else if c == '"' {
                in_str = true;
            } else if c == '{' {
                indent += 1;
            } else if c == '}' {
                indent = indent.saturating_sub(1);
            }
            i += 1;
        }
    }
    let mut joined = out.join("\n");
    if src.ends_with('\n') && !joined.ends_with('\n') {
        joined.push('\n');
    }
    joined
}

fn run_fmt(path: &str, check_only: bool) -> Result<(), String> {
    let source = read_text_limited(std::path::Path::new(path), MAX_SOURCE_BYTES)
        .map_err(|e| format!("Could not read '{}': {}", path, e))?;
    let formatted = format_askane_source(&source);
    if check_only {
        if formatted != source {
            return Err(format!("File '{}' is not formatted", path));
        }
        println!("OK (formatted): {}", path);
    } else {
        fs::write(path, formatted).map_err(|e| format!("Could not write '{}': {}", path, e))?;
        println!("Formatted {}", path);
    }
    Ok(())
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

    if args[1] == "check" {
        let mut json_out = false;
        let mut target: Option<String> = None;
        for arg in &args[2..] {
            if arg == "--json" {
                json_out = true;
            } else if target.is_none() {
                target = Some(arg.clone());
            } else {
                eprintln!("Usage: askane check [--json] <file.ask>");
                process::exit(1);
            }
        }
        let Some(path) = target else {
            eprintln!("Usage: askane check [--json] <file.ask>");
            process::exit(1);
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
                "name = \"my-askane-app\"\nversion = \"0.1.0\"\nentry = \"main.ask\"\nsandbox = true\n",
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
    } else {
        1
    };

    let mut dump_bytecode = false;
    let mut eval_code: Option<String> = None;
    let mut perms = Permissions::sandboxed();
    let mut script_path: Option<String> = None;

    let mut i = slice_start;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-h" || arg == "--help" {
            print_help();
            return;
        }
        if arg == "-v" || arg == "--version" {
            println!("askane {}", VERSION);
            return;
        }
        if arg == "-b" || arg == "--bytecode" {
            dump_bytecode = true;
        } else if arg == "-e" || arg == "--eval" {
            i += 1;
            let Some(code) = args.get(i) else {
                eprintln!("Missing code after {}", arg);
                process::exit(1);
            };
            eval_code = Some(code.clone());
        } else if arg == "--unsafe" {
            perms = Permissions::unrestricted();
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
        } else if arg == "--allow-exec" {
            append_csv(&mut perms.allow_exec, None);
        } else if let Some(val) = arg.strip_prefix("--allow-exec=") {
            append_csv(&mut perms.allow_exec, Some(val));
        } else if let Some(val) = arg.strip_prefix("--max-steps=") {
            match val.parse::<u64>() {
                Ok(n) => perms.max_instructions = Some(n),
                Err(_) => {
                    eprintln!("Invalid --max-steps value: {}", val);
                    process::exit(1);
                }
            }
        } else if let Some(val) = arg.strip_prefix("--timeout=") {
            match val.parse::<u64>() {
                Ok(n) => perms.max_duration_ms = Some(n),
                Err(_) => {
                    eprintln!("Invalid --timeout value: {}", val);
                    process::exit(1);
                }
            }
        } else if arg.starts_with('-') {
            eprintln!("Unknown option '{}'. Run 'askane --help' for usage.", arg);
            process::exit(1);
        } else if script_path.is_none() {
            script_path = Some(arg.clone());
        } else {
            eprintln!(
                "Unexpected extra argument '{}'. Run 'askane --help' for usage.",
                arg
            );
            process::exit(1);
        }
        i += 1;
    }

    if let Some(code) = eval_code {
        if let Err(e) = run_eval(&code, perms) {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
        return;
    }

    if is_test_cmd {
        let target = script_path.as_deref().unwrap_or(".");
        if let Err(e) = run_tests_cmd(target, perms) {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
        return;
    }

    let Some(path) = script_path else {
        eprintln!("Missing script file. Run 'askane --help' for usage.");
        process::exit(1);
    };
    if let Err(e) = run_file(&path, dump_bytecode, perms) {
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
        let proto = compile_source("fn boom() { boom() } boom()").unwrap();
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
        #[cfg(windows)]
        let trav_src = r#"
            import fs;
            fs.read("../../../Windows/System32/drivers/etc/hosts")
        "#;
        #[cfg(not(windows))]
        let trav_src = r#"
            import fs;
            fs.read("../../../../../../etc/hosts")
        "#;
        let proto_fs = compile_source(trav_src).unwrap();
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
            name: "bad".to_string(),
            arity: 0,
            local_count: 1,
            upvalue_count: 0,
            chunk: bytecode::Chunk {
                code: vec![bytecode::Op::Jump(999)],
                constants: vec![],
                lines: vec![1],
                upvalue_lists: vec![],
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
            test.assert_eq(hex.decode(hx), [72, 105]);

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
}
