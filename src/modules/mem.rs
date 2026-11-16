use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    if !cfg!(test)
        && std::env::var_os("ASKANE_ENABLE_UNSAFE_MEM").as_deref()
            != Some(std::ffi::OsStr::new("1"))
    {
        return build_module(
            "mem",
            &[
                ("current_pid", mem_disabled),
                ("find_pid", mem_disabled),
                ("open", mem_disabled),
                ("close", mem_disabled),
                ("module_base", mem_disabled),
                ("read_i32", mem_disabled),
                ("read_i64", mem_disabled),
                ("read_f32", mem_disabled),
                ("read_f64", mem_disabled),
                ("read_bytes", mem_disabled),
                ("write_i32", mem_disabled),
                ("write_i64", mem_disabled),
                ("write_f32", mem_disabled),
                ("write_f64", mem_disabled),
            ],
            &[],
        );
    }
    build_module(
        "mem",
        &[
            ("current_pid", mem_current_pid),
            ("find_pid", mem_find_pid),
            ("open", mem_open),
            ("close", mem_close),
            ("module_base", mem_module_base),
            ("read_i32", mem_read_i32),
            ("read_i64", mem_read_i64),
            ("read_f32", mem_read_f32),
            ("read_f64", mem_read_f64),
            ("read_bytes", mem_read_bytes),
            ("write_i32", mem_write_i32),
            ("write_i64", mem_write_i64),
            ("write_f32", mem_write_f32),
            ("write_f64", mem_write_f64),
        ],
        &[],
    )
}

fn mem_disabled(_vm: &mut VM, _args: &[Value]) -> Result<Value, String> {
    Err("mem module is disabled by default; set ASKANE_ENABLE_UNSAFE_MEM=1 only for trusted scripts".to_string())
}

#[cfg(windows)]
mod win32 {
    use std::ffi::c_void;

    pub type Handle = *mut c_void;
    pub const INVALID_HANDLE_VALUE: Handle = -1isize as Handle;
    pub const PROCESS_ACCESS: u32 = 0x0008 | 0x0010 | 0x0020 | 0x0400; // VM_OPERATION | VM_READ | VM_WRITE | QUERY_INFORMATION
    pub const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    pub const TH32CS_SNAPMODULE: u32 = 0x0000_0008;
    pub const TH32CS_SNAPMODULE32: u32 = 0x0000_0010;

    #[repr(C)]
    pub struct ProcessEntry32W {
        pub dw_size: u32,
        pub cnt_usage: u32,
        pub th32_process_id: u32,
        pub th32_default_heap_id: usize,
        pub th32_module_id: u32,
        pub cnt_threads: u32,
        pub th32_parent_process_id: u32,
        pub pc_pri_class_base: i32,
        pub dw_flags: u32,
        pub sz_exe_file: [u16; 260],
    }

    #[repr(C)]
    pub struct ModuleEntry32W {
        pub dw_size: u32,
        pub th32_module_id: u32,
        pub th32_process_id: u32,
        pub glbl_cnt_usage: u32,
        pub proc_cnt_usage: u32,
        pub mod_base_addr: *mut u8,
        pub mod_base_size: u32,
        pub h_module: Handle,
        pub sz_module: [u16; 256],
        pub sz_exe_path: [u16; 260],
    }

    unsafe extern "system" {
        pub fn GetCurrentProcessId() -> u32;
        pub fn OpenProcess(dwDesiredAccess: u32, bInheritHandle: i32, dwProcessId: u32) -> Handle;
        pub fn CloseHandle(hObject: Handle) -> i32;
        pub fn CreateToolhelp32Snapshot(dwFlags: u32, th32ProcessID: u32) -> Handle;
        pub fn Process32FirstW(hSnapshot: Handle, lppe: *mut ProcessEntry32W) -> i32;
        pub fn Process32NextW(hSnapshot: Handle, lppe: *mut ProcessEntry32W) -> i32;
        pub fn Module32FirstW(hSnapshot: Handle, lpme: *mut ModuleEntry32W) -> i32;
        pub fn Module32NextW(hSnapshot: Handle, lpme: *mut ModuleEntry32W) -> i32;
        pub fn ReadProcessMemory(
            hProcess: Handle,
            lpBaseAddress: *const c_void,
            lpBuffer: *mut c_void,
            nSize: usize,
            lpNumberOfBytesRead: *mut usize,
        ) -> i32;
        pub fn WriteProcessMemory(
            hProcess: Handle,
            lpBaseAddress: *mut c_void,
            lpBuffer: *const c_void,
            nSize: usize,
            lpNumberOfBytesWritten: *mut usize,
        ) -> i32;
    }

