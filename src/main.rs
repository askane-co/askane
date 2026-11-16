mod ast;
mod bytecode;
mod compiler;
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

const VERSION: &str = "0.11.0";
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

fn run_file(path: &str, dump_bytecode: bool, permissions: Permissions) -> Result<(), String> {
    let source = read_text_limited(std::path::Path::new(path), MAX_SOURCE_BYTES)
        .map_err(|e| format!("Could not read '{}': {}", path, e))?;
    let proto = compile_source(&source)?;
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
    vm.run(proto)?;
    Ok(())
}

fn run_repl() {
    println!("Askane v{} (open beta)", VERSION);
    println!("Type :help for commands, :exit to quit.");

    let mut vm = VM::new();
    let stdin = io::stdin();

    loop {
        print!("askane> ");
        let _ = io::stdout().flush();

        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                eprintln!("Read error: {}", e);
                break;
            }
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match trimmed {
            ":exit" | ":quit" => break,
            ":help" => {
                println!("Commands:");
                println!("  :help   Show this message");
                println!("  :exit   Exit the REPL");
                println!("  :bc     Prefix any code with ':bc ' to inspect its bytecode");
                continue;
            }
            _ => {}
        }

        let (show_bc, code) = if let Some(rest) = trimmed.strip_prefix(":bc ") {
            (true, rest)
        } else {
            (false, trimmed)
        };

        match compile_source(code) {
            Ok(proto) => {
                if show_bc {
                    print!("{}", proto.chunk.disassemble(&proto.name));
                }
                match vm.run(proto) {
                    Ok(Value::Unit) => {}
                    Ok(val) => println!("{}", val),
                    Err(e) => eprintln!("Runtime error: {}", e),
                }
            }
            Err(e) => eprintln!("Compile error: {}", e),
        }
    }
}

fn print_help() {
    println!("Askane v{}", VERSION);
    println!();
    println!("Usage:");
    println!("  askane                  Start interactive REPL");
    println!("  askane <file.ask>       Run an Askane script");
    println!("  askane -b <file.ask>    Disassemble compiled bytecode");
    println!("  askane -h | --help      Show help");
    println!("  askane -v | --version   Show version");
    println!();
    println!("Sandbox permissions:");
    println!("  --allow-net[=hosts]     Allow HTTPS access (optionally comma-separated host[:port])");
    println!("  --allow-read[=dirs]     Allow reading outside sandbox (optionally comma-separated dirs)");
    println!("  --allow-write[=dirs]    Allow writing outside sandbox (optionally comma-separated dirs)");
    println!("  --allow-env[=vars]      Allow sys.env() (optionally comma-separated names)");
    println!("  --allow-args            Allow sys.args()");
    println!("  --max-steps=N           Override sandbox instruction budget (default 10000000)");
    println!("  --unsafe                Allow unrestricted files, network, env and process access");
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

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() == 1 {
        run_repl();
        return;
    }

    let mut dump_bytecode = false;
    let mut perms = Permissions::sandboxed();
    let mut script_path: Option<String> = None;

    for arg in &args[1..] {
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
        } else if let Some(val) = arg.strip_prefix("--max-steps=") {
            match val.parse::<u64>() {
                Ok(n) => perms.max_instructions = Some(n),
                Err(_) => {
                    eprintln!("Invalid --max-steps value: {}", val);
                    process::exit(1);
                }
            }
        } else if arg.starts_with('-') {
            eprintln!("Unknown option '{}'. Run 'askane --help' for usage.", arg);
            process::exit(1);
        } else if script_path.is_none() {
            script_path = Some(arg.clone());
        } else {
            eprintln!("Unexpected extra argument '{}'. Run 'askane --help' for usage.", arg);
            process::exit(1);
        }
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
        let proto_fs = compile_source(
            r#"
            import fs;
            fs.read("../../../Windows/System32/drivers/etc/hosts")
            "#,
        )
        .unwrap();
        let mut vm_sb = VM::new_with_sandbox(true);
        let err_fs = vm_sb.run(proto_fs).unwrap_err();
        assert!(err_fs.contains("outside the sandbox"));

        let proto_abs = compile_source(
            r#"
            import fs;
            fs.write("C:\\askane_escape_test.txt", "x")
            "#,
        )
        .unwrap();
        let mut vm_sb2 = VM::new_with_sandbox(true);
        let err_abs = vm_sb2.run(proto_abs).unwrap_err();
        assert!(err_abs.contains("outside the sandbox"));

        // 2. Plain HTTP is blocked (HTTPS required)
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
        assert!(vm_ro.resolve_read_path(&temp_file.to_string_lossy()).is_ok());
        assert!(vm_ro.resolve_write_path(&temp_file.to_string_lossy()).is_err());
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
}
