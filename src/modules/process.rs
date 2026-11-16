use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::thread;
use std::time::Duration;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module("process", &[("run", proc_run)], &[])
}

fn proc_run(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("process.run() expects (cmd, [args_list])".to_string());
    }
    let cmd = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "process.run() command must be a string, got '{}'",
                other.type_name()
            ));
        }
    };
    if cmd.is_empty() || cmd.len() > 4096 || cmd.contains('\0') {
        return Err("process.run() command must be 1..4096 bytes and contain no nulls".to_string());
    }
    vm.check_exec(cmd)?;

    let mut cmd_args = Vec::new();
    if let Some(arg_list) = args.get(1) {
        match arg_list {
            Value::List(l) => {
                for item in l.borrow().iter() {
                    if cmd_args.len() >= 1024 {
                        return Err("process.run() accepts at most 1024 arguments".to_string());
                    }
                    cmd_args.push(format!("{}", item));
                }
            }
            other => {
                return Err(format!(
                    "process.run() args must be a list, got '{}'",
                    other.type_name()
                ));
            }
        }
    }

    const MAX_OUT: usize = 4 * 1024 * 1024;
    let mut child = Command::new(cmd)
        .args(&cmd_args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("process.run('{}'): {}", cmd, e))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "process.run(): stdout pipe unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "process.run(): stderr pipe unavailable".to_string())?;
    let out_thread = thread::spawn(move || {
        let mut b = Vec::new();
        stdout
            .take((MAX_OUT + 1) as u64)
            .read_to_end(&mut b)
            .map(|_| b)
    });
    let err_thread = thread::spawn(move || {
        let mut b = Vec::new();
        stderr
            .take((MAX_OUT + 1) as u64)
            .read_to_end(&mut b)
            .map(|_| b)
    });
    let mut timed_out = false;
    loop {
        if child
            .try_wait()
            .map_err(|e| format!("process.run(): {}", e))?
            .is_some()
        {
            break;
        }
        if vm.is_sandboxed() && vm.elapsed_ms() >= 30_000 {
            let _ = child.kill();
            timed_out = true;
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let status = child.wait().map_err(|e| format!("process.run(): {}", e))?;
    let stdout = out_thread
        .join()
        .map_err(|_| "process.run(): stdout reader failed".to_string())?
        .map_err(|e| format!("process.run(): {}", e))?;
    let stderr = err_thread
        .join()
        .map_err(|_| "process.run(): stderr reader failed".to_string())?
        .map_err(|e| format!("process.run(): {}", e))?;
    if timed_out {
        return Err("process.run(): execution timeout exceeded".to_string());
    }
    let stdout_bytes = &stdout[..stdout.len().min(MAX_OUT)];
    let stderr_bytes = &stderr[..stderr.len().min(MAX_OUT)];

    let mut res = HashMap::new();
    res.insert(
        "code".to_string(),
        Value::Int(status.code().unwrap_or(-1) as i64),
    );
    res.insert(
        "stdout".to_string(),
        Value::Str(String::from_utf8_lossy(stdout_bytes).into_owned()),
    );
    res.insert(
        "stderr".to_string(),
        Value::Str(String::from_utf8_lossy(stderr_bytes).into_owned()),
    );
    Ok(Value::Dict(Rc::new(RefCell::new(res))))
}