    pub fn utf16_to_string(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }
}

#[cfg(windows)]
fn mem_current_pid(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("mem.current_pid() expects 0 arguments".to_string());
    }
    let pid = unsafe { win32::GetCurrentProcessId() };
    Ok(Value::Int(pid as i64))
}

#[cfg(windows)]
fn mem_find_pid(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("mem.find_pid() expects 1 argument (process_name)".to_string());
    }
    let target = match &args[0] {
        Value::Str(s) => s.to_lowercase(),
        other => {
            return Err(format!(
                "mem.find_pid() expects a string, got '{}'",
                other.type_name()
            ));
        }
    };

    unsafe {
        let snap = win32::CreateToolhelp32Snapshot(win32::TH32CS_SNAPPROCESS, 0);
        if snap == win32::INVALID_HANDLE_VALUE || snap.is_null() {
            return Err("Failed to create process snapshot".to_string());
        }

        let mut entry: win32::ProcessEntry32W = std::mem::zeroed();
        entry.dw_size = std::mem::size_of::<win32::ProcessEntry32W>() as u32;

        let mut found = None;
        if win32::Process32FirstW(snap, &mut entry) != 0 {
            loop {
                let exe = win32::utf16_to_string(&entry.sz_exe_file).to_lowercase();
                if exe == target {
                    found = Some(entry.th32_process_id as i64);
                    break;
                }
                if win32::Process32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        win32::CloseHandle(snap);
        Ok(found.map(Value::Int).unwrap_or(Value::Unit))
    }
}

#[cfg(windows)]
fn mem_open(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("mem.open() expects 1 argument (pid)".to_string());
    }
    let pid = match &args[0] {
        Value::Int(p) if *p >= 0 => *p as u32,
        _ => return Err("mem.open() expects a non-negative integer PID".to_string()),
    };

    let handle = unsafe { win32::OpenProcess(win32::PROCESS_ACCESS, 0, pid) };
    if handle.is_null() {
        return Err(format!("Failed to open process {}", pid));
    }
    Ok(Value::Int(handle as isize as i64))
}

#[cfg(windows)]
fn mem_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("mem.close() expects 1 argument (handle)".to_string());
    }
    let h = match &args[0] {
        Value::Int(h) => *h,
        _ => return Err("mem.close() expects an integer handle".to_string()),
    };
    if h == 0 || h == -1 {
        return Err("mem.close() received invalid handle".to_string());
    }
    unsafe {
        win32::CloseHandle(h as isize as win32::Handle);
    }
    Ok(Value::Unit)
}

