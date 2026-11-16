use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "dialog",
        &[
            ("info", dlg_info),
            ("confirm", dlg_confirm),
            ("open_file", dlg_open_file),
            ("save_file", dlg_save_file),
        ],
        &[],
    )
}

#[cfg(windows)]
mod win32_dlg {
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    const MB_OK: u32 = 0x0000_0000;
    const MB_YESNO: u32 = 0x0000_0004;
    const MB_ICONINFORMATION: u32 = 0x0000_0040;
    const MB_ICONQUESTION: u32 = 0x0000_0020;
    const IDYES: i32 = 6;
    const OFN_PATHMUSTEXIST: u32 = 0x0000_0800;
    const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
    const OFN_OVERWRITEPROMPT: u32 = 0x0000_0002;

    #[repr(C)]
    struct OpenFileNameW {
        l_struct_size: u32,
        hwnd_owner: *mut c_void,
        h_instance: *mut c_void,
        lpstr_filter: *const u16,
        lpstr_custom_filter: *mut u16,
        n_max_custom_filter: u32,
        n_filter_index: u32,
        lpstr_file: *mut u16,
        n_max_file: u32,
        lpstr_file_title: *mut u16,
        n_max_file_title: u32,
        lpstr_initial_dir: *const u16,
        lpstr_title: *const u16,
        flags: u32,
        n_file_offset: u16,
        n_file_extension: u16,
        lpstr_def_ext: *const u16,
        l_cust_data: isize,
        lpfn_hook: *const c_void,
        lp_template_name: *const u16,
        pv_reserved: *mut c_void,
        dw_reserved: u32,
        flags_ex: u32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(
            hwnd: *mut c_void,
            lpText: *const u16,
            lpCaption: *const u16,
            uType: u32,
        ) -> i32;
    }

    #[link(name = "comdlg32")]
    unsafe extern "system" {
        fn GetOpenFileNameW(lpofn: *mut OpenFileNameW) -> i32;
        fn GetSaveFileNameW(lpofn: *mut OpenFileNameW) -> i32;
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn message_box(title: &str, text: &str, confirm: bool) -> bool {
        let w_title = to_wide(title);
        let w_text = to_wide(text);
        let flags = if confirm {
            MB_YESNO | MB_ICONQUESTION
        } else {
            MB_OK | MB_ICONINFORMATION
        };
        let res = unsafe { MessageBoxW(null_mut(), w_text.as_ptr(), w_title.as_ptr(), flags) };
        res == IDYES
    }

    pub fn file_dialog(save: bool) -> Option<String> {
        let mut buf = vec![0u16; 1024];
        let filter = to_wide("All Files (*.*)\0*.*\0Askane Scripts (*.ask)\0*.ask\0");
        let mut ofn = OpenFileNameW {
            l_struct_size: std::mem::size_of::<OpenFileNameW>() as u32,
            hwnd_owner: null_mut(),
            h_instance: null_mut(),
            lpstr_filter: filter.as_ptr(),
            lpstr_custom_filter: null_mut(),
            n_max_custom_filter: 0,
            n_filter_index: 1,
            lpstr_file: buf.as_mut_ptr(),
            n_max_file: buf.len() as u32,
            lpstr_file_title: null_mut(),
            n_max_file_title: 0,
            lpstr_initial_dir: null(),
            lpstr_title: null(),
            flags: if save {
                OFN_OVERWRITEPROMPT
            } else {
                OFN_PATHMUSTEXIST | OFN_FILEMUSTEXIST
            },
            n_file_offset: 0,
            n_file_extension: 0,
            lpstr_def_ext: null(),
            l_cust_data: 0,
            lpfn_hook: null(),
            lp_template_name: null(),
            pv_reserved: null_mut(),
            dw_reserved: 0,
            flags_ex: 0,
        };
        let ok = unsafe {
            if save {
                GetSaveFileNameW(&mut ofn)
            } else {
                GetOpenFileNameW(&mut ofn)
            }
        };
        if ok != 0 {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..end]))
        } else {
            None
        }
    }
}

fn dlg_info(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("dialog.info() expects 2 string arguments (title, message)".to_string());
    }
    #[cfg(windows)]
    {
        win32_dlg::message_box(&format!("{}", args[0]), &format!("{}", args[1]), false);
        Ok(Value::Unit)
    }
    #[cfg(not(windows))]
    {
        Err("dialog module is currently supported on Windows only".to_string())
    }
}

fn dlg_confirm(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("dialog.confirm() expects 2 string arguments (title, message)".to_string());
    }
    #[cfg(windows)]
    {
        Ok(Value::Bool(win32_dlg::message_box(
            &format!("{}", args[0]),
            &format!("{}", args[1]),
            true,
        )))
    }
    #[cfg(not(windows))]
    {
        Err("dialog module is currently supported on Windows only".to_string())
    }
}

fn dlg_open_file(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("dialog.open_file() expects 0 arguments".to_string());
    }
    #[cfg(windows)]
    {
        Ok(win32_dlg::file_dialog(false)
            .map(Value::Str)
            .unwrap_or(Value::Unit))
    }
    #[cfg(not(windows))]
    {
        Err("dialog module is currently supported on Windows only".to_string())
    }
}

fn dlg_save_file(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("dialog.save_file() expects 0 arguments".to_string());
    }
    #[cfg(windows)]
    {
        Ok(win32_dlg::file_dialog(true)
            .map(Value::Str)
            .unwrap_or(Value::Unit))
    }
    #[cfg(not(windows))]
    {
        Err("dialog module is currently supported on Windows only".to_string())
    }
}
