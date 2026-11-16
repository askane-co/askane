#[cfg(windows)]
mod win32_gui {
    use std::ffi::c_void;
    use std::fs;
    use std::path::PathBuf;
    use std::ptr::{null, null_mut};
    use std::sync::{Arc, Mutex};

    type Hwnd = *mut c_void;
    type Hinstance = *mut c_void;
    type Hmenu = *mut c_void;
    type Hfont = *mut c_void;
    type Hbrush = *mut c_void;
    type Handle = *mut c_void;
    type Wparam = usize;
    type Lparam = isize;
    type Lresult = isize;

    const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const WS_CHILD: u32 = 0x4000_0000;
    const WS_BORDER: u32 = 0x0080_0000;
    const WS_VSCROLL: u32 = 0x0020_0000;
    const WS_HSCROLL: u32 = 0x0010_0000;
    const WS_TABSTOP: u32 = 0x0001_0000;
    const ES_MULTILINE: u32 = 0x0004;
    const ES_AUTOVSCROLL: u32 = 0x0040;
    const ES_AUTOHSCROLL: u32 = 0x0080;
    const ES_READONLY: u32 = 0x0800;
    const ES_WANTRETURN: u32 = 0x1000;
    const BS_PUSHBUTTON: u32 = 0x0000;
    const BS_AUTOCHECKBOX: u32 = 0x0003;
    const BM_GETCHECK: u32 = 0x00F0;
    const BST_CHECKED: isize = 1;

    const WM_CREATE: u32 = 0x0001;
    const WM_DESTROY: u32 = 0x0002;
    const WM_SIZE: u32 = 0x0005;
    const WM_SETFONT: u32 = 0x0030;
    const WM_COMMAND: u32 = 0x0111;
    const WM_APP_OUT: u32 = 0x8001;
    const WM_APP_DONE: u32 = 0x8002;

    const EM_SETLIMITTEXT: u32 = 0x00C5;
    const EM_SETSEL: u32 = 0x00B1;
    const EM_REPLACESEL: u32 = 0x00C2;

    const SW_SHOW: i32 = 5;
    const COLOR_WINDOW: usize = 5;
    const DEFAULT_CHARSET: u32 = 1;
    const FIXED_PITCH: u32 = 1;
    const FF_MODERN: u32 = 48;

    const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
    const OFN_PATHMUSTEXIST: u32 = 0x0000_0800;
    const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
    const OFN_OVERWRITEPROMPT: u32 = 0x0000_0002;

    const ID_BTN_OPEN: usize = 101;
    const ID_BTN_SAVE: usize = 102;
    const ID_BTN_RUN: usize = 103;
    const ID_BTN_STOP: usize = 104;
    const ID_CHK_UNSAFE: usize = 105;
    const ID_EDIT_FLAGS: usize = 106;
    const ID_EDIT_CODE: usize = 107;
    const ID_EDIT_OUT: usize = 108;
    const ID_BTN_CLEAR: usize = 109;
    const WM_KEYDOWN: u32 = 0x0100;
    const VK_CONTROL: i32 = 0x11;
    const VK_F5: usize = 0x74;

    #[repr(C)]
    struct WndClassExW {
        cb_size: u32,
        style: u32,
        lpfn_wnd_proc: unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult,
        cb_cls_extra: i32,
        cb_wnd_extra: i32,
        h_instance: Hinstance,
        h_icon: *mut c_void,
        h_cursor: *mut c_void,
        hbr_background: Hbrush,
        lpsz_menu_name: *const u16,
        lpsz_class_name: *const u16,
        h_icon_sm: *mut c_void,
    }

    #[repr(C)]
    struct Msg {
        hwnd: Hwnd,
        message: u32,
        w_param: Wparam,
        l_param: Lparam,
        time: u32,
        pt_x: i32,
        pt_y: i32,
        l_private: u32,
    }

    #[repr(C)]
    struct SecurityAttributes {
        n_length: u32,
        lp_security_descriptor: *mut c_void,
        b_inherit_handle: i32,
    }