#[cfg(windows)]
fn mem_module_base(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("mem.module_base() expects 2 arguments (pid, module_name)".to_string());
    }
    let pid = match &args[0] {
        Value::Int(p) if *p >= 0 => *p as u32,
        _ => return Err("mem.module_base() expects a non-negative integer PID".to_string()),
    };
    let target = match &args[1] {
        Value::Str(s) => s.to_lowercase(),
        other => {
            return Err(format!(
                "mem.module_base() expects a string module name, got '{}'",
                other.type_name()
            ));
        }
    };

    unsafe {
        let snap = win32::CreateToolhelp32Snapshot(
            win32::TH32CS_SNAPMODULE | win32::TH32CS_SNAPMODULE32,
            pid,
        );
        if snap == win32::INVALID_HANDLE_VALUE || snap.is_null() {
            return Err(format!("Failed to snapshot modules for PID {}", pid));
        }

        let mut entry: win32::ModuleEntry32W = std::mem::zeroed();
        entry.dw_size = std::mem::size_of::<win32::ModuleEntry32W>() as u32;

        let mut found = None;
        if win32::Module32FirstW(snap, &mut entry) != 0 {
            loop {
                let name = win32::utf16_to_string(&entry.sz_module).to_lowercase();
                if name == target {
                    found = Some(entry.mod_base_addr as usize as i64);
                    break;
                }
                if win32::Module32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        win32::CloseHandle(snap);
        Ok(found.map(Value::Int).unwrap_or(Value::Unit))
    }
}

#[cfg(windows)]
fn read_raw<T: Copy>(handle: i64, addr: i64) -> Result<T, String> {
    if handle == 0 || handle == -1 {
        return Err("Invalid process handle".to_string());
    }
    if addr == 0 {
        return Err("Cannot read from null address (0x0)".to_string());
    }
    let mut buf = std::mem::MaybeUninit::<T>::uninit();
    let mut read = 0usize;
    let ok = unsafe {
        win32::ReadProcessMemory(
            handle as isize as win32::Handle,
            addr as usize as *const std::ffi::c_void,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            std::mem::size_of::<T>(),
            &mut read,
        )
    };
    if ok == 0 || read != std::mem::size_of::<T>() {
        return Err(format!("ReadProcessMemory failed at 0x{:X}", addr));
    }
    Ok(unsafe { buf.assume_init() })
}

#[cfg(windows)]
fn write_raw<T: Copy>(handle: i64, addr: i64, val: T) -> Result<(), String> {
    if handle == 0 || handle == -1 {
        return Err("Invalid process handle".to_string());
    }
    if addr == 0 {
        return Err("Cannot write to null address (0x0)".to_string());
    }
    let mut written = 0usize;
    let ok = unsafe {
        win32::WriteProcessMemory(
            handle as isize as win32::Handle,
            addr as usize as *mut std::ffi::c_void,
            (&val as *const T) as *const std::ffi::c_void,
            std::mem::size_of::<T>(),
            &mut written,
        )
    };
    if ok == 0 || written != std::mem::size_of::<T>() {
        return Err(format!("WriteProcessMemory failed at 0x{:X}", addr));
    }
    Ok(())
}

#[cfg(windows)]
fn parse_handle_addr(args: &[Value], fn_name: &str) -> Result<(i64, i64), String> {
    if args.len() != 2 {
        return Err(format!(
            "mem.{}() expects 2 arguments (handle, addr)",
            fn_name
        ));
    }
    match (&args[0], &args[1]) {
        (Value::Int(h), Value::Int(a)) => Ok((*h, *a)),
        _ => Err(format!("mem.{}() expects integer (handle, addr)", fn_name)),
    }
}

#[cfg(windows)]
fn mem_read_i32(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let (h, a) = parse_handle_addr(args, "read_i32")?;
    Ok(Value::Int(read_raw::<i32>(h, a)? as i64))
}

#[cfg(windows)]
fn mem_read_i64(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let (h, a) = parse_handle_addr(args, "read_i64")?;
    Ok(Value::Int(read_raw::<i64>(h, a)?))
}

#[cfg(windows)]
fn mem_read_f32(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let (h, a) = parse_handle_addr(args, "read_f32")?;
    Ok(Value::Float(read_raw::<f32>(h, a)? as f64))
}

#[cfg(windows)]
fn mem_read_f64(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    let (h, a) = parse_handle_addr(args, "read_f64")?;
    Ok(Value::Float(read_raw::<f64>(h, a)?))
}

#[cfg(windows)]
fn mem_read_bytes(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("mem.read_bytes() expects 3 arguments (handle, addr, size)".to_string());
    }
    let (h, a, size) = match (&args[0], &args[1], &args[2]) {
        (Value::Int(h), Value::Int(a), Value::Int(s)) if *s >= 0 && *s <= 64 * 1024 * 1024 => {
            (*h, *a, *s as usize)
        }
        _ => {
            return Err(
                "mem.read_bytes() expects (int handle, int addr, 0 <= int size <= 64MB)"
                    .to_string(),
            );
        }
    };
    if h == 0 || h == -1 {
        return Err("Invalid process handle".to_string());
    }
    if a == 0 {
        return Err("Cannot read from null address (0x0)".to_string());
    }
    let mut buf = vec![0u8; size];
    let mut read = 0usize;
    let ok = unsafe {
        win32::ReadProcessMemory(
            h as isize as win32::Handle,
            a as usize as *const std::ffi::c_void,
            buf.as_mut_ptr() as *mut std::ffi::c_void,
            size,
            &mut read,
        )
    };
    if ok == 0 {
        return Err(format!("ReadProcessMemory failed at 0x{:X}", a));
    }
    buf.truncate(read);
    let list = buf.into_iter().map(|b| Value::Int(b as i64)).collect();
    Ok(Value::List(std::rc::Rc::new(std::cell::RefCell::new(list))))
}

