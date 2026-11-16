use std::io::{self, Write};

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "term",
        &[
            ("red", term_red),
            ("green", term_green),
            ("yellow", term_yellow),
            ("cyan", term_cyan),
            ("bold", term_bold),
            ("clear", term_clear),
        ],
        &[],
    )
}

fn wrap_ansi(code: &str, args: &[Value], fn_name: &str) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(format!("term.{}() expects 1 argument", fn_name));
    }
    Ok(Value::Str(format!("\x1b[{}m{}\x1b[0m", code, args[0])))
}

fn term_red(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    wrap_ansi("31", args, "red")
}

fn term_green(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    wrap_ansi("32", args, "green")
}

fn term_yellow(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    wrap_ansi("33", args, "yellow")
}

fn term_cyan(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    wrap_ansi("36", args, "cyan")
}

fn term_bold(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    wrap_ansi("1", args, "bold")
}

fn term_clear(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("term.clear() expects 0 arguments".to_string());
    }
    print!("\x1b[2J\x1b[H");
    let _ = io::stdout().flush();
    Ok(Value::Unit)
}
