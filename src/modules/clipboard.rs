use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "clipboard",
        &[("get_text", clip_get_text), ("set_text", clip_set_text)],
        &[],
    )
}

#[cfg(windows)]
mod win32_clip {
    use std::ffi::c_void;
    use std::ptr::null_mut;

    type Handle = *mut c_void;
    const CF_UNICODETEXT: u32 = 13;
    const GMEM_MOVEABLE: u32 = 0x0002;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn OpenClipboard(hWndNewOwner: *mut c_void) -> i32;
        fn CloseClipboard() -> i32;
        fn EmptyClipboard() -> i32;
        fn GetClipboardData(uFormat: u32) -> Handle;
        fn SetClipboardData(uFormat: u32, hMem: Handle) -> Handle;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalAlloc(uFlags: u32, dwBytes: usize) -> Handle;
        fn GlobalLock(hMem: Handle) -> *mut c_void;
        fn GlobalUnlock(hMem: Handle) -> i32;
        fn GlobalFree(hMem: Handle) -> Handle;
        fn GlobalSize(hMem: Handle) -> usize;
    }

    pub fn get_text() -> Result<String, String> {
        unsafe {
            if OpenClipboard(null_mut()) == 0 {
                return Err("Failed to open clipboard".to_string());
            }
            let h = GetClipboardData(CF_UNICODETEXT);
            if h.is_null() {
                CloseClipboard();
                return Ok(String::new());
            }
            let ptr = GlobalLock(h) as *const u16;
            if ptr.is_null() {
                CloseClipboard();
                return Err("Failed to lock clipboard data".to_string());
            }
            let bytes = GlobalSize(h);
            if bytes < 2 || bytes > 8 * 1024 * 1024 {
                GlobalUnlock(h);
                CloseClipboard();
                return Err("Clipboard text exceeds the 8 MiB safety limit".to_string());
            }
            let max_units = bytes / std::mem::size_of::<u16>();
            let mut len = 0usize;
            while len < max_units && *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            let text = String::from_utf16_lossy(slice);
            GlobalUnlock(h);
            CloseClipboard();
            Ok(text)
        }
    }

    pub fn set_text(text: &str) -> Result<(), String> {
        if text.len() > 8 * 1024 * 1024 {
            return Err("Clipboard text exceeds the 8 MiB safety limit".to_string());
        }
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = wide.len() * 2;
        unsafe {
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
            if h.is_null() {
                return Err("GlobalAlloc failed".to_string());
            }
            let ptr = GlobalLock(h) as *mut u16;
            if ptr.is_null() {
                GlobalFree(h);
                return Err("GlobalLock failed".to_string());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            GlobalUnlock(h);

            if OpenClipboard(null_mut()) == 0 {
                GlobalFree(h);
                return Err("Failed to open clipboard".to_string());
            }
            EmptyClipboard();
            if SetClipboardData(CF_UNICODETEXT, h).is_null() {
                CloseClipboard();
                GlobalFree(h);
                return Err("SetClipboardData failed".to_string());
            }
            CloseClipboard();
            Ok(())
        }
    }
}

fn clip_get_text(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_clipboard()?;
    if !args.is_empty() {
        return Err("clipboard.get_text() expects 0 arguments".to_string());
    }
    #[cfg(windows)]
    {
        Ok(Value::Str(win32_clip::get_text()?))
    }
    #[cfg(not(windows))]
    {
        Err("clipboard module is currently supported on Windows only".to_string())
    }
}

fn clip_set_text(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    vm.check_clipboard()?;
    if args.len() != 1 {
        return Err("clipboard.set_text() expects 1 string argument".to_string());
    }
    let text = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "clipboard.set_text() expects a string, got '{}'",
                other.type_name()
            ));
        }
    };
    #[cfg(windows)]
    {
        win32_clip::set_text(text)?;
        Ok(Value::Unit)
    }
    #[cfg(not(windows))]
    {
        let _ = text;
        Err("clipboard module is currently supported on Windows only".to_string())
    }
}