#[cfg(windows)]
fn mem_write_i32(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("mem.write_i32() expects 3 arguments (handle, addr, val)".to_string());
    }
    match (&args[0], &args[1], &args[2]) {
        (Value::Int(h), Value::Int(a), Value::Int(v)) => {
            write_raw::<i32>(*h, *a, *v as i32)?;
            Ok(Value::Unit)
        }
        _ => Err("mem.write_i32() expects (int handle, int addr, int val)".to_string()),
    }
}

#[cfg(windows)]
fn mem_write_i64(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("mem.write_i64() expects 3 arguments (handle, addr, val)".to_string());
    }
    match (&args[0], &args[1], &args[2]) {
        (Value::Int(h), Value::Int(a), Value::Int(v)) => {
            write_raw::<i64>(*h, *a, *v)?;
            Ok(Value::Unit)
        }
        _ => Err("mem.write_i64() expects (int handle, int addr, int val)".to_string()),
    }
}

#[cfg(windows)]
fn mem_write_f32(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("mem.write_f32() expects 3 arguments (handle, addr, val)".to_string());
    }
    let val = match &args[2] {
        Value::Float(f) => *f as f32,
        Value::Int(i) => *i as f32,
        _ => return Err("mem.write_f32() expects numeric val".to_string()),
    };
    match (&args[0], &args[1]) {
        (Value::Int(h), Value::Int(a)) => {
            write_raw::<f32>(*h, *a, val)?;
            Ok(Value::Unit)
        }
        _ => Err("mem.write_f32() expects (int handle, int addr, number val)".to_string()),
    }
}

#[cfg(windows)]
fn mem_write_f64(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("mem.write_f64() expects 3 arguments (handle, addr, val)".to_string());
    }
    let val = match &args[2] {
        Value::Float(f) => *f,
        Value::Int(i) => *i as f64,
        _ => return Err("mem.write_f64() expects numeric val".to_string()),
    };
    match (&args[0], &args[1]) {
        (Value::Int(h), Value::Int(a)) => {
            write_raw::<f64>(*h, *a, val)?;
            Ok(Value::Unit)
        }
        _ => Err("mem.write_f64() expects (int handle, int addr, number val)".to_string()),
    }
}

#[cfg(not(windows))]
fn unsupported(_vm: &mut VM, _args: &[Value]) -> Result<Value, String> {
    Err("mem module is only supported on Windows".to_string())
}

#[cfg(not(windows))]
use unsupported as mem_current_pid;
#[cfg(not(windows))]
use unsupported as mem_find_pid;
#[cfg(not(windows))]
use unsupported as mem_open;
#[cfg(not(windows))]
use unsupported as mem_close;
#[cfg(not(windows))]
use unsupported as mem_module_base;
#[cfg(not(windows))]
use unsupported as mem_read_i32;
#[cfg(not(windows))]
use unsupported as mem_read_i64;
#[cfg(not(windows))]
use unsupported as mem_read_f32;
#[cfg(not(windows))]
use unsupported as mem_read_f64;
#[cfg(not(windows))]
use unsupported as mem_read_bytes;
#[cfg(not(windows))]
use unsupported as mem_write_i32;
#[cfg(not(windows))]
use unsupported as mem_write_i64;
#[cfg(not(windows))]
use unsupported as mem_write_f32;
#[cfg(not(windows))]
use unsupported as mem_write_f64;
