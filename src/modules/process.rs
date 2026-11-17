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

#[cfg(windows)]
mod win_job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::ptr::null;

    type Handle = *mut c_void;

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimitInformation {
        basic_limit_information: BasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(lpJobAttributes: *const c_void, lpName: *const u16) -> Handle;
        fn SetInformationJobObject(
            hJob: Handle,
            jobObjectInformationClass: i32,
            lpJobObjectInformation: *const c_void,
            cbJobObjectInformationLength: u32,
        ) -> i32;
        fn AssignProcessToJobObject(hJob: Handle, hProcess: Handle) -> i32;
        fn TerminateJobObject(hJob: Handle, uExitCode: u32) -> i32;
        fn CloseHandle(hObject: Handle) -> i32;
    }

    pub struct ProcessJob {
        handle: Handle,
    }

    impl ProcessJob {
        pub fn assign(child: &Child) -> Result<Self, String> {
            let handle = unsafe { CreateJobObjectW(null(), null()) };
            if handle.is_null() {
                return Err("CreateJobObjectW failed".to_string());
            }
            let job = Self { handle };
            let mut info = ExtendedLimitInformation::default();
            info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set_ok = unsafe {
                SetInformationJobObject(
                    job.handle,
                    JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                    (&info as *const ExtendedLimitInformation).cast::<c_void>(),
                    std::mem::size_of::<ExtendedLimitInformation>() as u32,
                )
            };
            if set_ok == 0 {
                return Err("SetInformationJobObject failed".to_string());
            }
            let proc_handle = child.as_raw_handle() as Handle;
            if unsafe { AssignProcessToJobObject(job.handle, proc_handle) } == 0 {
                return Err("AssignProcessToJobObject failed".to_string());
            }
            Ok(job)
        }

        pub fn terminate(&self) {
            unsafe {
                TerminateJobObject(self.handle, 1);
            }
        }
    }

    impl Drop for ProcessJob {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }
}

#[cfg(not(windows))]
mod win_job {
    use std::process::Child;

    pub struct ProcessJob;

    impl ProcessJob {
        pub fn assign(_child: &Child) -> Result<Self, String> {
            Ok(Self)
        }

        pub fn terminate(&self) {}
    }
}

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
    vm.check_timeout()
        .map_err(|e| format!("process.run(): {}", e))?;

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
    let mut command = Command::new(cmd);
    command
        .args(&cmd_args)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in ["SystemRoot", "SYSTEMDRIVE", "windir", "PATH", "PATHEXT"] {
        if let Some(val) = std::env::var_os(key) {
            command.env(key, val);
        }
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("process.run('{}'): {}", cmd, e))?;
    let job = match win_job::ProcessJob::assign(&child) {
        Ok(job) => job,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("process.run('{}'): {}", cmd, e));
        }
    };
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
    let mut timeout_err: Option<String> = None;
    loop {
        if child
            .try_wait()
            .map_err(|e| format!("process.run(): {}", e))?
            .is_some()
        {
            break;
        }
        if let Err(err) = vm.check_timeout() {
            job.terminate();
            let _ = child.kill();
            timeout_err = Some(err);
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
    if let Some(err) = timeout_err {
        return Err(format!("process.run(): {}", err));
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
