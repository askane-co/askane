use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "time",
        &[
            ("now_ms", time_now_ms),
            ("now_sec", time_now_sec),
            ("elapsed_ms", time_elapsed_ms),
            ("sleep_ms", time_sleep_ms),
        ],
        &[],
    )
}

fn time_now_ms(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("time.now_ms() expects 0 arguments".to_string());
    }
    Ok(Value::Int(vm.now_unix_ms()?))
}

fn time_now_sec(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("time.now_sec() expects 0 arguments".to_string());
    }
    Ok(Value::Int(vm.now_unix_ms()? / 1000))
}

fn time_elapsed_ms(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("time.elapsed_ms() expects 0 arguments".to_string());
    }
    Ok(Value::Int(vm.elapsed_ms()))
}

fn time_sleep_ms(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("time.sleep_ms() expects 1 argument (milliseconds)".to_string());
    }
    match &args[0] {
        Value::Int(ms) if *ms >= 0 => {
            vm.sleep_bounded(*ms as u64)?;
            Ok(Value::Unit)
        }
        _ => Err("time.sleep_ms() expects a non-negative integer".to_string()),
    }
}