    #[repr(C)]
    struct StartupInfoW {
        cb: u32,
        lp_reserved: *mut u16,
        lp_desktop: *mut u16,
        lp_title: *mut u16,
        dw_x: u32,
        dw_y: u32,
        dw_x_size: u32,
        dw_y_size: u32,
        dw_x_count_chars: u32,
        dw_y_count_chars: u32,
        dw_fill_attribute: u32,
        dw_flags: u32,
        w_show_window: u16,
        cb_reserved2: u16,
        lp_reserved2: *mut u8,
        h_std_input: Handle,
        h_std_output: Handle,
        h_std_error: Handle,
    }

    #[repr(C)]
    struct ProcessInformation {
        h_process: Handle,
        h_thread: Handle,
        dw_process_id: u32,
        dw_thread_id: u32,
    }

    #[repr(C)]
    struct OpenFileNameW {
        l_struct_size: u32,
        hwnd_owner: Hwnd,
        h_instance: Hinstance,
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
        fn RegisterClassExW(lpwcx: *const WndClassExW) -> u16;
        fn CreateWindowExW(
            dwExStyle: u32,
            lpClassName: *const u16,
            lpWindowName: *const u16,
            dwStyle: u32,
            x: i32,
            y: i32,
            nWidth: i32,
            nHeight: i32,
            hWndParent: Hwnd,
            hMenu: Hmenu,
            hInstance: Hinstance,
            lpParam: *mut c_void,
        ) -> Hwnd;
        fn ShowWindow(hwnd: Hwnd, nCmdShow: i32) -> i32;
        fn UpdateWindow(hwnd: Hwnd) -> i32;
        fn GetMessageW(lpMsg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
        fn TranslateMessage(lpMsg: *const Msg) -> i32;
        fn DispatchMessageW(lpMsg: *const Msg) -> Lresult;
        fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: Wparam, lparam: Lparam) -> Lresult;
        fn PostQuitMessage(nExitCode: i32);
        fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: Wparam, lparam: Lparam) -> i32;
        fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: Wparam, lparam: Lparam) -> Lresult;
        fn MoveWindow(hwnd: Hwnd, x: i32, y: i32, w: i32, h: i32, repaint: i32) -> i32;
        fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
        fn GetWindowTextW(hwnd: Hwnd, lpString: *mut u16, nMaxCount: i32) -> i32;
        fn SetWindowTextW(hwnd: Hwnd, lpString: *const u16) -> i32;
        fn LoadCursorW(hInstance: Hinstance, lpCursorName: *const u16) -> *mut c_void;
        fn GetWindowLongPtrW(hwnd: Hwnd, nIndex: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: Hwnd, nIndex: i32, dwNewLong: isize) -> isize;
        fn GetKeyState(nVirtKey: i32) -> i16;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn CreateFontW(
            cHeight: i32,
            cWidth: i32,
            cEscapement: i32,
            cOrientation: i32,
            cWeight: i32,
            bItalic: u32,
            bUnderline: u32,
            bStrikeOut: u32,
            iCharSet: u32,
            iOutPrecision: u32,
            iClipPrecision: u32,
            iQuality: u32,
            iPitchAndFamily: u32,
            pszFaceName: *const u16,
        ) -> Hfont;
        fn DeleteObject(ho: *mut c_void) -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(lpModuleName: *const u16) -> Hinstance;
        fn CreatePipe(
            hReadPipe: *mut Handle,
            hWritePipe: *mut Handle,
            lpPipeAttributes: *const SecurityAttributes,
            nSize: u32,
        ) -> i32;
        fn SetHandleInformation(hObject: Handle, dwMask: u32, dwFlags: u32) -> i32;
        fn CreateProcessW(
            lpApplicationName: *const u16,
            lpCommandLine: *mut u16,
            lpProcessAttributes: *const SecurityAttributes,
            lpThreadAttributes: *const SecurityAttributes,
            bInheritHandles: i32,
            dwCreationFlags: u32,
            lpEnvironment: *mut c_void,
            lpCurrentDirectory: *const u16,
            lpStartupInfo: *const StartupInfoW,
            lpProcessInformation: *mut ProcessInformation,
        ) -> i32;
        fn ReadFile(
            hFile: Handle,
            lpBuffer: *mut u8,
            nNumberOfBytesToRead: u32,
            lpNumberOfBytesRead: *mut u32,
            lpOverlapped: *mut c_void,
        ) -> i32;
        fn TerminateProcess(hProcess: Handle, uExitCode: u32) -> i32;
        fn CreateJobObjectW(
            lpJobAttributes: *const SecurityAttributes,
            lpName: *const u16,
        ) -> Handle;
        fn AssignProcessToJobObject(hJob: Handle, hProcess: Handle) -> i32;
        fn TerminateJobObject(hJob: Handle, uExitCode: u32) -> i32;
        fn WaitForSingleObject(hHandle: Handle, dwMilliseconds: u32) -> u32;
        fn GetExitCodeProcess(hProcess: Handle, lpExitCode: *mut u32) -> i32;
        fn CloseHandle(hObject: Handle) -> i32;
    }

    #[link(name = "comdlg32")]
    unsafe extern "system" {
        fn GetOpenFileNameW(lpofn: *mut OpenFileNameW) -> i32;
        fn GetSaveFileNameW(lpofn: *mut OpenFileNameW) -> i32;
    }

    const GWLP_USERDATA: i32 = -21;

    struct GuiState {
        hwnd_code: Hwnd,
        hwnd_out: Hwnd,
        hwnd_chk_unsafe: Hwnd,
        hwnd_flags: Hwnd,
        mono_font: Hfont,
        ui_font: Hfont,
        current_file: Option<PathBuf>,
        child_proc: Arc<Mutex<Option<isize>>>,
        child_job: Arc<Mutex<Option<isize>>>,
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn get_text(hwnd: Hwnd) -> String {
        let len = unsafe { GetWindowTextLengthW(hwnd) };
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let copied = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), len + 1) };
        String::from_utf16_lossy(&buf[..copied.max(0) as usize])
    }

    fn set_text(hwnd: Hwnd, s: &str) {
        let normalized = s.replace("\r\n", "\n").replace('\n', "\r\n");
        let wide = to_wide(&normalized);
        unsafe {
            SetWindowTextW(hwnd, wide.as_ptr());
        }
    }

    fn append_out(hwnd_out: Hwnd, s: &str) {
        let normalized = s.replace("\r\n", "\n").replace('\n', "\r\n");
        let wide = to_wide(&normalized);
        unsafe {
            let len = GetWindowTextLengthW(hwnd_out);
            SendMessageW(hwnd_out, EM_SETSEL, len as usize, len as isize);
            SendMessageW(hwnd_out, EM_REPLACESEL, 0, wide.as_ptr() as isize);
        }
    }

    fn stop_child(state: &GuiState) {
        {
            let mut job_guard = state.child_job.lock().unwrap();
            if let Some(raw_job) = job_guard.take() {
                let h_job = raw_job as Handle;
                unsafe {
                    TerminateJobObject(h_job, 1);
                    CloseHandle(h_job);
                }
            }
        }
        let mut guard = state.child_proc.lock().unwrap();
        if let Some(raw) = guard.take() {
            let h = raw as Handle;
            unsafe {
                TerminateProcess(h, 1);
                CloseHandle(h);
            }
        }
    }

    fn quote_windows_arg(arg: &str) -> String {
        let mut out = String::from("\"");
        let mut slashes = 0usize;
        for ch in arg.chars() {
            if ch == '\\' {
                slashes += 1;
            } else if ch == '"' {
                out.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            } else {
                out.extend(std::iter::repeat_n('\\', slashes));
                slashes = 0;
                out.push(ch);
            }
        }
        out.extend(std::iter::repeat_n('\\', slashes * 2));
        out.push('"');
        out
    }

    fn gui_flag_allowed(flag: &str) -> bool {
        !flag.is_empty()
            && !flag.chars().any(|c| {
                c == '"' || c == '\r' || c == '\n' || c == '&' || c == '|' || c == '<' || c == '>'
            })
            && (flag == "--allow-args"
                || flag == "--allow-clipboard"
                || flag == "--unsafe"
                || flag.starts_with("--allow-net=")
                || flag.starts_with("--allow-read=")
                || flag.starts_with("--allow-write=")
                || flag.starts_with("--allow-env=")
                || flag.starts_with("--max-steps=")
                || flag.starts_with("--timeout="))
    }

    fn run_script(hwnd: Hwnd, state: &GuiState) {
        stop_child(state);
        set_text(state.hwnd_out, "");

        let code = get_text(state.hwnd_code).replace("\r\n", "\n");
        let temp_path = std::env::temp_dir().join(format!(
            "askane_studio_{}_{}.ask",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));

        use std::fs::OpenOptions;
        use std::io::Write;
        let write_result = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .and_then(|mut f| f.write_all(code.as_bytes()));
        if let Err(e) = write_result {
            append_out(
                state.hwnd_out,
                &format!("Failed to write temp file: {}\n", e),
            );
            return;
        }

        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                append_out(
                    state.hwnd_out,
                    &format!("Failed to locate askane.exe: {}\n", e),
                );
                return;
            }
        };

        let is_unsafe =
            unsafe { SendMessageW(state.hwnd_chk_unsafe, BM_GETCHECK, 0, 0) } == BST_CHECKED;
        let extra_flags = get_text(state.hwnd_flags);

        let mut cmd = quote_windows_arg(&exe.to_string_lossy());
        if is_unsafe {
            cmd.push_str(" --unsafe");
        }
        for flag in extra_flags.split_whitespace() {
            if !gui_flag_allowed(flag) {
                append_out(
                    state.hwnd_out,
                    &format!("Rejected unsupported or unsafe flag: {}\n", flag),
                );
                let _ = fs::remove_file(&temp_path);
                return;
            }
            cmd.push(' ');
            cmd.push_str(&quote_windows_arg(flag));
        }
        cmd.push(' ');
        cmd.push_str(&quote_windows_arg(&temp_path.to_string_lossy()));

        let sa = SecurityAttributes {
            n_length: std::mem::size_of::<SecurityAttributes>() as u32,
            lp_security_descriptor: null_mut(),
            b_inherit_handle: 1,
        };
        let mut h_read: Handle = null_mut();
        let mut h_write: Handle = null_mut();
        if unsafe { CreatePipe(&mut h_read, &mut h_write, &sa, 0) } == 0 {
            append_out(state.hwnd_out, "Failed to create output pipe.\n");
            return;
        }
        unsafe {
            SetHandleInformation(h_read, HANDLE_FLAG_INHERIT, 0);
        }

        let si = StartupInfoW {
            cb: std::mem::size_of::<StartupInfoW>() as u32,
            lp_reserved: null_mut(),
            lp_desktop: null_mut(),
            lp_title: null_mut(),
            dw_x: 0,
            dw_y: 0,
            dw_x_size: 0,
            dw_y_size: 0,
            dw_x_count_chars: 0,
            dw_y_count_chars: 0,
            dw_fill_attribute: 0,
            dw_flags: STARTF_USESTDHANDLES,
            w_show_window: 0,
            cb_reserved2: 0,
            lp_reserved2: null_mut(),
            h_std_input: null_mut(),
            h_std_output: h_write,
            h_std_error: h_write,
        };
        let mut pi = ProcessInformation {
            h_process: null_mut(),
            h_thread: null_mut(),
            dw_process_id: 0,
            dw_thread_id: 0,
        };

        let mut wide_cmd = to_wide(&cmd);
        let ok = unsafe {
            CreateProcessW(
                null(),
                wide_cmd.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_NO_WINDOW,
                null_mut(),
                null(),
                &si,
                &mut pi,
            )
        };
        unsafe {
            CloseHandle(h_write);
        }
        if ok == 0 {
            unsafe { CloseHandle(h_read) };
            append_out(state.hwnd_out, "Failed to start askane child process.\n");
            return;
        }

        unsafe { CloseHandle(pi.h_thread) };
        let h_job = unsafe { CreateJobObjectW(null(), null()) };
        if !h_job.is_null() {
            if unsafe { AssignProcessToJobObject(h_job, pi.h_process) } == 0 {
                unsafe {
                    TerminateProcess(pi.h_process, 1);
                    CloseHandle(h_job);
                    CloseHandle(pi.h_process);
                    CloseHandle(h_read);
                }
                append_out(
                    state.hwnd_out,
                    "Failed to isolate Askane child process in a Job Object.\n",
                );
                return;
            }
            *state.child_job.lock().unwrap() = Some(h_job as isize);
        } else {
            unsafe {
                TerminateProcess(pi.h_process, 1);
                CloseHandle(pi.h_process);
                CloseHandle(h_read);
            }
            append_out(
                state.hwnd_out,
                "Failed to create Job Object; child process was not started.\n",
            );
            return;
        }
        let h_proc_raw = pi.h_process as isize;
        *state.child_proc.lock().unwrap() = Some(h_proc_raw);

        let hwnd_raw = hwnd as isize;
        let h_read_raw = h_read as isize;
        let child_arc = Arc::clone(&state.child_proc);
        let job_arc = Arc::clone(&state.child_job);
        let cleanup_tmp = Some(temp_path);

        std::thread::spawn(move || {
            let h_read = h_read_raw as Handle;
            let hwnd = hwnd_raw as Hwnd;
            let mut buf = [0u8; 4096];
            loop {
                let mut read_bytes: u32 = 0;
                let res = unsafe {
                    ReadFile(
                        h_read,
                        buf.as_mut_ptr(),
                        buf.len() as u32,
                        &mut read_bytes,
                        null_mut(),
                    )
                };
                if res == 0 || read_bytes == 0 {
                    break;
                }
                let text = String::from_utf8_lossy(&buf[..read_bytes as usize]).into_owned();
                let boxed = Box::into_raw(Box::new(text)) as Lparam;
                unsafe {
                    PostMessageW(hwnd, WM_APP_OUT, 0, boxed);
                }
            }
            unsafe { CloseHandle(h_read) };

            let h_proc = h_proc_raw as Handle;
            let mut exit_code: u32 = 0;
            unsafe {
                WaitForSingleObject(h_proc, 1000);
                GetExitCodeProcess(h_proc, &mut exit_code);
            }
            {
                let mut guard = child_arc.lock().unwrap();
                if *guard == Some(h_proc_raw) {
                    guard.take();
                    unsafe { CloseHandle(h_proc) };
                }
            }
            {
                let mut jguard = job_arc.lock().unwrap();
                if let Some(jraw) = jguard.take() {
                    unsafe { CloseHandle(jraw as Handle) };
                }
            }
            if let Some(p) = cleanup_tmp {
                let _ = fs::remove_file(p);
            }
            unsafe {
                PostMessageW(hwnd, WM_APP_DONE, exit_code as Wparam, 0);
            }
        });
    }

    fn open_file_dialog(hwnd: Hwnd) -> Option<PathBuf> {
        let mut buf = vec![0u16; 1024];
        let filter = to_wide("Askane Scripts (*.ask)\0*.ask\0All Files (*.*)\0*.*\0");
        let def_ext = to_wide("ask");
        let mut ofn = OpenFileNameW {
            l_struct_size: std::mem::size_of::<OpenFileNameW>() as u32,
            hwnd_owner: hwnd,
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
            flags: OFN_PATHMUSTEXIST | OFN_FILEMUSTEXIST,
            n_file_offset: 0,
            n_file_extension: 0,
            lpstr_def_ext: def_ext.as_ptr(),
            l_cust_data: 0,
            lpfn_hook: null(),
            lp_template_name: null(),
            pv_reserved: null_mut(),
            dw_reserved: 0,
            flags_ex: 0,
        };
        if unsafe { GetOpenFileNameW(&mut ofn) } != 0 {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(PathBuf::from(String::from_utf16_lossy(&buf[..end])))
        } else {
            None
        }
    }

    fn save_file_dialog(hwnd: Hwnd) -> Option<PathBuf> {
        let mut buf = vec![0u16; 1024];
        let filter = to_wide("Askane Scripts (*.ask)\0*.ask\0All Files (*.*)\0*.*\0");
        let def_ext = to_wide("ask");
        let mut ofn = OpenFileNameW {
            l_struct_size: std::mem::size_of::<OpenFileNameW>() as u32,
            hwnd_owner: hwnd,
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
            flags: OFN_OVERWRITEPROMPT,
            n_file_offset: 0,
            n_file_extension: 0,
            lpstr_def_ext: def_ext.as_ptr(),
            l_cust_data: 0,
            lpfn_hook: null(),
            lp_template_name: null(),
            pv_reserved: null_mut(),
            dw_reserved: 0,
            flags_ex: 0,
        };
        if unsafe { GetSaveFileNameW(&mut ofn) } != 0 {
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(PathBuf::from(String::from_utf16_lossy(&buf[..end])))
        } else {
            None
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: Hwnd,
        msg: u32,
        wparam: Wparam,
        lparam: Lparam,
    ) -> Lresult {
        let state_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut GuiState;
        match msg {
            WM_CREATE => 0,
            WM_SIZE => {
                if !state_ptr.is_null() {
                    let state = unsafe { &*state_ptr };
                    let w = (lparam & 0xFFFF) as i32;
                    let h = ((lparam >> 16) & 0xFFFF) as i32;
                    let top_h = 44;
                    let out_h = ((h - top_h) / 3).max(120);
                    let code_h = (h - top_h - out_h - 16).max(120);
                    unsafe {
                        MoveWindow(state.hwnd_code, 8, top_h, (w - 16).max(100), code_h, 1);
                        MoveWindow(
                            state.hwnd_out,
                            8,
                            top_h + code_h + 8,
                            (w - 16).max(100),
                            out_h,
                            1,
                        );
                    }
                }
                0
            }
            WM_COMMAND => {
                if !state_ptr.is_null() {
                    let state = unsafe { &mut *state_ptr };
                    let id = wparam & 0xFFFF;
                    match id {
                        ID_BTN_OPEN => {
                            if let Some(p) = open_file_dialog(hwnd) {
                                match crate::read_text_limited(&p, crate::MAX_SOURCE_BYTES) {
                                    Ok(content) => {
                                        set_text(state.hwnd_code, &content);
                                        state.current_file = Some(p);
                                    }
                                    Err(e) => {
                                        set_text(state.hwnd_out, &format!("Open failed: {}\n", e))
                                    }
                                }
                            }
                        }
                        ID_BTN_SAVE => {
                            let target = state
                                .current_file
                                .clone()
                                .or_else(|| save_file_dialog(hwnd));
                            if let Some(p) = target {
                                let content = get_text(state.hwnd_code).replace("\r\n", "\n");
                                match fs::write(&p, content) {
                                    Ok(_) => {
                                        state.current_file = Some(p.clone());
                                        set_text(
                                            state.hwnd_out,
                                            &format!("Saved {}\n", p.display()),
                                        );
                                    }
                                    Err(e) => {
                                        set_text(state.hwnd_out, &format!("Save failed: {}\n", e))
                                    }
                                }
                            }
                        }
                        ID_BTN_RUN => run_script(hwnd, state),
                        ID_BTN_STOP => {
                            stop_child(state);
                            append_out(state.hwnd_out, "\n[Stopped by user]\n");
                        }
                        ID_BTN_CLEAR => set_text(state.hwnd_out, ""),
                        _ => {}
                    }
                }
                0
            }
            WM_APP_OUT => {
                let text_box = unsafe { Box::from_raw(lparam as *mut String) };
                if !state_ptr.is_null() {
                    let state = unsafe { &*state_ptr };
                    append_out(state.hwnd_out, &text_box);
                }
                0
            }
            WM_APP_DONE => {
                if !state_ptr.is_null() {
                    let state = unsafe { &*state_ptr };
                    append_out(
                        state.hwnd_out,
                        &format!("\n[Exited with code {}]\n", wparam),
                    );
                }
                0
            }
            WM_DESTROY => {
                if !state_ptr.is_null() {
                    let state = unsafe { Box::from_raw(state_ptr) };
                    stop_child(&state);
                    unsafe {
                        DeleteObject(state.mono_font);
                        DeleteObject(state.ui_font);
                        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    }
                }
                unsafe { PostQuitMessage(0) };
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    pub fn launch_gui(initial_file: Option<&str>) -> Result<(), String> {
        let h_inst = unsafe { GetModuleHandleW(null()) };
        let class_name = to_wide("AskaneStudioWindow");
        let title = to_wide(&format!("Askane Studio v{}", crate::VERSION));
        let wc = WndClassExW {
            cb_size: std::mem::size_of::<WndClassExW>() as u32,
            style: 0,
            lpfn_wnd_proc: wnd_proc,
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: h_inst,
            h_icon: null_mut(),
            h_cursor: unsafe { LoadCursorW(null_mut(), 32512usize as *const u16) },
            hbr_background: (COLOR_WINDOW + 1) as Hbrush,
            lpsz_menu_name: null(),
            lpsz_class_name: class_name.as_ptr(),
            h_icon_sm: null_mut(),
        };
        if unsafe { RegisterClassExW(&wc) } == 0 {
            return Err("Failed to register GUI window class".to_string());
        }

        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                100,
                100,
                840,
                640,
                null_mut(),
                null_mut(),
                h_inst,
                null_mut(),
            )
        };
        if hwnd.is_null() {
            return Err("Failed to create GUI window".to_string());
        }

        let consolas = to_wide("Consolas");
        let segoe = to_wide("Segoe UI");
        let mono_font = unsafe {
            CreateFontW(
                16,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                0,
                0,
                0,
                FIXED_PITCH | FF_MODERN,
                consolas.as_ptr(),
            )
        };
        let ui_font = unsafe {
            CreateFontW(
                15,
                0,
                0,
                0,
                400,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                0,
                0,
                0,
                0,
                segoe.as_ptr(),
            )
        };

        let btn_cls = to_wide("BUTTON");
        let edit_cls = to_wide("EDIT");

        let mk_btn = |text: &str, x: i32, w: i32, id: usize, style: u32| -> Hwnd {
            let wt = to_wide(text);
            let h = unsafe {
                CreateWindowExW(
                    0,
                    btn_cls.as_ptr(),
                    wt.as_ptr(),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP | style,
                    x,
                    8,
                    w,
                    28,
                    hwnd,
                    id as Hmenu,
                    h_inst,
                    null_mut(),
                )
            };
            unsafe { SendMessageW(h, WM_SETFONT, ui_font as Wparam, 1) };
            h
        };

        mk_btn("Open", 8, 68, ID_BTN_OPEN, BS_PUSHBUTTON);
        mk_btn("Save", 82, 68, ID_BTN_SAVE, BS_PUSHBUTTON);
        mk_btn("Run", 156, 68, ID_BTN_RUN, BS_PUSHBUTTON);
        mk_btn("Stop", 230, 68, ID_BTN_STOP, BS_PUSHBUTTON);
        mk_btn("Clear", 304, 68, ID_BTN_CLEAR, BS_PUSHBUTTON);
        let hwnd_chk_unsafe = mk_btn("Unsafe", 380, 74, ID_CHK_UNSAFE, BS_AUTOCHECKBOX);

        let flags_hint = to_wide("");
        let hwnd_flags = unsafe {
            CreateWindowExW(
                0,
                edit_cls.as_ptr(),
                flags_hint.as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_BORDER | WS_TABSTOP | ES_AUTOHSCROLL,
                460,
                10,
                356,
                24,
                hwnd,
                ID_EDIT_FLAGS as Hmenu,
                h_inst,
                null_mut(),
            )
        };
        unsafe { SendMessageW(hwnd_flags, WM_SETFONT, ui_font as Wparam, 1) };

        let empty = to_wide("");
        let hwnd_code = unsafe {
            CreateWindowExW(
                0,
                edit_cls.as_ptr(),
                empty.as_ptr(),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_BORDER
                    | WS_VSCROLL
                    | WS_HSCROLL
                    | WS_TABSTOP
                    | ES_MULTILINE
                    | ES_AUTOVSCROLL
                    | ES_AUTOHSCROLL
                    | ES_WANTRETURN,
                8,
                44,
                808,
                360,
                hwnd,
                ID_EDIT_CODE as Hmenu,
                h_inst,
                null_mut(),
            )
        };
        unsafe {
            SendMessageW(hwnd_code, WM_SETFONT, mono_font as Wparam, 1);
            SendMessageW(hwnd_code, EM_SETLIMITTEXT, 1_048_576, 0);
        }

        let hwnd_out = unsafe {
            CreateWindowExW(
                0,
                edit_cls.as_ptr(),
                empty.as_ptr(),
                WS_CHILD
                    | WS_VISIBLE
                    | WS_BORDER
                    | WS_VSCROLL
                    | WS_HSCROLL
                    | ES_MULTILINE
                    | ES_AUTOVSCROLL
                    | ES_AUTOHSCROLL
                    | ES_READONLY,
                8,
                412,
                808,
                180,
                hwnd,
                ID_EDIT_OUT as Hmenu,
                h_inst,
                null_mut(),
            )
        };
        unsafe {
            SendMessageW(hwnd_out, WM_SETFONT, mono_font as Wparam, 1);
            SendMessageW(hwnd_out, EM_SETLIMITTEXT, 1_048_576, 0);
        }

        let mut current_file = None;
        if let Some(path) = initial_file {
            let p = PathBuf::from(path);
            if let Ok(content) = fs::read_to_string(&p) {
                set_text(hwnd_code, &content);
                current_file = Some(p);
            }
        } else {
            set_text(
                hwnd_code,
                "// Paste or write Askane code here, then click Run\nprintln(\"Hello from Askane Studio!\");\n",
            );
        }

        let state = Box::new(GuiState {
            hwnd_code,
            hwnd_out,
            hwnd_chk_unsafe,
            hwnd_flags,
            mono_font,
            ui_font,
            current_file,
            child_proc: Arc::new(Mutex::new(None)),
            child_job: Arc::new(Mutex::new(None)),
        });
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);
        }

        let mut msg = Msg {
            hwnd: null_mut(),
            message: 0,
            w_param: 0,
            l_param: 0,
            time: 0,
            pt_x: 0,
            pt_y: 0,
            l_private: 0,
        };
        while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
            if msg.message == WM_KEYDOWN {
                let ctrl = unsafe { GetKeyState(VK_CONTROL) } < 0;
                if msg.w_param == VK_F5 {
                    unsafe { SendMessageW(hwnd, WM_COMMAND, ID_BTN_RUN, 0) };
                    continue;
                } else if ctrl && msg.w_param == 0x53 {
                    unsafe { SendMessageW(hwnd, WM_COMMAND, ID_BTN_SAVE, 0) };
                    continue;
                } else if ctrl && msg.w_param == 0x4F {
                    unsafe { SendMessageW(hwnd, WM_COMMAND, ID_BTN_OPEN, 0) };
                    continue;
                } else if ctrl && msg.w_param == 0x41 && msg.hwnd == hwnd_code {
                    unsafe { SendMessageW(hwnd_code, EM_SETSEL, 0, -1) };
                    continue;
                }
            }
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        Ok(())
    }
}

pub fn run(initial_file: Option<&str>) -> Result<(), String> {
    #[cfg(windows)]
    {
        win32_gui::launch_gui(initial_file)
    }
    #[cfg(not(windows))]
    {
        let _ = initial_file;
        Err("GUI studio is currently supported on Windows only".to_string())
    }
}
