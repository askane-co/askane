use std::cell::RefCell;
use std::env;
use std::process;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "sys",
        &[
            ("args", sys_args),
            ("env", sys_env),
            ("exit", sys_exit),
            ("os", sys_os),
            ("arch", sys_arch),
            ("sleep", sys_sleep),
        ],
        &[],
    )
}

fn sys_args(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_sys_args()?;
    if !args.is_empty() {
        return Err("sys.args() expects 0 arguments".to_string());
    }
    let list: Vec<Value> = env::args().map(Value::Str).collect();
    Ok(Value::List(Rc::new(RefCell::new(list))))
}

fn sys_env(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("sys.env() expects 1 argument (name)".to_string());
    }
    let name = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "sys.env() expects a string, got '{}'",
                other.type_name()
            ));
        }
    };
    vm.check_env(name)?;
    match env::var(name) {
        Ok(val) => Ok(Value::Str(val)),
        Err(_) => Ok(Value::Unit),
    }
}

fn sys_exit(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if vm.is_sandboxed() {
        return Err("sys.exit() is disabled in sandbox mode".to_string());
    }
    let code = match args {
        [] => 0,
        [Value::Int(c)] => *c as i32,
        _ => return Err("sys.exit() expects 0 or 1 integer argument".to_string()),
    };
    process::exit(code);
}

fn sys_os(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("sys.os() expects 0 arguments".to_string());
    }
    Ok(Value::Str(env::consts::OS.to_string()))
}

fn sys_arch(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("sys.arch() expects 0 arguments".to_string());
    }
    Ok(Value::Str(env::consts::ARCH.to_string()))
}

fn sys_sleep(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("sys.sleep() expects 1 argument (milliseconds)".to_string());
    }
    match &args[0] {
        Value::Int(ms) if *ms >= 0 => {
            vm.sleep_bounded(*ms as u64)?;
            Ok(Value::Unit)
        }
        _ => Err("sys.sleep() expects a non-negative integer".to_string()),
    }
}
