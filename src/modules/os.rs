use std::cell::RefCell;
use std::env;
use std::process;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "os",
        &[
            ("platform", os_platform),
            ("arch", os_arch),
            ("family", os_family),
            ("cpu_count", os_cpu_count),
            ("pid", os_pid),
            ("sep", os_sep),
            ("line_ending", os_line_ending),
            ("cwd", os_cwd),
            ("home_dir", os_home_dir),
            ("temp_dir", os_temp_dir),
            ("env", os_env),
            ("args", os_args),
        ],
        &[],
    )
}

fn os_platform(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("os.platform() expects 0 arguments".to_string());
    }
    Ok(Value::from_str(env::consts::OS))
}

fn os_arch(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("os.arch() expects 0 arguments".to_string());
    }
    Ok(Value::from_str(env::consts::ARCH))
}

fn os_family(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("os.family() expects 0 arguments".to_string());
    }
    Ok(Value::from_str(env::consts::FAMILY))
}

fn os_cpu_count(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("os.cpu_count() expects 0 arguments".to_string());
    }
    let count = std::thread::available_parallelism()
        .map(|n| n.get() as i64)
        .unwrap_or(1);
    Ok(Value::Int(count))
}

fn os_pid(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("os.pid() expects 0 arguments".to_string());
    }
    Ok(Value::Int(process::id() as i64))
}

fn os_sep(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("os.sep() expects 0 arguments".to_string());
    }
    Ok(Value::from_str(std::path::MAIN_SEPARATOR.to_string()))
}

fn os_line_ending(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("os.line_ending() expects 0 arguments".to_string());
    }
    #[cfg(windows)]
    return Ok(Value::from_str("\r\n"));
    #[cfg(not(windows))]
    return Ok(Value::from_str("\n"));
}

fn os_cwd(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("os.cwd() expects 0 arguments".to_string());
    }
    if vm.is_sandboxed() {
        vm.resolve_read_path(".")?;
        return Ok(Value::from_str("."));
    }
    let cur = env::current_dir().map_err(|e| format!("os.cwd(): {}", e))?;
    Ok(Value::from_str(cur.display().to_string()))
}

fn os_home_dir(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("os.home_dir() expects 0 arguments".to_string());
    }
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    vm.check_env(key)?;
    Ok(env::var(key).map(Value::from_str).unwrap_or(Value::Unit))
}

fn os_temp_dir(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("os.temp_dir() expects 0 arguments".to_string());
    }
    if vm.is_sandboxed() {
        return Err(
            "os.temp_dir() is disabled in sandbox mode (use relative paths or --unsafe)"
                .to_string(),
        );
    }
    Ok(Value::from_str(env::temp_dir().display().to_string()))
}

fn os_env(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("os.env() expects 1 argument (name)".to_string());
    }
    let Value::Str(name) = &args[0] else {
        return Err("os.env() expects string name".to_string());
    };
    vm.check_env(name)?;
    Ok(env::var(name.as_str())
        .map(Value::from_str)
        .unwrap_or(Value::Unit))
}

fn os_args(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("os.args() expects 0 arguments".to_string());
    }
    vm.check_sys_args()?;
    let list: Vec<Value> = match vm.script_args_slice() {
        Some(args) => args.iter().cloned().map(Value::from_str).collect(),
        None => env::args().map(Value::from_str).collect(),
    };
    Ok(Value::List(Rc::new(RefCell::new(list))))
}
