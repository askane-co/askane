use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectTreeItem {
    pub display: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PermissionFlagsState {
    pub unsafe_mode: bool,
    pub allow_read: bool,
    pub allow_write: bool,
    pub allow_net: bool,
    pub allow_env: bool,
    pub allow_exec: bool,
    pub allow_input: bool,
    pub allow_clipboard: bool,
}

pub(crate) fn find_project_root_for(file: &Path) -> PathBuf {
    let start = if file.is_dir() {
        file.to_path_buf()
    } else {
        file.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    };
    let mut cur = start.clone();
    for _ in 0..4 {
        if cur.join("askane.toml").is_file() {
            return cur;
        }
        let Some(parent) = cur.parent() else {
            break;
        };
        cur = parent.to_path_buf();
    }
    start
}

fn should_skip_tree_entry(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "target" | "node_modules" | "__snapshots__")
}

fn scan_tree_recursive(dir: &Path, depth: usize, limit: usize, out: &mut Vec<ProjectTreeItem>) {
    if depth > 4 || out.len() >= limit {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if should_skip_tree_entry(&name) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            dirs.push((name, path));
        } else {
            let is_relevant = name.eq_ignore_ascii_case("askane.toml")
                || path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    matches!(
                        e.to_ascii_lowercase().as_str(),
                        "ask" | "json" | "md" | "toml"
                    )
                });
            if is_relevant {
                files.push((name, path));
            }
        }
    }
    dirs.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));
    files.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));

    let indent = "  ".repeat(depth);
    for (name, path) in dirs {
        if out.len() >= limit {
            break;
        }
        out.push(ProjectTreeItem {
            display: format!("{}[{}]", indent, name),
            path: path.clone(),
            is_dir: true,
        });
        scan_tree_recursive(&path, depth + 1, limit, out);
    }
    for (name, path) in files {
        if out.len() >= limit {
            break;
        }
        out.push(ProjectTreeItem {
            display: format!("{}{}", indent, name),
            path,
            is_dir: false,
        });
    }
}

pub(crate) fn scan_project_tree(root: &Path, limit: usize) -> Vec<ProjectTreeItem> {
    let mut out = Vec::new();
    if root.is_dir() {
        scan_tree_recursive(root, 0, limit, &mut out);
    }
    out
}

pub(crate) fn parse_error_line_col(line: &str) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'[' {
            if let Some(close_rel) = line[i + 1..].find(']') {
                let inner = &line[i + 1..i + 1 + close_rel];
                if let Some((l_s, c_s)) = inner.split_once(':') {
                    if let (Ok(l), Ok(c)) =
                        (l_s.trim().parse::<usize>(), c_s.trim().parse::<usize>())
                    {
                        if l > 0 {
                            return Some((l, c.max(1)));
                        }
                    }
                }
            }
        }
    }
    for marker in ["(line ", "[line "] {
        if let Some(pos) = line.find(marker) {
            let rest = &line[pos + marker.len()..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(l) = digits.parse::<usize>() {
                if l > 0 {
                    return Some((l, 1));
                }
            }
        }
    }
    None
}

pub(crate) fn gui_flag_allowed(flag: &str) -> bool {
    !flag.is_empty()
        && !flag.chars().any(|c| {
            c == '"' || c == '\r' || c == '\n' || c == '&' || c == '|' || c == '<' || c == '>'
        })
        && (flag == "--allow-args"
            || flag == "--allow-clipboard"
            || flag == "--allow-exec"
            || flag == "--allow-input"
            || flag == "--allow-net"
            || flag == "--allow-read"
            || flag == "--allow-write"
            || flag == "--allow-env"
            || flag == "--unsafe"
            || flag.starts_with("--allow-net=")
            || flag.starts_with("--allow-read=")
            || flag.starts_with("--allow-write=")
            || flag.starts_with("--allow-env=")
            || flag.starts_with("--allow-exec=")
            || flag.starts_with("--max-steps=")
            || flag.starts_with("--max-mem=")
            || flag.starts_with("--timeout="))
}

pub(crate) fn compose_permission_flags(
    perms: &PermissionFlagsState,
    project_root: Option<&Path>,
    extra_flags: &str,
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    if perms.unsafe_mode {
        out.push("--unsafe".to_string());
    }
    let root_str = project_root
        .map(|p| p.to_string_lossy().to_string())
        .filter(|s| !s.is_empty());
    if perms.allow_read {
        if let Some(r) = &root_str {
            out.push(format!("--allow-read={}", r));
        } else {
            out.push("--allow-read".to_string());
        }
    }
    if perms.allow_write {
        if let Some(r) = &root_str {
            out.push(format!("--allow-write={}", r));
        } else {
            out.push("--allow-write".to_string());
        }
    }
    if perms.allow_net {
        out.push("--allow-net".to_string());
    }
    if perms.allow_env {
        out.push("--allow-env".to_string());
    }
    if perms.allow_exec {
        out.push("--allow-exec".to_string());
    }
    if perms.allow_input {
        out.push("--allow-input".to_string());
    }
    if perms.allow_clipboard {
        out.push("--allow-clipboard".to_string());
    }
    for flag in extra_flags.split_whitespace() {
        if !gui_flag_allowed(flag) {
            return Err(format!("Rejected unsupported or unsafe flag: {}", flag));
        }
        out.push(flag.to_string());
    }
    Ok(out)
}

#[cfg(windows)]
mod win32_gui {
    use super::{
        PermissionFlagsState, ProjectTreeItem, compose_permission_flags, find_project_root_for,
        parse_error_line_col, scan_project_tree,
    };
    use std::ffi::c_void;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::path::{Path, PathBuf};
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
    const WS_CAPTION: u32 = 0x00C0_0000;
    const WS_SYSMENU: u32 = 0x0008_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const WS_CHILD: u32 = 0x4000_0000;
    const WS_VSCROLL: u32 = 0x0020_0000;
    const WS_HSCROLL: u32 = 0x0010_0000;
    const WS_TABSTOP: u32 = 0x0001_0000;
    const WS_CLIPCHILDREN: u32 = 0x0200_0000;

    const ES_RIGHT: u32 = 0x0002;
    const ES_MULTILINE: u32 = 0x0004;
    const ES_AUTOVSCROLL: u32 = 0x0040;
    const ES_AUTOHSCROLL: u32 = 0x0080;
    const ES_READONLY: u32 = 0x0800;
    const ES_WANTRETURN: u32 = 0x1000;
    const BS_AUTOCHECKBOX: u32 = 0x0003;
    const BS_OWNERDRAW: u32 = 0x000B;
    const LBS_NOTIFY: u32 = 0x0001;
    const LBS_NOINTEGRALHEIGHT: u32 = 0x0100;

    const BM_GETCHECK: u32 = 0x00F0;
    const BM_SETCHECK: u32 = 0x00F1;
    const BST_CHECKED: isize = 1;

    const WM_CREATE: u32 = 0x0001;
    const WM_DESTROY: u32 = 0x0002;
    const WM_SIZE: u32 = 0x0005;
    const WM_PAINT: u32 = 0x000F;
    const WM_CLOSE: u32 = 0x0010;
    const WM_ERASEBKGND: u32 = 0x0014;
    const WM_DRAWITEM: u32 = 0x002B;
    const WM_SETFONT: u32 = 0x0030;
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_COMMAND: u32 = 0x0111;
    const WM_TIMER: u32 = 0x0113;
    const WM_CTLCOLOREDIT: u32 = 0x0133;
    const WM_CTLCOLORLISTBOX: u32 = 0x0134;
    const WM_CTLCOLORBTN: u32 = 0x0135;
    const WM_CTLCOLORSTATIC: u32 = 0x0138;
    const WM_LBUTTONDBLCLK: u32 = 0x0203;
    const WM_APP_OUT: u32 = 0x8001;
    const WM_APP_DONE: u32 = 0x8002;

    const EM_GETSEL: u32 = 0x00B0;
    const EM_SETSEL: u32 = 0x00B1;
    const EM_LINESCROLL: u32 = 0x00B6;
    const EM_SCROLLCARET: u32 = 0x00B7;
    const EM_GETLINECOUNT: u32 = 0x00BA;
    const EM_LINEINDEX: u32 = 0x00BB;
    const EM_REPLACESEL: u32 = 0x00C2;
    const EM_SETLIMITTEXT: u32 = 0x00C5;
    const EM_LINEFROMCHAR: u32 = 0x00C9;
    const EM_GETFIRSTVISIBLELINE: u32 = 0x00CE;
    const EN_CHANGE: usize = 0x0300;
    const EN_VSCROLL: usize = 0x0602;

    const LB_ADDSTRING: u32 = 0x0180;
    const LB_RESETCONTENT: u32 = 0x0184;
    const LB_SETCURSEL: u32 = 0x0186;
    const LB_GETCURSEL: u32 = 0x0188;
    const LBN_SELCHANGE: usize = 1;
    const LBN_DBLCLK: usize = 2;

    const SW_SHOW: i32 = 5;
    const SW_HIDE: i32 = 0;
    const COLOR_WINDOW: usize = 5;
    const DEFAULT_CHARSET: u32 = 1;
    const FIXED_PITCH: u32 = 1;
    const FF_MODERN: u32 = 48;
    const TRANSPARENT: i32 = 1;
    const DT_CENTER: u32 = 0x0001;
    const DT_VCENTER: u32 = 0x0004;
    const DT_SINGLELINE: u32 = 0x0020;
    const DT_END_ELLIPSIS: u32 = 0x0000_8000;
    const ODS_SELECTED: u32 = 0x0001;
    const RDW_INVALIDATE: u32 = 0x0001;
    const RDW_ERASE: u32 = 0x0004;
    const RDW_ALLCHILDREN: u32 = 0x0080;

    const STARTF_USESTDHANDLES: u32 = 0x0000_0100;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;
    const OFN_PATHMUSTEXIST: u32 = 0x0000_0800;
    const OFN_FILEMUSTEXIST: u32 = 0x0000_1000;
    const OFN_OVERWRITEPROMPT: u32 = 0x0000_0002;
    const BIF_RETURNONLYFSDIRS: u32 = 0x0001;
    const BIF_EDITBOX: u32 = 0x0010;
    const BIF_NEWDIALOGSTYLE: u32 = 0x0040;

    const MB_YESNOCANCEL: u32 = 0x0000_0003;
    const MB_ICONQUESTION: u32 = 0x0000_0020;
    const IDYES: i32 = 6;
    const IDCANCEL: i32 = 2;

    const GWLP_WNDPROC: i32 = -4;
    const GWLP_USERDATA: i32 = -21;
    const VK_CONTROL: i32 = 0x11;
    const VK_RETURN: usize = 0x0D;
    const VK_F3: usize = 0x72;
    const VK_F5: usize = 0x74;

    const MAX_TABS: usize = 10;
    const MAX_OUTPUT_CHARS: usize = 262_144;
    const ID_TIMER_SYNC: usize = 1;

    // BGR COLORREFs
    const CLR_DARK_WIN_BG: u32 = 0x0022_1B16; // #161B22
    const CLR_DARK_EDITOR_BG: u32 = 0x0017_110D; // #0D1117
    const CLR_DARK_GUTTER_BG: u32 = 0x001B_1511; // #11151B
    const CLR_DARK_BTN_BG: u32 = 0x002D_2621; // #21262D
    const CLR_DARK_BTN_SEL: u32 = 0x003D_3630; // #30363D
    const CLR_DARK_TAB_INACT: u32 = 0x001F_1914; // #14191F
    const CLR_DARK_TAB_ACT: u32 = 0x002E_241E; // #1E242E
    const CLR_DARK_BORDER: u32 = 0x003D_3630; // #30363D
    const CLR_DARK_TEXT: u32 = 0x00F3_EDE6; // #E6EDF3
    const CLR_DARK_MUTED: u32 = 0x009E_948B; // #8B949E
    const CLR_ACCENT_BLUE: u32 = 0x00F7_8F4F; // #4F8FF7
    const CLR_ACCENT_GREEN: u32 = 0x0036_8623; // #238636
    const CLR_ACCENT_RED: u32 = 0x0036_2E8E; // #8E2E36

    const CLR_LIGHT_WIN_BG: u32 = 0x00FA_F6F3;
    const CLR_LIGHT_EDITOR_BG: u32 = 0x00FF_FFFF;
    const CLR_LIGHT_GUTTER_BG: u32 = 0x00F4_EFEA;
    const CLR_LIGHT_BTN_BG: u32 = 0x00EB_E5DF;
    const CLR_LIGHT_BTN_SEL: u32 = 0x00DC_D4CC;
    const CLR_LIGHT_BORDER: u32 = 0x00D0_C7BE;
    const CLR_LIGHT_TEXT: u32 = 0x002A_201A;
    const CLR_LIGHT_MUTED: u32 = 0x0078_685A;

    const ID_BTN_NEW: usize = 100;
    const ID_BTN_OPEN: usize = 101;
    const ID_BTN_SAVE: usize = 102;
    const ID_BTN_RUN: usize = 103;
    const ID_BTN_STOP: usize = 104;
    const ID_CHK_UNSAFE: usize = 105;
    const ID_EDIT_FLAGS: usize = 106;
    const ID_EDIT_CODE: usize = 107;
    const ID_EDIT_OUT: usize = 108;
    const ID_BTN_CLEAR: usize = 109;
    const ID_BTN_CHECK: usize = 110;
    const ID_CHK_DARK: usize = 111;
    const ID_EDIT_IN: usize = 112;
    const ID_BTN_SEND: usize = 113;
    const ID_BTN_SETUP: usize = 114;
    const ID_BTN_FOLDER: usize = 115;
    const ID_BTN_EXAMPLES: usize = 116;
    const ID_BTN_FMT: usize = 117;
    const ID_BTN_TEST: usize = 118;
    const ID_BTN_FIND_TOGGLE: usize = 119;
    const ID_BTN_REFRESH_TREE: usize = 120;
    const ID_LIST_TREE: usize = 121;
    const ID_EDIT_GUTTER: usize = 122;
    const ID_LBL_STATUS: usize = 123;
    const ID_BTN_CLOSE_TAB: usize = 124;
    const ID_LBL_FLAGS: usize = 125;

    const ID_CHK_READ: usize = 130;
    const ID_CHK_WRITE: usize = 131;
    const ID_CHK_NET: usize = 132;
    const ID_CHK_ENV: usize = 133;
    const ID_CHK_EXEC: usize = 134;
    const ID_CHK_INPUT: usize = 135;
    const ID_CHK_CLIP: usize = 136;

    const ID_EDIT_FIND: usize = 140;
    const ID_EDIT_REPL: usize = 141;
    const ID_BTN_FIND_NEXT: usize = 142;
    const ID_BTN_REPL_ONE: usize = 143;
    const ID_BTN_REPL_ALL: usize = 144;
    const ID_BTN_FIND_CLOSE: usize = 145;

    const ID_TAB_BASE: usize = 200;

    const ID_SETUP_LBL_INFO: usize = 301;
    const ID_SETUP_ADD_PATH: usize = 302;
    const ID_SETUP_REM_PATH: usize = 303;
    const ID_SETUP_ASSOC_RUN: usize = 304;
    const ID_SETUP_ASSOC_STUDIO: usize = 305;
    const ID_SETUP_UNINSTALL: usize = 306;
    const ID_SETUP_CLOSE: usize = 307;

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
    struct PaintStruct {
        hdc: *mut c_void,
        f_erase: i32,
        rc_paint: [i32; 4],
        f_restore: i32,
        f_inc_update: i32,
        rgb_reserved: [u8; 32],
    }

    #[repr(C)]
    struct DrawItemStruct {
        ctl_type: u32,
        ctl_id: u32,
        item_id: u32,
        item_action: u32,
        item_state: u32,
        hwnd_item: Hwnd,
        hdc: *mut c_void,
        rc_item: [i32; 4],
        item_data: usize,
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
    struct JobExtendedLimitInformation {
        basic_limit_information: BasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
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

    #[repr(C)]
    struct BrowseInfoW {
        hwnd_owner: Hwnd,
        pidl_root: *const c_void,
        psz_display_name: *mut u16,
        lpsz_title: *const u16,
        ul_flags: u32,
        lpfn: *const c_void,
        l_param: isize,
        i_image: i32,
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
        fn DestroyWindow(hwnd: Hwnd) -> i32;
        fn EnableWindow(hwnd: Hwnd, bEnable: i32) -> i32;
        fn SetFocus(hwnd: Hwnd) -> Hwnd;
        fn UpdateWindow(hwnd: Hwnd) -> i32;
        fn GetMessageW(lpMsg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
        fn TranslateMessage(lpMsg: *const Msg) -> i32;
        fn DispatchMessageW(lpMsg: *const Msg) -> Lresult;
        fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: Wparam, lparam: Lparam) -> Lresult;
        fn CallWindowProcW(
            lpPrevWndFunc: isize,
            hwnd: Hwnd,
            msg: u32,
            wparam: Wparam,
            lparam: Lparam,
        ) -> Lresult;
        fn PostQuitMessage(nExitCode: i32);
        fn PostMessageW(hwnd: Hwnd, msg: u32, wparam: Wparam, lparam: Lparam) -> i32;
        fn SendMessageW(hwnd: Hwnd, msg: u32, wparam: Wparam, lparam: Lparam) -> Lresult;
        fn MoveWindow(hwnd: Hwnd, x: i32, y: i32, w: i32, h: i32, repaint: i32) -> i32;
        fn GetClientRect(hwnd: Hwnd, lpRect: *mut [i32; 4]) -> i32;
        fn GetWindowRect(hwnd: Hwnd, lpRect: *mut [i32; 4]) -> i32;
        fn MapWindowPoints(
            hwndFrom: Hwnd,
            hwndTo: Hwnd,
            lpPoints: *mut [i32; 2],
            cPoints: u32,
        ) -> i32;
        fn GetWindowTextLengthW(hwnd: Hwnd) -> i32;
        fn GetWindowTextW(hwnd: Hwnd, lpString: *mut u16, nMaxCount: i32) -> i32;
        fn SetWindowTextW(hwnd: Hwnd, lpString: *const u16) -> i32;
        fn LoadCursorW(hInstance: Hinstance, lpCursorName: *const u16) -> *mut c_void;
        fn LoadIconW(hInstance: Hinstance, lpIconName: *const u16) -> *mut c_void;
        fn MessageBoxW(hwnd: Hwnd, lpText: *const u16, lpCaption: *const u16, uType: u32) -> i32;
        fn GetWindowLongPtrW(hwnd: Hwnd, nIndex: i32) -> isize;
        fn SetWindowLongPtrW(hwnd: Hwnd, nIndex: i32, dwNewLong: isize) -> isize;
        fn GetKeyState(nVirtKey: i32) -> i16;
        fn InvalidateRect(hwnd: Hwnd, lpRect: *const c_void, bErase: i32) -> i32;
        fn RedrawWindow(
            hwnd: Hwnd,
            lprcUpdate: *const c_void,
            hrgnUpdate: *mut c_void,
            flags: u32,
        ) -> i32;
        fn SetTimer(hwnd: Hwnd, nIDEvent: usize, uElapse: u32, lpTimerFunc: *const c_void)
        -> usize;
        fn KillTimer(hwnd: Hwnd, uIDEvent: usize) -> i32;
        fn FillRect(hdc: *mut c_void, lprc: *const [i32; 4], hbr: Hbrush) -> i32;
        fn FrameRect(hdc: *mut c_void, lprc: *const [i32; 4], hbr: Hbrush) -> i32;
        fn DrawTextW(
            hdc: *mut c_void,
            lpchText: *const u16,
            cchText: i32,
            lprc: *mut [i32; 4],
            format: u32,
        ) -> i32;
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
        fn SetTextColor(hdc: *mut c_void, color: u32) -> u32;
        fn SetBkColor(hdc: *mut c_void, color: u32) -> u32;
        fn SetBkMode(hdc: *mut c_void, mode: i32) -> i32;
        fn SelectObject(hdc: *mut c_void, h: *mut c_void) -> *mut c_void;
        fn CreateSolidBrush(color: u32) -> Hbrush;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(lpModuleName: *const u16) -> Hinstance;
        fn LoadLibraryW(lpLibFileName: *const u16) -> *mut c_void;
        fn GetProcAddress(hModule: *mut c_void, lpProcName: *const u8) -> *const c_void;
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
        fn WriteFile(
            hFile: Handle,
            lpBuffer: *const u8,
            nNumberOfBytesToWrite: u32,
            lpNumberOfBytesWritten: *mut u32,
            lpOverlapped: *mut c_void,
        ) -> i32;
        fn TerminateProcess(hProcess: Handle, uExitCode: u32) -> i32;
        fn CreateJobObjectW(lpJobAttributes: *const c_void, lpName: *const u16) -> Handle;
        fn SetInformationJobObject(
            hJob: Handle,
            jobObjectInformationClass: i32,
            lpJobObjectInformation: *const c_void,
            cbJobObjectInformationLength: u32,
        ) -> i32;
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

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn SHBrowseForFolderW(lpbi: *mut BrowseInfoW) -> *mut c_void;
        fn SHGetPathFromIDListW(pidl: *const c_void, pszPath: *mut u16) -> i32;
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoTaskMemFree(pv: *mut c_void);
    }

    unsafe fn gui_begin_paint(hwnd: Hwnd, lp_paint: *mut PaintStruct) -> *mut c_void {
        let dll = to_wide("user32.dll");
        let hmod = unsafe { GetModuleHandleW(dll.as_ptr()) };
        if hmod.is_null() {
            return null_mut();
        }
        let proc = unsafe { GetProcAddress(hmod, b"BeginPaint\0".as_ptr()) };
        if proc.is_null() {
            return null_mut();
        }
        type FnBegin = unsafe extern "system" fn(Hwnd, *mut PaintStruct) -> *mut c_void;
        let func: FnBegin = unsafe { std::mem::transmute(proc) };
        unsafe { func(hwnd, lp_paint) }
    }

    unsafe fn gui_end_paint(hwnd: Hwnd, lp_paint: *const PaintStruct) -> i32 {
        let dll = to_wide("user32.dll");
        let hmod = unsafe { GetModuleHandleW(dll.as_ptr()) };
        if hmod.is_null() {
            return 0;
        }
        let proc = unsafe { GetProcAddress(hmod, b"EndPaint\0".as_ptr()) };
        if proc.is_null() {
            return 0;
        }
        type FnEnd = unsafe extern "system" fn(Hwnd, *const PaintStruct) -> i32;
        let func: FnEnd = unsafe { std::mem::transmute(proc) };
        unsafe { func(hwnd, lp_paint) }
    }

    fn apply_dwm_dark_titlebar(hwnd: Hwnd, dark: bool) {
        let dll = to_wide("dwmapi.dll");
        let hmod = unsafe { LoadLibraryW(dll.as_ptr()) };
        if hmod.is_null() {
            return;
        }
        let proc = unsafe { GetProcAddress(hmod, b"DwmSetWindowAttribute\0".as_ptr()) };
        if proc.is_null() {
            return;
        }
        type FnDwm = unsafe extern "system" fn(Hwnd, u32, *const c_void, u32) -> i32;
        let func: FnDwm = unsafe { std::mem::transmute(proc) };
        let val: i32 = if dark { 1 } else { 0 };
        unsafe {
            let _ = func(hwnd, 20, (&val as *const i32).cast::<c_void>(), 4);
            let _ = func(hwnd, 19, (&val as *const i32).cast::<c_void>(), 4);
        }
    }

    fn apply_control_scrollbar_theme(ctrl: Hwnd, dark: bool) {
        let dll = to_wide("uxtheme.dll");
        let hmod = unsafe { LoadLibraryW(dll.as_ptr()) };
        if hmod.is_null() {
            return;
        }
        let proc = unsafe { GetProcAddress(hmod, b"SetWindowTheme\0".as_ptr()) };
        if proc.is_null() {
            return;
        }
        type FnTheme = unsafe extern "system" fn(Hwnd, *const u16, *const u16) -> i32;
        let func: FnTheme = unsafe { std::mem::transmute(proc) };
        let subapp = if dark {
            to_wide("DarkMode_Explorer")
        } else {
            to_wide("Explorer")
        };
        unsafe {
            let _ = func(ctrl, subapp.as_ptr(), null());
        }
    }

    struct EditorTab {
        path: Option<PathBuf>,
        title: String,
        content: String,
        dirty: bool,
    }

    struct ThemeBrushes {
        win_bg: Hbrush,
        editor_bg: Hbrush,
        gutter_bg: Hbrush,
        btn_bg: Hbrush,
        btn_sel: Hbrush,
        tab_inact: Hbrush,
        tab_act: Hbrush,
        border: Hbrush,
        accent_blue: Hbrush,
        accent_green: Hbrush,
        accent_red: Hbrush,
        light_win_bg: Hbrush,
        light_editor_bg: Hbrush,
        light_gutter_bg: Hbrush,
        light_btn_bg: Hbrush,
        light_btn_sel: Hbrush,
        light_border: Hbrush,
    }

    impl ThemeBrushes {
        fn new() -> Self {
            unsafe {
                Self {
                    win_bg: CreateSolidBrush(CLR_DARK_WIN_BG),
                    editor_bg: CreateSolidBrush(CLR_DARK_EDITOR_BG),
                    gutter_bg: CreateSolidBrush(CLR_DARK_GUTTER_BG),
                    btn_bg: CreateSolidBrush(CLR_DARK_BTN_BG),
                    btn_sel: CreateSolidBrush(CLR_DARK_BTN_SEL),
                    tab_inact: CreateSolidBrush(CLR_DARK_TAB_INACT),
                    tab_act: CreateSolidBrush(CLR_DARK_TAB_ACT),
                    border: CreateSolidBrush(CLR_DARK_BORDER),
                    accent_blue: CreateSolidBrush(CLR_ACCENT_BLUE),
                    accent_green: CreateSolidBrush(CLR_ACCENT_GREEN),
                    accent_red: CreateSolidBrush(CLR_ACCENT_RED),
                    light_win_bg: CreateSolidBrush(CLR_LIGHT_WIN_BG),
                    light_editor_bg: CreateSolidBrush(CLR_LIGHT_EDITOR_BG),
                    light_gutter_bg: CreateSolidBrush(CLR_LIGHT_GUTTER_BG),
                    light_btn_bg: CreateSolidBrush(CLR_LIGHT_BTN_BG),
                    light_btn_sel: CreateSolidBrush(CLR_LIGHT_BTN_SEL),
                    light_border: CreateSolidBrush(CLR_LIGHT_BORDER),
                }
            }
        }

        fn destroy(&self) {
            unsafe {
                for br in [
                    self.win_bg,
                    self.editor_bg,
                    self.gutter_bg,
                    self.btn_bg,
                    self.btn_sel,
                    self.tab_inact,
                    self.tab_act,
                    self.border,
                    self.accent_blue,
                    self.accent_green,
                    self.accent_red,
                    self.light_win_bg,
                    self.light_editor_bg,
                    self.light_gutter_bg,
                    self.light_btn_bg,
                    self.light_btn_sel,
                    self.light_border,
                ] {
                    DeleteObject(br);
                }
            }
        }
    }

    struct GuiState {
        hwnd_main: Hwnd,
        hwnd_tree_hdr: Hwnd,
        hwnd_refresh_tree: Hwnd,
        hwnd_tree: Hwnd,
        hwnd_gutter: Hwnd,
        hwnd_code: Hwnd,
        hwnd_out: Hwnd,
        hwnd_in: Hwnd,
        hwnd_send: Hwnd,
        hwnd_status: Hwnd,
        hwnd_lbl_flags: Hwnd,
        hwnd_flags: Hwnd,
        hwnd_chk_unsafe: Hwnd,
        hwnd_chk_dark: Hwnd,
        hwnd_chk_read: Hwnd,
        hwnd_chk_write: Hwnd,
        hwnd_chk_net: Hwnd,
        hwnd_chk_env: Hwnd,
        hwnd_chk_exec: Hwnd,
        hwnd_chk_input: Hwnd,
        hwnd_chk_clip: Hwnd,
        hwnd_close_tab: Hwnd,
        tab_buttons: Vec<Hwnd>,
        hwnd_find_box: Hwnd,
        hwnd_repl_box: Hwnd,
        hwnd_find_next: Hwnd,
        hwnd_repl_one: Hwnd,
        hwnd_repl_all: Hwnd,
        hwnd_find_close: Hwnd,
        find_visible: bool,
        mono_font: Hfont,
        ui_font: Hfont,
        brushes: ThemeBrushes,
        dark_mode: bool,
        project_root: Option<PathBuf>,
        tree_items: Vec<ProjectTreeItem>,
        tabs: Vec<EditorTab>,
        active_tab: usize,
        ignore_change: bool,
        last_line_count: usize,
        last_first_line: i32,
        out_prev_proc: isize,
        child_proc: Arc<Mutex<Option<isize>>>,
        child_job: Arc<Mutex<Option<isize>>>,
        child_stdin: Arc<Mutex<Option<isize>>>,
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
        let cur_len = unsafe { GetWindowTextLengthW(hwnd_out) }.max(0) as usize;
        if cur_len + normalized.len() > MAX_OUTPUT_CHARS {
            let mut full = get_text(hwnd_out);
            full.push_str(&normalized);
            let keep_target = MAX_OUTPUT_CHARS / 2;
            let mut cut = full.len().saturating_sub(keep_target);
            while cut < full.len() && !full.is_char_boundary(cut) {
                cut += 1;
            }
            let truncated = format!("[... earlier output truncated ...]\r\n{}", &full[cut..]);
            set_text(hwnd_out, &truncated);
            let new_len = unsafe { GetWindowTextLengthW(hwnd_out) }.max(0) as usize;
            unsafe {
                SendMessageW(hwnd_out, EM_SETSEL, new_len, new_len as isize);
                SendMessageW(hwnd_out, EM_SCROLLCARET, 0, 0);
            }
            return;
        }
        let wide = to_wide(&normalized);
        unsafe {
            let len = GetWindowTextLengthW(hwnd_out);
            SendMessageW(hwnd_out, EM_SETSEL, len as usize, len as isize);
            SendMessageW(hwnd_out, EM_REPLACESEL, 0, wide.as_ptr() as isize);
        }
    }

    fn is_checked(hwnd: Hwnd) -> bool {
        unsafe { SendMessageW(hwnd, BM_GETCHECK, 0, 0) == BST_CHECKED }
    }

    fn read_perm_state(state: &GuiState) -> PermissionFlagsState {
        PermissionFlagsState {
            unsafe_mode: is_checked(state.hwnd_chk_unsafe),
            allow_read: is_checked(state.hwnd_chk_read),
            allow_write: is_checked(state.hwnd_chk_write),
            allow_net: is_checked(state.hwnd_chk_net),
            allow_env: is_checked(state.hwnd_chk_env),
            allow_exec: is_checked(state.hwnd_chk_exec),
            allow_input: is_checked(state.hwnd_chk_input),
            allow_clipboard: is_checked(state.hwnd_chk_clip),
        }
    }

    fn sync_active_tab_content(state: &mut GuiState) {
        if let Some(tab) = state.tabs.get_mut(state.active_tab) {
            tab.content = get_text(state.hwnd_code).replace("\r\n", "\n");
        }
    }

    fn update_gutter(state: &mut GuiState, force: bool) {
        let count =
            (unsafe { SendMessageW(state.hwnd_code, EM_GETLINECOUNT, 0, 0) } as usize).max(1);
        let first_vis =
            unsafe { SendMessageW(state.hwnd_code, EM_GETFIRSTVISIBLELINE, 0, 0) } as i32;
        if force || count != state.last_line_count {
            let mut nums = String::with_capacity(count * 5);
            for i in 1..=count {
                if i > 1 {
                    nums.push('\n');
                }
                nums.push_str(&i.to_string());
            }
            set_text(state.hwnd_gutter, &nums);
            state.last_line_count = count;
            let g_first =
                unsafe { SendMessageW(state.hwnd_gutter, EM_GETFIRSTVISIBLELINE, 0, 0) } as i32;
            let delta = first_vis - g_first;
            if delta != 0 {
                unsafe { SendMessageW(state.hwnd_gutter, EM_LINESCROLL, 0, delta as isize) };
            }
            state.last_first_line = first_vis;
        } else if first_vis != state.last_first_line {
            let g_first =
                unsafe { SendMessageW(state.hwnd_gutter, EM_GETFIRSTVISIBLELINE, 0, 0) } as i32;
            let delta = first_vis - g_first;
            if delta != 0 {
                unsafe { SendMessageW(state.hwnd_gutter, EM_LINESCROLL, 0, delta as isize) };
            }
            state.last_first_line = first_vis;
        }
    }

    fn relayout_window(state: &GuiState) {
        let mut rc = [0i32; 4];
        unsafe { GetClientRect(state.hwnd_main, &mut rc) };
        layout_controls(state, rc[2] - rc[0], rc[3] - rc[1]);
        unsafe {
            InvalidateRect(state.hwnd_main, null(), 1);
        }
    }

    fn refresh_tabs_ui(state: &GuiState) {
        for (i, btn) in state.tab_buttons.iter().enumerate() {
            if let Some(tab) = state.tabs.get(i) {
                let star = if tab.dirty { " *" } else { "" };
                let lbl = format!("{}{}", tab.title, star);
                let w = to_wide(&lbl);
                unsafe {
                    SetWindowTextW(*btn, w.as_ptr());
                    ShowWindow(*btn, SW_SHOW);
                    InvalidateRect(*btn, null(), 1);
                }
            } else {
                unsafe { ShowWindow(*btn, SW_HIDE) };
            }
        }
        unsafe {
            EnableWindow(
                state.hwnd_close_tab,
                if state.tabs.len() > 1 { 1 } else { 0 },
            );
            InvalidateRect(state.hwnd_close_tab, null(), 1);
        }
    }

    fn update_status_and_title(state: &GuiState) {
        let tab = &state.tabs[state.active_tab];
        let star = if tab.dirty { " *" } else { "" };
        let path_lbl = tab
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| tab.title.clone());
        let win_title = to_wide(&format!(
            "Askane Studio v{} - {}{}",
            crate::VERSION,
            tab.title,
            star
        ));
        unsafe { SetWindowTextW(state.hwnd_main, win_title.as_ptr()) };

        let mut sel_start: u32 = 0;
        let mut sel_end: u32 = 0;
        unsafe {
            SendMessageW(
                state.hwnd_code,
                EM_GETSEL,
                (&mut sel_start as *mut u32) as usize,
                (&mut sel_end as *mut u32) as isize,
            );
        }
        let line_idx =
            unsafe { SendMessageW(state.hwnd_code, EM_LINEFROMCHAR, sel_start as usize, 0) }.max(0)
                as usize;
        let line_start =
            unsafe { SendMessageW(state.hwnd_code, EM_LINEINDEX, line_idx, 0) }.max(0) as usize;
        let col = (sel_start as usize).saturating_sub(line_start) + 1;

        let perms = read_perm_state(state);
        let extra = get_text(state.hwnd_flags);
        let cmd_preview =
            match compose_permission_flags(&perms, state.project_root.as_deref(), &extra) {
                Ok(flags) => {
                    if flags.is_empty() {
                        "askane (sandbox)".to_string()
                    } else {
                        format!("askane {}", flags.join(" "))
                    }
                }
                Err(e) => format!("INVALID FLAG ({})", e),
            };
        let status = format!(
            " Ln {}, Col {}  |  {}  |  Cmd: {}",
            line_idx + 1,
            col,
            path_lbl,
            cmd_preview
        );
        let w_st = to_wide(&status);
        unsafe { SetWindowTextW(state.hwnd_status, w_st.as_ptr()) };
    }

    fn load_tab_into_editor(state: &mut GuiState, idx: usize) {
        if idx >= state.tabs.len() {
            return;
        }
        state.active_tab = idx;
        state.ignore_change = true;
        let text = state.tabs[idx].content.clone();
        set_text(state.hwnd_code, &text);
        state.ignore_change = false;
        relayout_window(state);
        update_gutter(state, true);
        refresh_tabs_ui(state);
        update_status_and_title(state);
    }

    fn switch_to_tab(state: &mut GuiState, idx: usize) {
        if idx >= state.tabs.len() || idx == state.active_tab {
            return;
        }
        sync_active_tab_content(state);
        load_tab_into_editor(state, idx);
    }

    fn populate_project_tree(state: &mut GuiState) {
        unsafe { SendMessageW(state.hwnd_tree, LB_RESETCONTENT, 0, 0) };
        state.tree_items.clear();
        let Some(root) = state.project_root.clone() else {
            let w_hdr = to_wide(" Project: (no folder)");
            unsafe { SetWindowTextW(state.hwnd_tree_hdr, w_hdr.as_ptr()) };
            return;
        };
        let folder_name = root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.display().to_string());
        let w_hdr = to_wide(&format!(" Project: {}", folder_name));
        unsafe { SetWindowTextW(state.hwnd_tree_hdr, w_hdr.as_ptr()) };

        let items = scan_project_tree(&root, 400);
        for it in &items {
            let w_item = to_wide(&it.display);
            unsafe { SendMessageW(state.hwnd_tree, LB_ADDSTRING, 0, w_item.as_ptr() as isize) };
        }
        unsafe { SendMessageW(state.hwnd_tree, LB_SETCURSEL, usize::MAX, 0) };
        state.tree_items = items;
    }

    fn open_path_in_tab(state: &mut GuiState, p: PathBuf) {
        let canon = fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
        if let Some(existing) = state.tabs.iter().position(|t| {
            t.path
                .as_ref()
                .and_then(|tp| fs::canonicalize(tp).ok())
                .is_some_and(|tc| tc == canon)
        }) {
            switch_to_tab(state, existing);
            return;
        }
        match crate::read_text_limited(&p, crate::MAX_SOURCE_BYTES) {
            Ok(content) => {
                sync_active_tab_content(state);
                let title = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "script.ask".to_string());
                let new_tab = EditorTab {
                    path: Some(p.clone()),
                    title,
                    content: content.replace("\r\n", "\n"),
                    dirty: false,
                };
                if state.tabs.len() == 1
                    && state.tabs[0].path.is_none()
                    && !state.tabs[0].dirty
                    && state.tabs[0]
                        .content
                        .trim()
                        .starts_with("// Write Askane code")
                {
                    state.tabs[0] = new_tab;
                    load_tab_into_editor(state, 0);
                } else {
                    if state.tabs.len() >= MAX_TABS {
                        append_out(
                            state.hwnd_out,
                            &format!("[Max {} tabs reached; close a tab first]\n", MAX_TABS),
                        );
                        return;
                    }
                    state.tabs.push(new_tab);
                    let idx = state.tabs.len() - 1;
                    load_tab_into_editor(state, idx);
                }
                if state.project_root.is_none() {
                    state.project_root = Some(find_project_root_for(&p));
                    populate_project_tree(state);
                }
            }
            Err(e) => append_out(state.hwnd_out, &format!("Open failed: {}\n", e)),
        }
    }

    fn create_new_tab(state: &mut GuiState) {
        if state.tabs.len() >= MAX_TABS {
            append_out(
                state.hwnd_out,
                &format!("[Max {} tabs reached; close a tab first]\n", MAX_TABS),
            );
            return;
        }
        sync_active_tab_content(state);
        state.tabs.push(EditorTab {
            path: None,
            title: format!("untitled_{}.ask", state.tabs.len() + 1),
            content: String::new(),
            dirty: false,
        });
        let idx = state.tabs.len() - 1;
        load_tab_into_editor(state, idx);
    }

    fn save_tab_at(hwnd: Hwnd, state: &mut GuiState, idx: usize) -> bool {
        sync_active_tab_content(state);
        let Some(tab) = state.tabs.get(idx) else {
            return false;
        };
        let target = tab.path.clone().or_else(|| save_file_dialog(hwnd));
        let Some(p) = target else {
            return false;
        };
        let content = state.tabs[idx].content.clone();
        match fs::write(&p, content) {
            Ok(_) => {
                let title = p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "script.ask".to_string());
                state.tabs[idx].path = Some(p.clone());
                state.tabs[idx].title = title;
                state.tabs[idx].dirty = false;
                if state.project_root.is_none() {
                    state.project_root = Some(find_project_root_for(&p));
                }
                populate_project_tree(state);
                refresh_tabs_ui(state);
                update_status_and_title(state);
                append_out(state.hwnd_out, &format!("Saved {}\n", p.display()));
                true
            }
            Err(e) => {
                append_out(state.hwnd_out, &format!("Save failed: {}\n", e));
                false
            }
        }
    }

    fn confirm_tab_close(hwnd: Hwnd, state: &mut GuiState, idx: usize) -> bool {
        if idx >= state.tabs.len() || !state.tabs[idx].dirty {
            return true;
        }
        let msg = to_wide(&format!(
            "Save changes to '{}' before closing?",
            state.tabs[idx].title
        ));
        let cap = to_wide("Unsaved Changes");
        let choice = unsafe {
            MessageBoxW(
                hwnd,
                msg.as_ptr(),
                cap.as_ptr(),
                MB_YESNOCANCEL | MB_ICONQUESTION,
            )
        };
        if choice == IDCANCEL {
            return false;
        }
        if choice == IDYES {
            return save_tab_at(hwnd, state, idx);
        }
        true
    }

    fn close_active_tab(hwnd: Hwnd, state: &mut GuiState) {
        if state.tabs.len() <= 1 {
            return;
        }
        let idx = state.active_tab;
        if !confirm_tab_close(hwnd, state, idx) {
            return;
        }
        state.tabs.remove(idx);
        let next = idx.min(state.tabs.len().saturating_sub(1));
        load_tab_into_editor(state, next);
    }

    fn stop_child(state: &GuiState) {
        {
            let mut in_guard = state.child_stdin.lock().unwrap();
            if let Some(raw_in) = in_guard.take() {
                unsafe { CloseHandle(raw_in as Handle) };
            }
        }
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

    fn spawn_askane_child(
        hwnd: Hwnd,
        state: &GuiState,
        args: Vec<String>,
        work_dir: Option<&Path>,
        cleanup_tmp: Option<PathBuf>,
    ) {
        let exe = match std::env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                if let Some(p) = cleanup_tmp {
                    let _ = fs::remove_file(p);
                }
                append_out(
                    state.hwnd_out,
                    &format!("Failed to locate askane.exe: {}\n", e),
                );
                return;
            }
        };

        let mut cmd = quote_windows_arg(&exe.to_string_lossy());
        for a in &args {
            cmd.push(' ');
            cmd.push_str(&quote_windows_arg(a));
        }

        let sa = SecurityAttributes {
            n_length: std::mem::size_of::<SecurityAttributes>() as u32,
            lp_security_descriptor: null_mut(),
            b_inherit_handle: 1,
        };
        let mut h_read: Handle = null_mut();
        let mut h_write: Handle = null_mut();
        if unsafe { CreatePipe(&mut h_read, &mut h_write, &sa, 0) } == 0 {
            if let Some(p) = cleanup_tmp {
                let _ = fs::remove_file(p);
            }
            append_out(state.hwnd_out, "Failed to create output pipe.\n");
            return;
        }
        unsafe { SetHandleInformation(h_read, HANDLE_FLAG_INHERIT, 0) };

        let mut h_in_read: Handle = null_mut();
        let mut h_in_write: Handle = null_mut();
        if unsafe { CreatePipe(&mut h_in_read, &mut h_in_write, &sa, 0) } == 0 {
            unsafe {
                CloseHandle(h_read);
                CloseHandle(h_write);
            }
            if let Some(p) = cleanup_tmp {
                let _ = fs::remove_file(p);
            }
            append_out(state.hwnd_out, "Failed to create input pipe.\n");
            return;
        }
        unsafe { SetHandleInformation(h_in_write, HANDLE_FLAG_INHERIT, 0) };

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
            h_std_input: h_in_read,
            h_std_output: h_write,
            h_std_error: h_write,
        };
        let mut pi = ProcessInformation {
            h_process: null_mut(),
            h_thread: null_mut(),
            dw_process_id: 0,
            dw_thread_id: 0,
        };

        let cwd_wide = work_dir.map(|d| to_wide(&d.to_string_lossy()));
        let cwd_ptr = cwd_wide.as_ref().map_or(null(), |w| w.as_ptr());
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
                cwd_ptr,
                &si,
                &mut pi,
            )
        };
        unsafe {
            CloseHandle(h_write);
            CloseHandle(h_in_read);
        }
        if ok == 0 {
            unsafe {
                CloseHandle(h_read);
                CloseHandle(h_in_write);
            }
            if let Some(p) = cleanup_tmp {
                let _ = fs::remove_file(p);
            }
            append_out(state.hwnd_out, "Failed to start askane child process.\n");
            return;
        }

        unsafe { CloseHandle(pi.h_thread) };
        let h_job = unsafe { CreateJobObjectW(null(), null()) };
        if h_job.is_null() {
            unsafe {
                TerminateProcess(pi.h_process, 1);
                CloseHandle(pi.h_process);
                CloseHandle(h_read);
                CloseHandle(h_in_write);
            }
            if let Some(p) = cleanup_tmp {
                let _ = fs::remove_file(p);
            }
            append_out(
                state.hwnd_out,
                "Failed to create Job Object; child process was not started.\n",
            );
            return;
        }
        let mut job_info = JobExtendedLimitInformation::default();
        job_info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let set_ok = unsafe {
            SetInformationJobObject(
                h_job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                (&job_info as *const JobExtendedLimitInformation).cast::<c_void>(),
                std::mem::size_of::<JobExtendedLimitInformation>() as u32,
            )
        };
        if set_ok == 0 || unsafe { AssignProcessToJobObject(h_job, pi.h_process) } == 0 {
            unsafe {
                TerminateProcess(pi.h_process, 1);
                CloseHandle(h_job);
                CloseHandle(pi.h_process);
                CloseHandle(h_read);
                CloseHandle(h_in_write);
            }
            if let Some(p) = cleanup_tmp {
                let _ = fs::remove_file(p);
            }
            append_out(
                state.hwnd_out,
                "Failed to isolate Askane child process in a Job Object.\n",
            );
            return;
        }

        let h_proc_raw = pi.h_process as isize;
        let h_job_raw = h_job as isize;
        let h_in_raw = h_in_write as isize;
        *state.child_job.lock().unwrap() = Some(h_job_raw);
        *state.child_proc.lock().unwrap() = Some(h_proc_raw);
        *state.child_stdin.lock().unwrap() = Some(h_in_raw);

        let hwnd_raw = hwnd as isize;
        let h_read_raw = h_read as isize;
        let child_arc = Arc::clone(&state.child_proc);
        let job_arc = Arc::clone(&state.child_job);
        let stdin_arc = Arc::clone(&state.child_stdin);

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
                unsafe { PostMessageW(hwnd, WM_APP_OUT, 0, boxed) };
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
                if *jguard == Some(h_job_raw) {
                    jguard.take();
                    unsafe { CloseHandle(h_job_raw as Handle) };
                }
            }
            {
                let mut iguard = stdin_arc.lock().unwrap();
                if *iguard == Some(h_in_raw) {
                    iguard.take();
                    unsafe { CloseHandle(h_in_raw as Handle) };
                }
            }
            if let Some(p) = cleanup_tmp {
                let _ = fs::remove_file(p);
            }
            unsafe { PostMessageW(hwnd, WM_APP_DONE, exit_code as Wparam, 0) };
        });
    }

    fn run_script(hwnd: Hwnd, state: &mut GuiState) {
        stop_child(state);
        set_text(state.hwnd_out, "");
        sync_active_tab_content(state);

        let perms = read_perm_state(state);
        let extra = get_text(state.hwnd_flags);
        let mut args = match compose_permission_flags(&perms, state.project_root.as_deref(), &extra)
        {
            Ok(a) => a,
            Err(e) => {
                append_out(state.hwnd_out, &format!("{}\n", e));
                return;
            }
        };

        let tab = &state.tabs[state.active_tab];
        let (target_path, work_dir, cleanup_tmp) = if let Some(saved) = &tab.path {
            if !tab.dirty {
                let dir = saved
                    .parent()
                    .map(Path::to_path_buf)
                    .or_else(|| state.project_root.clone());
                (saved.clone(), dir, None)
            } else {
                append_out(
                    state.hwnd_out,
                    "[Running unsaved buffer from temp file; save (Ctrl+S) to run from project dir]\n",
                );
                let tmp = std::env::temp_dir().join(format!(
                    "askane_studio_{}_{}.ask",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                ));
                if let Err(e) = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&tmp)
                    .and_then(|mut f| f.write_all(tab.content.as_bytes()))
                {
                    append_out(
                        state.hwnd_out,
                        &format!("Failed to write temp file: {}\n", e),
                    );
                    return;
                }
                (tmp.clone(), state.project_root.clone(), Some(tmp))
            }
        } else {
            let tmp = std::env::temp_dir().join(format!(
                "askane_studio_{}_{}.ask",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            if let Err(e) = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .and_then(|mut f| f.write_all(tab.content.as_bytes()))
            {
                append_out(
                    state.hwnd_out,
                    &format!("Failed to write temp file: {}\n", e),
                );
                return;
            }
            (tmp.clone(), state.project_root.clone(), Some(tmp))
        };

        args.push(target_path.to_string_lossy().to_string());
        spawn_askane_child(hwnd, state, args, work_dir.as_deref(), cleanup_tmp);
    }

    fn run_project_tests(hwnd: Hwnd, state: &mut GuiState) {
        stop_child(state);
        set_text(state.hwnd_out, "");

        let target = state
            .project_root
            .clone()
            .or_else(|| state.tabs[state.active_tab].path.clone());
        let Some(path) = target else {
            append_out(
                state.hwnd_out,
                "Open a folder or save a .ask file first to run tests.\n",
            );
            return;
        };

        let perms = read_perm_state(state);
        let extra = get_text(state.hwnd_flags);
        let mut args = vec!["test".to_string()];
        match compose_permission_flags(&perms, state.project_root.as_deref(), &extra) {
            Ok(flags) => args.extend(flags),
            Err(e) => {
                append_out(state.hwnd_out, &format!("{}\n", e));
                return;
            }
        }
        args.push(path.to_string_lossy().to_string());
        append_out(
            state.hwnd_out,
            &format!("[Running tests: {}]\n", path.display()),
        );
        spawn_askane_child(hwnd, state, args, state.project_root.as_deref(), None);
    }

    fn open_file_dialog(hwnd: Hwnd) -> Option<PathBuf> {
        let mut buf = vec![0u16; 1024];
        let filter =
            to_wide("Askane Files (*.ask;askane.toml)\0*.ask;askane.toml\0All Files (*.*)\0*.*\0");
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
            flags: OFN_PATHMUSTEXIST | OFN_FILEMUSTEXIST | 0x0000_0008,
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
            flags: OFN_OVERWRITEPROMPT | 0x0000_0008,
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

    fn browse_folder_dialog(hwnd: Hwnd) -> Option<PathBuf> {
        let mut disp = vec![0u16; 260];
        let title = to_wide("Select Askane Project Folder");
        let mut bi = BrowseInfoW {
            hwnd_owner: hwnd,
            pidl_root: null(),
            psz_display_name: disp.as_mut_ptr(),
            lpsz_title: title.as_ptr(),
            ul_flags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE | BIF_EDITBOX,
            lpfn: null(),
            l_param: 0,
            i_image: 0,
        };
        let pidl = unsafe { SHBrowseForFolderW(&mut bi) };
        if pidl.is_null() {
            return None;
        }
        let mut path_buf = vec![0u16; 1024];
        let ok = unsafe { SHGetPathFromIDListW(pidl, path_buf.as_mut_ptr()) };
        unsafe { CoTaskMemFree(pidl) };
        if ok != 0 {
            let end = path_buf
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(path_buf.len());
            Some(PathBuf::from(String::from_utf16_lossy(&path_buf[..end])))
        } else {
            None
        }
    }

    fn locate_examples_dir() -> Option<PathBuf> {
        if let Ok(exe) = crate::setup::win32_setup::current_exe_clean() {
            if let Some(dir) = exe.parent() {
                let candidate = dir.join("examples");
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
        }
        if let Ok(cwd) = std::env::current_dir() {
            let candidate = cwd.join("examples");
            if candidate.is_dir() {
                return Some(candidate);
            }
        }
        None
    }

    fn jump_code_to_line(state: &GuiState, line_1based: usize, col_1based: usize) {
        let line_idx = line_1based.saturating_sub(1);
        let char_idx = unsafe { SendMessageW(state.hwnd_code, EM_LINEINDEX, line_idx, 0) };
        if char_idx >= 0 {
            let target = (char_idx as usize) + col_1based.saturating_sub(1);
            unsafe {
                SendMessageW(state.hwnd_code, EM_SETSEL, target, target as isize);
                SendMessageW(state.hwnd_code, EM_SCROLLCARET, 0, 0);
                SetFocus(state.hwnd_code);
            }
            update_status_and_title(state);
        }
    }

    fn find_next_in_code(state: &GuiState, needle: &str) -> bool {
        if needle.is_empty() {
            return false;
        }
        let text = get_text(state.hwnd_code);
        let text_lower = text.to_ascii_lowercase();
        let needle_lower = needle.to_ascii_lowercase();

        let mut sel_start: u32 = 0;
        let mut sel_end: u32 = 0;
        unsafe {
            SendMessageW(
                state.hwnd_code,
                EM_GETSEL,
                (&mut sel_start as *mut u32) as usize,
                (&mut sel_end as *mut u32) as isize,
            );
        }
        let start_utf16 = sel_end as usize;
        let mut utf16_pos = 0usize;
        let mut byte_from = text.len();
        for (bidx, ch) in text.char_indices() {
            if utf16_pos >= start_utf16 {
                byte_from = bidx;
                break;
            }
            utf16_pos += ch.len_utf16();
        }

        let found_byte = text_lower[byte_from..]
            .find(&needle_lower)
            .map(|rel| byte_from + rel)
            .or_else(|| text_lower.find(&needle_lower));

        let Some(match_byte) = found_byte else {
            append_out(state.hwnd_out, &format!("Find: '{}' not found.\n", needle));
            return false;
        };

        let u16_start: usize = text[..match_byte].chars().map(|c| c.len_utf16()).sum();
        let u16_len: usize = text[match_byte..match_byte + needle.len()]
            .chars()
            .map(|c| c.len_utf16())
            .sum();
        unsafe {
            SendMessageW(
                state.hwnd_code,
                EM_SETSEL,
                u16_start,
                (u16_start + u16_len) as isize,
            );
            SendMessageW(state.hwnd_code, EM_SCROLLCARET, 0, 0);
            SetFocus(state.hwnd_code);
        }
        update_status_and_title(state);
        true
    }

    fn replace_current_or_next(state: &mut GuiState, needle: &str, repl: &str) {
        if needle.is_empty() {
            return;
        }
        let mut sel_start: u32 = 0;
        let mut sel_end: u32 = 0;
        unsafe {
            SendMessageW(
                state.hwnd_code,
                EM_GETSEL,
                (&mut sel_start as *mut u32) as usize,
                (&mut sel_end as *mut u32) as isize,
            );
        }
        if sel_end > sel_start {
            let text = get_text(state.hwnd_code);
            let utf16_all: Vec<u16> = text.encode_utf16().collect();
            let s = (sel_start as usize).min(utf16_all.len());
            let e = (sel_end as usize).min(utf16_all.len());
            let selected = String::from_utf16_lossy(&utf16_all[s..e]);
            if selected.eq_ignore_ascii_case(needle) {
                let norm_r = repl.replace("\r\n", "\n").replace('\n', "\r\n");
                let w_r = to_wide(&norm_r);
                unsafe {
                    SendMessageW(state.hwnd_code, EM_REPLACESEL, 1, w_r.as_ptr() as isize);
                }
                if let Some(tab) = state.tabs.get_mut(state.active_tab) {
                    tab.dirty = true;
                }
                refresh_tabs_ui(state);
            }
        }
        find_next_in_code(state, needle);
    }

    fn replace_all_in_code(state: &mut GuiState, needle: &str, repl: &str) {
        if needle.is_empty() {
            return;
        }
        let code = get_text(state.hwnd_code).replace("\r\n", "\n");
        let count = code.matches(needle).count();
        if count == 0 {
            append_out(
                state.hwnd_out,
                &format!("Replace All: '{}' not found.\n", needle),
            );
            return;
        }
        let updated = code.replace(needle, repl);
        state.ignore_change = true;
        set_text(state.hwnd_code, &updated);
        state.ignore_change = false;
        if let Some(tab) = state.tabs.get_mut(state.active_tab) {
            tab.content = updated;
            tab.dirty = true;
        }
        update_gutter(state, true);
        refresh_tabs_ui(state);
        update_status_and_title(state);
        append_out(
            state.hwnd_out,
            &format!("Replaced {} occurrence(s) of '{}'.\n", count, needle),
        );
    }

    fn layout_controls(state: &GuiState, w: i32, h: i32) {
        let sidebar_w = 236;
        let left_x = 10;
        let work_x = left_x + sidebar_w + 10;
        let work_w = (w - work_x - 10).max(200);
        let find_h = if state.find_visible { 32 } else { 0 };
        let top_y = 72;
        let status_h = 22;
        let in_h = 28;

        // Expand flags input dynamically with window width
        let flags_x = 486;
        let flags_w = (w - flags_x - 10).max(120);
        unsafe {
            MoveWindow(state.hwnd_lbl_flags, 444, 40, 40, 20, 1);
            MoveWindow(state.hwnd_flags, flags_x, 38, flags_w, 22, 1);
        }

        // Sidebar layout
        let side_h = (h - top_y - status_h - 10).max(160);
        unsafe {
            MoveWindow(
                state.hwnd_tree_hdr,
                left_x,
                top_y + 2,
                sidebar_w - 64,
                20,
                1,
            );
            MoveWindow(
                state.hwnd_refresh_tree,
                left_x + sidebar_w - 60,
                top_y,
                60,
                22,
                1,
            );
            MoveWindow(
                state.hwnd_tree,
                left_x + 1,
                top_y + 27,
                sidebar_w - 2,
                side_h - 28,
                1,
            );
        }

        // Find/Replace bar
        let find_show = if state.find_visible { SW_SHOW } else { SW_HIDE };
        for ctrl in [
            state.hwnd_find_box,
            state.hwnd_repl_box,
            state.hwnd_find_next,
            state.hwnd_repl_one,
            state.hwnd_repl_all,
            state.hwnd_find_close,
        ] {
            unsafe { ShowWindow(ctrl, find_show) };
        }
        if state.find_visible {
            let fy = top_y;
            let field_w = ((work_w - 210) / 2).max(90);
            unsafe {
                MoveWindow(state.hwnd_find_box, work_x + 1, fy + 1, field_w, 22, 1);
                MoveWindow(
                    state.hwnd_repl_box,
                    work_x + field_w + 9,
                    fy + 1,
                    field_w,
                    22,
                    1,
                );
                let bx = work_x + field_w * 2 + 16;
                MoveWindow(state.hwnd_find_next, bx, fy, 52, 24, 1);
                MoveWindow(state.hwnd_repl_one, bx + 56, fy, 60, 24, 1);
                MoveWindow(state.hwnd_repl_all, bx + 120, fy, 42, 24, 1);
                MoveWindow(state.hwnd_find_close, bx + 166, fy, 26, 24, 1);
            }
        }

        // Tab strip (evenly sized flat IDE tabs)
        let tab_y = top_y + find_h;
        let tab_h = 26;
        let close_w = 28;
        let count = state.tabs.len().max(1) as i32;
        let max_tabs_area = (work_w - close_w - 8).max(90);
        let each_w = ((max_tabs_area - (count - 1) * 4) / count).clamp(64, 170);
        for (i, btn) in state.tab_buttons.iter().enumerate() {
            if i < state.tabs.len() {
                unsafe {
                    MoveWindow(
                        *btn,
                        work_x + (i as i32) * (each_w + 4),
                        tab_y,
                        each_w,
                        tab_h,
                        1,
                    );
                }
            }
        }
        unsafe {
            MoveWindow(
                state.hwnd_close_tab,
                work_x + work_w - close_w,
                tab_y,
                close_w,
                tab_h,
                1,
            );
        }

        // Editor + Gutter + Output + Stdin + Status
        let ed_y = tab_y + tab_h + 2;
        let out_h = ((h - ed_y - in_h - status_h - 24) / 3).clamp(90, 260);
        let ed_h = (h - ed_y - out_h - in_h - status_h - 24).max(110);
        let out_y = ed_y + ed_h + 8;
        let in_y = out_y + out_h + 8;
        let st_y = h - status_h;

        let gutter_w = 46;
        unsafe {
            MoveWindow(
                state.hwnd_gutter,
                work_x + 1,
                ed_y + 1,
                gutter_w,
                ed_h - 2,
                1,
            );
            MoveWindow(
                state.hwnd_code,
                work_x + gutter_w + 2,
                ed_y + 1,
                (work_w - gutter_w - 3).max(120),
                ed_h - 2,
                1,
            );
            MoveWindow(
                state.hwnd_out,
                work_x + 1,
                out_y + 1,
                work_w - 2,
                out_h - 2,
                1,
            );
            MoveWindow(
                state.hwnd_in,
                work_x + 1,
                in_y + 1,
                (work_w - 82).max(80),
                22,
                1,
            );
            MoveWindow(state.hwnd_send, work_x + work_w - 74, in_y, 74, 24, 1);
            MoveWindow(
                state.hwnd_status,
                10,
                st_y,
                (w - 20).max(120),
                status_h - 2,
                1,
            );
        }
    }

    fn frame_child_box(hwnd_main: Hwnd, hdc: *mut c_void, child: Hwnd, br: Hbrush) {
        let mut rc = [0i32; 4];
        unsafe {
            if GetWindowRect(child, &mut rc) == 0 {
                return;
            }
            MapWindowPoints(null_mut(), hwnd_main, rc.as_mut_ptr().cast::<[i32; 2]>(), 2);
            rc[0] -= 1;
            rc[1] -= 1;
            rc[2] += 1;
            rc[3] += 1;
            FrameRect(hdc, &rc, br);
        }
    }

    fn draw_owner_button(state: &GuiState, dis: &DrawItemStruct) {
        let id = dis.ctl_id as usize;
        let selected = (dis.item_state & ODS_SELECTED) != 0;
        let dark = state.dark_mode;

        let is_tab = (ID_TAB_BASE..ID_TAB_BASE + MAX_TABS).contains(&id);
        let is_active_tab = is_tab && (id - ID_TAB_BASE == state.active_tab);

        let (bg, border, fg, top_accent) = if is_tab {
            if is_active_tab {
                (
                    if dark {
                        state.brushes.tab_act
                    } else {
                        state.brushes.light_editor_bg
                    },
                    if dark {
                        state.brushes.border
                    } else {
                        state.brushes.light_border
                    },
                    if dark { CLR_DARK_TEXT } else { CLR_LIGHT_TEXT },
                    Some(state.brushes.accent_blue),
                )
            } else {
                (
                    if dark {
                        if selected {
                            state.brushes.btn_sel
                        } else {
                            state.brushes.tab_inact
                        }
                    } else {
                        state.brushes.light_btn_bg
                    },
                    if dark {
                        state.brushes.border
                    } else {
                        state.brushes.light_border
                    },
                    if dark {
                        CLR_DARK_MUTED
                    } else {
                        CLR_LIGHT_MUTED
                    },
                    None,
                )
            }
        } else if id == ID_BTN_RUN {
            (
                if selected {
                    state.brushes.btn_sel
                } else {
                    state.brushes.accent_green
                },
                state.brushes.accent_green,
                0x00FF_FFFF,
                None,
            )
        } else if id == ID_BTN_STOP {
            let has_child = state.child_proc.lock().unwrap().is_some();
            if has_child {
                (
                    state.brushes.accent_red,
                    state.brushes.accent_red,
                    0x00FF_FFFF,
                    None,
                )
            } else {
                (
                    if dark {
                        if selected {
                            state.brushes.btn_sel
                        } else {
                            state.brushes.btn_bg
                        }
                    } else {
                        if selected {
                            state.brushes.light_btn_sel
                        } else {
                            state.brushes.light_btn_bg
                        }
                    },
                    if dark {
                        state.brushes.border
                    } else {
                        state.brushes.light_border
                    },
                    if dark {
                        CLR_DARK_MUTED
                    } else {
                        CLR_LIGHT_MUTED
                    },
                    None,
                )
            }
        } else if id == ID_BTN_TEST || id == ID_BTN_SETUP {
            (
                if dark {
                    if selected {
                        state.brushes.btn_sel
                    } else {
                        state.brushes.btn_bg
                    }
                } else {
                    if selected {
                        state.brushes.light_btn_sel
                    } else {
                        state.brushes.light_btn_bg
                    }
                },
                state.brushes.accent_blue,
                if dark { CLR_DARK_TEXT } else { CLR_LIGHT_TEXT },
                None,
            )
        } else {
            (
                if dark {
                    if selected {
                        state.brushes.btn_sel
                    } else {
                        state.brushes.btn_bg
                    }
                } else {
                    if selected {
                        state.brushes.light_btn_sel
                    } else {
                        state.brushes.light_btn_bg
                    }
                },
                if dark {
                    state.brushes.border
                } else {
                    state.brushes.light_border
                },
                if dark { CLR_DARK_TEXT } else { CLR_LIGHT_TEXT },
                None,
            )
        };

        let mut rc = dis.rc_item;
        unsafe {
            FillRect(dis.hdc, &rc, bg);
            FrameRect(dis.hdc, &rc, border);
            if let Some(acc) = top_accent {
                let top_bar = [rc[0], rc[1], rc[2], rc[1] + 2];
                FillRect(dis.hdc, &top_bar, acc);
            }
            SetBkMode(dis.hdc, TRANSPARENT);
            SetTextColor(dis.hdc, fg);
            SelectObject(dis.hdc, state.ui_font);
            let txt = get_text(dis.hwnd_item);
            let mut pad_rc = [rc[0] + 4, rc[1], rc[2] - 4, rc[3]];
            let wtxt = to_wide(&txt);
            DrawTextW(
                dis.hdc,
                wtxt.as_ptr(),
                -1,
                &mut pad_rc,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
            );
        }
        let _ = &mut rc;
    }

    fn apply_theme_to_window(state: &GuiState) {
        apply_dwm_dark_titlebar(state.hwnd_main, state.dark_mode);
        for ctrl in [
            state.hwnd_code,
            state.hwnd_gutter,
            state.hwnd_out,
            state.hwnd_tree,
        ] {
            apply_control_scrollbar_theme(ctrl, state.dark_mode);
        }
        unsafe {
            RedrawWindow(
                state.hwnd_main,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
            );
        }
    }

    unsafe extern "system" fn out_subclass_proc(
        hwnd: Hwnd,
        msg: u32,
        wparam: Wparam,
        lparam: Lparam,
    ) -> Lresult {
        let main_hwnd = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as Hwnd;
        if !main_hwnd.is_null() {
            let state_ptr = unsafe { GetWindowLongPtrW(main_hwnd, GWLP_USERDATA) } as *mut GuiState;
            if !state_ptr.is_null() {
                let state = unsafe { &*state_ptr };
                if msg == WM_LBUTTONDBLCLK {
                    let res =
                        unsafe { CallWindowProcW(state.out_prev_proc, hwnd, msg, wparam, lparam) };
                    let mut sel_s: u32 = 0;
                    let mut sel_e: u32 = 0;
                    unsafe {
                        SendMessageW(
                            hwnd,
                            EM_GETSEL,
                            (&mut sel_s as *mut u32) as usize,
                            (&mut sel_e as *mut u32) as isize,
                        );
                    }
                    let line_idx = unsafe { SendMessageW(hwnd, EM_LINEFROMCHAR, sel_s as usize, 0) }
                        .max(0) as usize;
                    let text = get_text(hwnd).replace("\r\n", "\n");
                    if let Some(clicked_line) = text.lines().nth(line_idx) {
                        if let Some((ln, col)) = parse_error_line_col(clicked_line) {
                            jump_code_to_line(state, ln, col);
                        }
                    }
                    return res;
                }
                return unsafe { CallWindowProcW(state.out_prev_proc, hwnd, msg, wparam, lparam) };
            }
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    struct SetupDialogState {
        hwnd_lbl: Hwnd,
        hwnd_studio_out: Hwnd,
        exe_path: PathBuf,
        dark_mode: bool,
        win_bg: Hbrush,
        editor_bg: Hbrush,
        btn_bg: Hbrush,
        border: Hbrush,
        accent_blue: Hbrush,
        ui_font: Hfont,
    }

    fn refresh_setup_dialog_label(dlg: &SetupDialogState) {
        let st = crate::setup::win32_setup::current_status(&dlg.exe_path);
        let text = format!(
            "Executable: {}\r\nUser PATH:  {}\r\n.ask Open:  {}",
            st.exe_path,
            if st.in_path {
                "Installed"
            } else {
                "Not in PATH"
            },
            st.assoc_mode
        );
        let w = to_wide(&text);
        unsafe { SetWindowTextW(dlg.hwnd_lbl, w.as_ptr()) };
    }

    unsafe extern "system" fn setup_dlg_proc(
        hwnd: Hwnd,
        msg: u32,
        wparam: Wparam,
        lparam: Lparam,
    ) -> Lresult {
        let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut SetupDialogState;
        match msg {
            WM_ERASEBKGND => {
                if !ptr.is_null() {
                    let dlg = unsafe { &*ptr };
                    if dlg.dark_mode {
                        let mut rc = [0i32; 4];
                        unsafe {
                            GetClientRect(hwnd, &mut rc);
                            FillRect(wparam as *mut c_void, &rc, dlg.win_bg);
                        }
                        return 1;
                    }
                }
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
                if !ptr.is_null() {
                    let dlg = unsafe { &*ptr };
                    if dlg.dark_mode {
                        let hdc = wparam as *mut c_void;
                        unsafe {
                            SetTextColor(hdc, CLR_DARK_TEXT);
                            SetBkColor(hdc, CLR_DARK_EDITOR_BG);
                        }
                        return dlg.editor_bg as Lresult;
                    }
                }
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            WM_DRAWITEM => {
                if !ptr.is_null() {
                    let dlg = unsafe { &*ptr };
                    let dis = unsafe { &*(lparam as *const DrawItemStruct) };
                    let selected = (dis.item_state & ODS_SELECTED) != 0;
                    let bg = if dlg.dark_mode {
                        if selected {
                            dlg.accent_blue
                        } else {
                            dlg.btn_bg
                        }
                    } else {
                        dlg.btn_bg
                    };
                    let mut rc = dis.rc_item;
                    unsafe {
                        FillRect(dis.hdc, &rc, bg);
                        FrameRect(dis.hdc, &rc, dlg.border);
                        SetBkMode(dis.hdc, TRANSPARENT);
                        SetTextColor(
                            dis.hdc,
                            if dlg.dark_mode {
                                CLR_DARK_TEXT
                            } else {
                                CLR_LIGHT_TEXT
                            },
                        );
                        SelectObject(dis.hdc, dlg.ui_font);
                        let txt = get_text(dis.hwnd_item);
                        let wtxt = to_wide(&txt);
                        DrawTextW(
                            dis.hdc,
                            wtxt.as_ptr(),
                            -1,
                            &mut rc,
                            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                        );
                    }
                    return 1;
                }
                0
            }
            WM_COMMAND => {
                if !ptr.is_null() {
                    let dlg = unsafe { &*ptr };
                    let id = wparam & 0xFFFF;
                    match id {
                        ID_SETUP_ADD_PATH => {
                            match crate::setup::win32_setup::install_path(&dlg.exe_path) {
                                Ok(added) => append_out(
                                    dlg.hwnd_studio_out,
                                    if added {
                                        "[Setup] Added Askane directory to User PATH.\n"
                                    } else {
                                        "[Setup] Directory already in User PATH.\n"
                                    },
                                ),
                                Err(e) => append_out(
                                    dlg.hwnd_studio_out,
                                    &format!("[Setup Error] {}\n", e),
                                ),
                            }
                            refresh_setup_dialog_label(dlg);
                        }
                        ID_SETUP_REM_PATH => {
                            match crate::setup::win32_setup::remove_path(&dlg.exe_path) {
                                Ok(rem) => append_out(
                                    dlg.hwnd_studio_out,
                                    if rem {
                                        "[Setup] Removed Askane directory from User PATH.\n"
                                    } else {
                                        "[Setup] Askane directory was not in User PATH.\n"
                                    },
                                ),
                                Err(e) => append_out(
                                    dlg.hwnd_studio_out,
                                    &format!("[Setup Error] {}\n", e),
                                ),
                            }
                            refresh_setup_dialog_label(dlg);
                        }
                        ID_SETUP_ASSOC_RUN => {
                            match crate::setup::win32_setup::install_assoc_with_default(
                                &dlg.exe_path,
                                false,
                            ) {
                                Ok(()) => append_out(
                                    dlg.hwnd_studio_out,
                                    "[Setup] Registered .ask files (default: Run with Askane).\n",
                                ),
                                Err(e) => append_out(
                                    dlg.hwnd_studio_out,
                                    &format!("[Setup Error] {}\n", e),
                                ),
                            }
                            refresh_setup_dialog_label(dlg);
                        }
                        ID_SETUP_ASSOC_STUDIO => {
                            match crate::setup::win32_setup::install_assoc_with_default(
                                &dlg.exe_path,
                                true,
                            ) {
                                Ok(()) => append_out(
                                    dlg.hwnd_studio_out,
                                    "[Setup] Registered .ask files (default: Edit in Askane Studio).\n",
                                ),
                                Err(e) => append_out(
                                    dlg.hwnd_studio_out,
                                    &format!("[Setup Error] {}\n", e),
                                ),
                            }
                            refresh_setup_dialog_label(dlg);
                        }
                        ID_SETUP_UNINSTALL => {
                            match crate::setup::win32_setup::uninstall_all(&dlg.exe_path) {
                                Ok((p, a)) => append_out(
                                    dlg.hwnd_studio_out,
                                    &format!(
                                        "[Setup] Removed PATH: {}, Removed .ask assoc: {}\n",
                                        p, a
                                    ),
                                ),
                                Err(e) => append_out(
                                    dlg.hwnd_studio_out,
                                    &format!("[Setup Error] {}\n", e),
                                ),
                            }
                            refresh_setup_dialog_label(dlg);
                        }
                        ID_SETUP_CLOSE => {
                            unsafe { DestroyWindow(hwnd) };
                        }
                        _ => {}
                    }
                }
                0
            }
            WM_CLOSE => {
                unsafe { DestroyWindow(hwnd) };
                0
            }
            WM_DESTROY => {
                if !ptr.is_null() {
                    let _ = unsafe { Box::from_raw(ptr) };
                    unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
                }
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    fn open_setup_dialog(parent: Hwnd, state: &GuiState) {
        let exe_path = match crate::setup::win32_setup::current_exe_clean() {
            Ok(p) => p,
            Err(e) => {
                append_out(state.hwnd_out, &format!("[Setup Error] {}\n", e));
                return;
            }
        };
        let h_inst = unsafe { GetModuleHandleW(null()) };
        let cls = to_wide("AskaneSetupDialog");
        let app_icon = unsafe { LoadIconW(h_inst, 1usize as *const u16) };
        let wc = WndClassExW {
            cb_size: std::mem::size_of::<WndClassExW>() as u32,
            style: 0,
            lpfn_wnd_proc: setup_dlg_proc,
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: h_inst,
            h_icon: app_icon,
            h_cursor: unsafe { LoadCursorW(null_mut(), 32512usize as *const u16) },
            hbr_background: (COLOR_WINDOW + 1) as Hbrush,
            lpsz_menu_name: null(),
            lpsz_class_name: cls.as_ptr(),
            h_icon_sm: app_icon,
        };
        unsafe { RegisterClassExW(&wc) };

        let title = to_wide("Askane Windows Integration Setup");
        let dlg_hwnd = unsafe {
            CreateWindowExW(
                0,
                cls.as_ptr(),
                title.as_ptr(),
                WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
                220,
                180,
                510,
                260,
                parent,
                null_mut(),
                h_inst,
                null_mut(),
            )
        };
        if dlg_hwnd.is_null() {
            return;
        }
        apply_dwm_dark_titlebar(dlg_hwnd, state.dark_mode);

        let edit_cls = to_wide("EDIT");
        let btn_cls = to_wide("BUTTON");
        let empty = to_wide("");
        let hwnd_lbl = unsafe {
            CreateWindowExW(
                0,
                edit_cls.as_ptr(),
                empty.as_ptr(),
                WS_CHILD | WS_VISIBLE | ES_MULTILINE | ES_READONLY | ES_AUTOHSCROLL,
                14,
                14,
                466,
                72,
                dlg_hwnd,
                ID_SETUP_LBL_INFO as Hmenu,
                h_inst,
                null_mut(),
            )
        };
        unsafe { SendMessageW(hwnd_lbl, WM_SETFONT, state.mono_font as Wparam, 1) };

        let mk = |lbl: &str, x: i32, y: i32, w: i32, id: usize| {
            let wt = to_wide(lbl);
            let h = unsafe {
                CreateWindowExW(
                    0,
                    btn_cls.as_ptr(),
                    wt.as_ptr(),
                    WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_OWNERDRAW,
                    x,
                    y,
                    w,
                    30,
                    dlg_hwnd,
                    id as Hmenu,
                    h_inst,
                    null_mut(),
                )
            };
            unsafe { SendMessageW(h, WM_SETFONT, state.ui_font as Wparam, 1) };
        };
        mk("Add to User PATH", 14, 100, 226, ID_SETUP_ADD_PATH);
        mk("Remove from PATH", 254, 100, 226, ID_SETUP_REM_PATH);
        mk(
            "Register .ask (Run Default)",
            14,
            140,
            226,
            ID_SETUP_ASSOC_RUN,
        );
        mk(
            "Register .ask (Studio Default)",
            254,
            140,
            226,
            ID_SETUP_ASSOC_STUDIO,
        );
        mk(
            "Uninstall All Integration",
            14,
            182,
            226,
            ID_SETUP_UNINSTALL,
        );
        mk("Close", 254, 182, 226, ID_SETUP_CLOSE);

        let dlg_state = Box::new(SetupDialogState {
            hwnd_lbl,
            hwnd_studio_out: state.hwnd_out,
            exe_path,
            dark_mode: state.dark_mode,
            win_bg: state.brushes.win_bg,
            editor_bg: state.brushes.editor_bg,
            btn_bg: if state.dark_mode {
                state.brushes.btn_bg
            } else {
                state.brushes.light_btn_bg
            },
            border: if state.dark_mode {
                state.brushes.border
            } else {
                state.brushes.light_border
            },
            accent_blue: state.brushes.accent_blue,
            ui_font: state.ui_font,
        });
        refresh_setup_dialog_label(&dlg_state);
        unsafe {
            SetWindowLongPtrW(dlg_hwnd, GWLP_USERDATA, Box::into_raw(dlg_state) as isize);
            RedrawWindow(
                dlg_hwnd,
                null(),
                null_mut(),
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN,
            );
            ShowWindow(dlg_hwnd, SW_SHOW);
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
            WM_ERASEBKGND => {
                if !state_ptr.is_null() {
                    let state = unsafe { &*state_ptr };
                    let mut rc = [0i32; 4];
                    unsafe {
                        GetClientRect(hwnd, &mut rc);
                        let br = if state.dark_mode {
                            state.brushes.win_bg
                        } else {
                            state.brushes.light_win_bg
                        };
                        FillRect(wparam as *mut c_void, &rc, br);
                    }
                    return 1;
                }
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            WM_PAINT => {
                if !state_ptr.is_null() {
                    let state = unsafe { &*state_ptr };
                    let mut ps = PaintStruct {
                        hdc: null_mut(),
                        f_erase: 0,
                        rc_paint: [0; 4],
                        f_restore: 0,
                        f_inc_update: 0,
                        rgb_reserved: [0; 32],
                    };
                    let hdc = unsafe { gui_begin_paint(hwnd, &mut ps) };
                    let border = if state.dark_mode {
                        state.brushes.border
                    } else {
                        state.brushes.light_border
                    };
                    frame_child_box(hwnd, hdc, state.hwnd_flags, border);
                    frame_child_box(hwnd, hdc, state.hwnd_tree, border);
                    frame_child_box(hwnd, hdc, state.hwnd_gutter, border);
                    frame_child_box(hwnd, hdc, state.hwnd_code, border);
                    frame_child_box(hwnd, hdc, state.hwnd_out, border);
                    frame_child_box(hwnd, hdc, state.hwnd_in, border);
                    if state.find_visible {
                        frame_child_box(hwnd, hdc, state.hwnd_find_box, border);
                        frame_child_box(hwnd, hdc, state.hwnd_repl_box, border);
                    }
                    unsafe { gui_end_paint(hwnd, &ps) };
                    return 0;
                }
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            WM_DRAWITEM => {
                if !state_ptr.is_null() {
                    let state = unsafe { &*state_ptr };
                    let dis = unsafe { &*(lparam as *const DrawItemStruct) };
                    draw_owner_button(state, dis);
                    return 1;
                }
                0
            }
            WM_SIZE => {
                if !state_ptr.is_null() {
                    let state = unsafe { &mut *state_ptr };
                    let w = (lparam & 0xFFFF) as i32;
                    let h = ((lparam >> 16) & 0xFFFF) as i32;
                    layout_controls(state, w, h);
                    update_gutter(state, false);
                    unsafe { InvalidateRect(hwnd, null(), 1) };
                }
                0
            }
            WM_TIMER => {
                if !state_ptr.is_null() && wparam == ID_TIMER_SYNC {
                    let state = unsafe { &mut *state_ptr };
                    update_gutter(state, false);
                }
                0
            }
            WM_COMMAND => {
                if !state_ptr.is_null() {
                    let state = unsafe { &mut *state_ptr };
                    let id = wparam & 0xFFFF;
                    let notif = (wparam >> 16) & 0xFFFF;

                    if id == ID_EDIT_CODE && (notif == EN_CHANGE || notif == EN_VSCROLL) {
                        if notif == EN_CHANGE && !state.ignore_change {
                            let was_dirty = state.tabs[state.active_tab].dirty;
                            state.tabs[state.active_tab].dirty = true;
                            if !was_dirty {
                                refresh_tabs_ui(state);
                            }
                        }
                        update_gutter(state, false);
                        update_status_and_title(state);
                        return 0;
                    }
                    if id == ID_EDIT_FLAGS && notif == EN_CHANGE {
                        update_status_and_title(state);
                        return 0;
                    }
                    if id == ID_LIST_TREE && (notif == LBN_DBLCLK || notif == LBN_SELCHANGE) {
                        let sel = unsafe { SendMessageW(state.hwnd_tree, LB_GETCURSEL, 0, 0) };
                        if sel >= 0 {
                            if let Some(item) = state.tree_items.get(sel as usize).cloned() {
                                if !item.is_dir {
                                    open_path_in_tab(state, item.path);
                                }
                            }
                        }
                        return 0;
                    }
                    if id >= ID_TAB_BASE && id < ID_TAB_BASE + MAX_TABS {
                        switch_to_tab(state, id - ID_TAB_BASE);
                        return 0;
                    }

                    match id {
                        ID_BTN_NEW => create_new_tab(state),
                        ID_BTN_OPEN => {
                            if let Some(p) = open_file_dialog(hwnd) {
                                open_path_in_tab(state, p);
                            }
                        }
                        ID_BTN_FOLDER => {
                            if let Some(dir) = browse_folder_dialog(hwnd) {
                                state.project_root = Some(dir);
                                populate_project_tree(state);
                                update_status_and_title(state);
                            }
                        }
                        ID_BTN_EXAMPLES => {
                            if let Some(ex) = locate_examples_dir() {
                                state.project_root = Some(ex);
                                populate_project_tree(state);
                                update_status_and_title(state);
                            } else {
                                append_out(state.hwnd_out, "[Could not locate examples/ folder]\n");
                            }
                        }
                        ID_BTN_REFRESH_TREE => populate_project_tree(state),
                        ID_BTN_SAVE => {
                            let idx = state.active_tab;
                            save_tab_at(hwnd, state, idx);
                        }
                        ID_BTN_CLOSE_TAB => close_active_tab(hwnd, state),
                        ID_BTN_RUN => run_script(hwnd, state),
                        ID_BTN_TEST => run_project_tests(hwnd, state),
                        ID_BTN_STOP => {
                            stop_child(state);
                            append_out(state.hwnd_out, "\n[Stopped by user]\n");
                        }
                        ID_BTN_CLEAR => set_text(state.hwnd_out, ""),
                        ID_BTN_SETUP => open_setup_dialog(hwnd, state),
                        ID_BTN_FMT => {
                            let raw = get_text(state.hwnd_code).replace("\r\n", "\n");
                            let formatted = crate::format_askane_source(&raw);
                            if formatted != raw {
                                state.ignore_change = true;
                                set_text(state.hwnd_code, &formatted);
                                state.ignore_change = false;
                                state.tabs[state.active_tab].content = formatted;
                                state.tabs[state.active_tab].dirty = true;
                                update_gutter(state, true);
                                refresh_tabs_ui(state);
                                update_status_and_title(state);
                            }
                            append_out(state.hwnd_out, "Formatted current buffer.\n");
                        }
                        ID_BTN_CHECK => {
                            let code = get_text(state.hwnd_code).replace("\r\n", "\n");
                            match crate::compile_source(&code) {
                                Ok(_) => set_text(
                                    state.hwnd_out,
                                    "Check OK: no syntax/compile errors.\n",
                                ),
                                Err(e) => {
                                    set_text(
                                        state.hwnd_out,
                                        &format!(
                                            "Check failed (double-click error line to jump):\n{}\n",
                                            crate::format_error_with_source(&code, &e)
                                        ),
                                    );
                                    if let Some((ln, col)) = parse_error_line_col(&e) {
                                        jump_code_to_line(state, ln, col);
                                    }
                                }
                            }
                        }
                        ID_BTN_FIND_TOGGLE => {
                            state.find_visible = !state.find_visible;
                            relayout_window(state);
                            if state.find_visible {
                                unsafe { SetFocus(state.hwnd_find_box) };
                            }
                        }
                        ID_BTN_FIND_CLOSE => {
                            state.find_visible = false;
                            relayout_window(state);
                        }
                        ID_BTN_FIND_NEXT => {
                            let needle = get_text(state.hwnd_find_box);
                            find_next_in_code(state, &needle);
                        }
                        ID_BTN_REPL_ONE => {
                            let needle = get_text(state.hwnd_find_box);
                            let repl = get_text(state.hwnd_repl_box);
                            replace_current_or_next(state, &needle, &repl);
                        }
                        ID_BTN_REPL_ALL => {
                            let needle = get_text(state.hwnd_find_box);
                            let repl = get_text(state.hwnd_repl_box);
                            replace_all_in_code(state, &needle, &repl);
                        }
                        ID_CHK_UNSAFE | ID_CHK_READ | ID_CHK_WRITE | ID_CHK_NET | ID_CHK_ENV
                        | ID_CHK_EXEC | ID_CHK_INPUT | ID_CHK_CLIP => {
                            update_status_and_title(state);
                        }
                        ID_BTN_SEND => {
                            let line = get_text(state.hwnd_in).replace(['\r', '\n'], "");
                            set_text(state.hwnd_in, "");
                            let guard = state.child_stdin.lock().unwrap();
                            if let Some(raw_in) = *guard {
                                append_out(state.hwnd_out, &format!("> {}\n", line));
                                let payload = format!("{}\n", line);
                                let mut written: u32 = 0;
                                unsafe {
                                    WriteFile(
                                        raw_in as Handle,
                                        payload.as_ptr(),
                                        payload.len() as u32,
                                        &mut written,
                                        null_mut(),
                                    );
                                }
                            } else {
                                append_out(
                                    state.hwnd_out,
                                    "[No running process to receive input]\n",
                                );
                            }
                        }
                        ID_CHK_DARK => {
                            state.dark_mode = is_checked(state.hwnd_chk_dark);
                            apply_theme_to_window(state);
                        }
                        _ => {}
                    }
                }
                0
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC | WM_CTLCOLORLISTBOX | WM_CTLCOLORBTN => {
                if !state_ptr.is_null() {
                    let state = unsafe { &*state_ptr };
                    let ctrl = lparam as Hwnd;
                    let hdc = wparam as *mut c_void;
                    let dark = state.dark_mode;
                    if ctrl == state.hwnd_gutter {
                        unsafe {
                            SetTextColor(
                                hdc,
                                if dark {
                                    CLR_DARK_MUTED
                                } else {
                                    CLR_LIGHT_MUTED
                                },
                            );
                            SetBkColor(
                                hdc,
                                if dark {
                                    CLR_DARK_GUTTER_BG
                                } else {
                                    CLR_LIGHT_GUTTER_BG
                                },
                            );
                        }
                        return (if dark {
                            state.brushes.gutter_bg
                        } else {
                            state.brushes.light_gutter_bg
                        }) as Lresult;
                    }
                    if ctrl == state.hwnd_code
                        || ctrl == state.hwnd_out
                        || ctrl == state.hwnd_in
                        || ctrl == state.hwnd_tree
                        || ctrl == state.hwnd_flags
                        || ctrl == state.hwnd_find_box
                        || ctrl == state.hwnd_repl_box
                    {
                        unsafe {
                            SetTextColor(hdc, if dark { CLR_DARK_TEXT } else { CLR_LIGHT_TEXT });
                            SetBkColor(
                                hdc,
                                if dark {
                                    CLR_DARK_EDITOR_BG
                                } else {
                                    CLR_LIGHT_EDITOR_BG
                                },
                            );
                        }
                        return (if dark {
                            state.brushes.editor_bg
                        } else {
                            state.brushes.light_editor_bg
                        }) as Lresult;
                    }
                    unsafe {
                        SetTextColor(hdc, if dark { CLR_DARK_TEXT } else { CLR_LIGHT_TEXT });
                        SetBkColor(
                            hdc,
                            if dark {
                                CLR_DARK_WIN_BG
                            } else {
                                CLR_LIGHT_WIN_BG
                            },
                        );
                    }
                    return (if dark {
                        state.brushes.win_bg
                    } else {
                        state.brushes.light_win_bg
                    }) as Lresult;
                }
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
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
            WM_CLOSE => {
                if !state_ptr.is_null() {
                    let state = unsafe { &mut *state_ptr };
                    sync_active_tab_content(state);
                    for i in 0..state.tabs.len() {
                        if state.tabs[i].dirty && !confirm_tab_close(hwnd, state, i) {
                            return 0;
                        }
                    }
                }
                unsafe { DestroyWindow(hwnd) };
                0
            }
            WM_DESTROY => {
                unsafe { KillTimer(hwnd, ID_TIMER_SYNC) };
                if !state_ptr.is_null() {
                    let state = unsafe { Box::from_raw(state_ptr) };
                    stop_child(&state);
                    unsafe {
                        if state.out_prev_proc != 0 {
                            SetWindowLongPtrW(state.hwnd_out, GWLP_WNDPROC, state.out_prev_proc);
                        }
                        DeleteObject(state.mono_font);
                        DeleteObject(state.ui_font);
                        state.brushes.destroy();
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
        let app_icon = unsafe { LoadIconW(h_inst, 1usize as *const u16) };
        let wc = WndClassExW {
            cb_size: std::mem::size_of::<WndClassExW>() as u32,
            style: 0,
            lpfn_wnd_proc: wnd_proc,
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: h_inst,
            h_icon: app_icon,
            h_cursor: unsafe { LoadCursorW(null_mut(), 32512usize as *const u16) },
            hbr_background: (COLOR_WINDOW + 1) as Hbrush,
            lpsz_menu_name: null(),
            lpsz_class_name: class_name.as_ptr(),
            h_icon_sm: app_icon,
        };
        if unsafe { RegisterClassExW(&wc) } == 0 {
            return Err("Failed to register GUI window class".to_string());
        }

        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                60,
                40,
                1180,
                760,
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
                14,
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
        let list_cls = to_wide("LISTBOX");
        let stat_cls = to_wide("STATIC");

        let mk_ctrl = |cls: &[u16],
                       text: &str,
                       x: i32,
                       y: i32,
                       w: i32,
                       h: i32,
                       id: usize,
                       style: u32,
                       font: Hfont|
         -> Hwnd {
            let wt = to_wide(text);
            let ctrl = unsafe {
                CreateWindowExW(
                    0,
                    cls.as_ptr(),
                    wt.as_ptr(),
                    WS_CHILD | WS_VISIBLE | style,
                    x,
                    y,
                    w,
                    h,
                    hwnd,
                    id as Hmenu,
                    h_inst,
                    null_mut(),
                )
            };
            unsafe { SendMessageW(ctrl, WM_SETFONT, font as Wparam, 1) };
            ctrl
        };

        // Row 1 toolbar (modern flat owner-drawn buttons)
        let mut bx = 10;
        let row1 = [
            ("New", 46, ID_BTN_NEW, BS_OWNERDRAW),
            ("Open", 50, ID_BTN_OPEN, BS_OWNERDRAW),
            ("Folder", 56, ID_BTN_FOLDER, BS_OWNERDRAW),
            ("Examples", 70, ID_BTN_EXAMPLES, BS_OWNERDRAW),
            ("Save", 50, ID_BTN_SAVE, BS_OWNERDRAW),
            ("Run", 50, ID_BTN_RUN, BS_OWNERDRAW),
            ("Test", 48, ID_BTN_TEST, BS_OWNERDRAW),
            ("Stop", 48, ID_BTN_STOP, BS_OWNERDRAW),
            ("Check", 54, ID_BTN_CHECK, BS_OWNERDRAW),
            ("Fmt", 44, ID_BTN_FMT, BS_OWNERDRAW),
            ("Find", 46, ID_BTN_FIND_TOGGLE, BS_OWNERDRAW),
            ("Clear", 50, ID_BTN_CLEAR, BS_OWNERDRAW),
            ("Dark", 50, ID_CHK_DARK, BS_AUTOCHECKBOX),
            ("Setup", 56, ID_BTN_SETUP, BS_OWNERDRAW),
        ];
        let mut hwnd_chk_dark = null_mut();
        for (lbl, bw, id, st) in row1 {
            let ctrl = mk_ctrl(&btn_cls, lbl, bx, 8, bw, 26, id, WS_TABSTOP | st, ui_font);
            if id == ID_CHK_DARK {
                hwnd_chk_dark = ctrl;
            }
            bx += bw + 4;
        }
        unsafe { SendMessageW(hwnd_chk_dark, BM_SETCHECK, BST_CHECKED as usize, 0) };

        // Row 2 permissions + flags bar
        let mut px = 10;
        let hwnd_chk_unsafe = mk_ctrl(
            &btn_cls,
            "Unsafe",
            px,
            38,
            60,
            22,
            ID_CHK_UNSAFE,
            WS_TABSTOP | BS_AUTOCHECKBOX,
            ui_font,
        );
        px += 62;
        let hwnd_chk_read = mk_ctrl(
            &btn_cls,
            "+Read",
            px,
            38,
            52,
            22,
            ID_CHK_READ,
            WS_TABSTOP | BS_AUTOCHECKBOX,
            ui_font,
        );
        px += 54;
        let hwnd_chk_write = mk_ctrl(
            &btn_cls,
            "+Write",
            px,
            38,
            56,
            22,
            ID_CHK_WRITE,
            WS_TABSTOP | BS_AUTOCHECKBOX,
            ui_font,
        );
        px += 58;
        let hwnd_chk_net = mk_ctrl(
            &btn_cls,
            "+Net",
            px,
            38,
            46,
            22,
            ID_CHK_NET,
            WS_TABSTOP | BS_AUTOCHECKBOX,
            ui_font,
        );
        px += 48;
        let hwnd_chk_env = mk_ctrl(
            &btn_cls,
            "+Env",
            px,
            38,
            48,
            22,
            ID_CHK_ENV,
            WS_TABSTOP | BS_AUTOCHECKBOX,
            ui_font,
        );
        px += 50;
        let hwnd_chk_exec = mk_ctrl(
            &btn_cls,
            "+Exec",
            px,
            38,
            52,
            22,
            ID_CHK_EXEC,
            WS_TABSTOP | BS_AUTOCHECKBOX,
            ui_font,
        );
        px += 54;
        let hwnd_chk_input = mk_ctrl(
            &btn_cls,
            "+Input",
            px,
            38,
            54,
            22,
            ID_CHK_INPUT,
            WS_TABSTOP | BS_AUTOCHECKBOX,
            ui_font,
        );
        px += 56;
        let hwnd_chk_clip = mk_ctrl(
            &btn_cls,
            "+Clip",
            px,
            38,
            50,
            22,
            ID_CHK_CLIP,
            WS_TABSTOP | BS_AUTOCHECKBOX,
            ui_font,
        );
        let hwnd_lbl_flags = mk_ctrl(
            &stat_cls,
            "Flags:",
            444,
            40,
            40,
            20,
            ID_LBL_FLAGS,
            0,
            ui_font,
        );
        let hwnd_flags = mk_ctrl(
            &edit_cls,
            "",
            486,
            38,
            400,
            22,
            ID_EDIT_FLAGS,
            WS_TABSTOP | ES_AUTOHSCROLL,
            ui_font,
        );

        // Project explorer sidebar
        let hwnd_tree_hdr = mk_ctrl(&stat_cls, "Project:", 10, 72, 160, 20, 900, 0, ui_font);
        let hwnd_refresh_tree = mk_ctrl(
            &btn_cls,
            "Reload",
            186,
            72,
            60,
            22,
            ID_BTN_REFRESH_TREE,
            WS_TABSTOP | BS_OWNERDRAW,
            ui_font,
        );
        let hwnd_tree = mk_ctrl(
            &list_cls,
            "",
            10,
            99,
            234,
            500,
            ID_LIST_TREE,
            WS_VSCROLL | WS_HSCROLL | WS_TABSTOP | LBS_NOTIFY | LBS_NOINTEGRALHEIGHT,
            ui_font,
        );

        // Find/Replace bar
        let hwnd_find_box = mk_ctrl(
            &edit_cls,
            "",
            256,
            72,
            180,
            22,
            ID_EDIT_FIND,
            WS_TABSTOP | ES_AUTOHSCROLL,
            ui_font,
        );
        let hwnd_repl_box = mk_ctrl(
            &edit_cls,
            "",
            444,
            72,
            180,
            22,
            ID_EDIT_REPL,
            WS_TABSTOP | ES_AUTOHSCROLL,
            ui_font,
        );
        let hwnd_find_next = mk_ctrl(
            &btn_cls,
            "Next",
            630,
            72,
            52,
            24,
            ID_BTN_FIND_NEXT,
            WS_TABSTOP | BS_OWNERDRAW,
            ui_font,
        );
        let hwnd_repl_one = mk_ctrl(
            &btn_cls,
            "Replace",
            686,
            72,
            60,
            24,
            ID_BTN_REPL_ONE,
            WS_TABSTOP | BS_OWNERDRAW,
            ui_font,
        );
        let hwnd_repl_all = mk_ctrl(
            &btn_cls,
            "All",
            750,
            72,
            42,
            24,
            ID_BTN_REPL_ALL,
            WS_TABSTOP | BS_OWNERDRAW,
            ui_font,
        );
        let hwnd_find_close = mk_ctrl(
            &btn_cls,
            "x",
            796,
            72,
            26,
            24,
            ID_BTN_FIND_CLOSE,
            WS_TABSTOP | BS_OWNERDRAW,
            ui_font,
        );

        // Tab buttons pool (owner-drawn)
        let mut tab_buttons = Vec::with_capacity(MAX_TABS);
        for i in 0..MAX_TABS {
            tab_buttons.push(mk_ctrl(
                &btn_cls,
                "",
                256,
                72,
                100,
                26,
                ID_TAB_BASE + i,
                BS_OWNERDRAW,
                ui_font,
            ));
        }
        let hwnd_close_tab = mk_ctrl(
            &btn_cls,
            "x",
            900,
            72,
            28,
            26,
            ID_BTN_CLOSE_TAB,
            BS_OWNERDRAW,
            ui_font,
        );

        // Gutter, code, output, stdin, status (without classic WS_BORDER; framed cleanly in WM_PAINT)
        let hwnd_gutter = mk_ctrl(
            &edit_cls,
            "1",
            257,
            101,
            46,
            340,
            ID_EDIT_GUTTER,
            ES_RIGHT | ES_MULTILINE | ES_READONLY,
            mono_font,
        );
        let hwnd_code = mk_ctrl(
            &edit_cls,
            "",
            304,
            101,
            780,
            340,
            ID_EDIT_CODE,
            WS_VSCROLL
                | WS_HSCROLL
                | WS_TABSTOP
                | ES_MULTILINE
                | ES_AUTOVSCROLL
                | ES_AUTOHSCROLL
                | ES_WANTRETURN,
            mono_font,
        );
        unsafe { SendMessageW(hwnd_code, EM_SETLIMITTEXT, 1_048_576, 0) };

        let hwnd_out = mk_ctrl(
            &edit_cls,
            "",
            257,
            448,
            828,
            180,
            ID_EDIT_OUT,
            WS_VSCROLL | WS_HSCROLL | ES_MULTILINE | ES_AUTOVSCROLL | ES_AUTOHSCROLL | ES_READONLY,
            mono_font,
        );
        unsafe { SendMessageW(hwnd_out, EM_SETLIMITTEXT, 1_048_576, 0) };
        let out_prev_proc = unsafe {
            SetWindowLongPtrW(hwnd_out, GWLP_USERDATA, hwnd as isize);
            SetWindowLongPtrW(
                hwnd_out,
                GWLP_WNDPROC,
                out_subclass_proc as *const () as isize,
            )
        };

        let hwnd_in = mk_ctrl(
            &edit_cls,
            "",
            257,
            636,
            750,
            22,
            ID_EDIT_IN,
            WS_TABSTOP | ES_AUTOHSCROLL,
            mono_font,
        );
        let hwnd_send = mk_ctrl(
            &btn_cls,
            "Send",
            1010,
            635,
            74,
            24,
            ID_BTN_SEND,
            WS_TABSTOP | BS_OWNERDRAW,
            ui_font,
        );
        let hwnd_status = mk_ctrl(
            &stat_cls,
            "Ready",
            10,
            670,
            1080,
            20,
            ID_LBL_STATUS,
            0,
            ui_font,
        );

        let initial_tab = if let Some(path) = initial_file {
            let p = PathBuf::from(path);
            let content = fs::read_to_string(&p)
                .unwrap_or_else(|_| "// Could not read initial file\n".to_string())
                .replace("\r\n", "\n");
            let title = p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "script.ask".to_string());
            EditorTab {
                path: Some(p),
                title,
                content,
                dirty: false,
            }
        } else {
            EditorTab {
                path: None,
                title: "scratch.ask".to_string(),
                content: "// Write Askane code or pick a file in the Project sidebar.\nprintln(\"Hello from Askane Studio v2!\");\n".to_string(),
                dirty: false,
            }
        };

        let project_root = initial_tab
            .path
            .as_ref()
            .map(|p| find_project_root_for(p))
            .or_else(|| {
                std::env::current_dir()
                    .ok()
                    .filter(|d| d.join("askane.toml").is_file() || d.join("examples").is_dir())
            });

        let brushes = ThemeBrushes::new();
        let mut state = Box::new(GuiState {
            hwnd_main: hwnd,
            hwnd_tree_hdr,
            hwnd_refresh_tree,
            hwnd_tree,
            hwnd_gutter,
            hwnd_code,
            hwnd_out,
            hwnd_in,
            hwnd_send,
            hwnd_status,
            hwnd_lbl_flags,
            hwnd_flags,
            hwnd_chk_unsafe,
            hwnd_chk_dark,
            hwnd_chk_read,
            hwnd_chk_write,
            hwnd_chk_net,
            hwnd_chk_env,
            hwnd_chk_exec,
            hwnd_chk_input,
            hwnd_chk_clip,
            hwnd_close_tab,
            tab_buttons,
            hwnd_find_box,
            hwnd_repl_box,
            hwnd_find_next,
            hwnd_repl_one,
            hwnd_repl_all,
            hwnd_find_close,
            find_visible: false,
            mono_font,
            ui_font,
            brushes,
            dark_mode: true,
            project_root,
            tree_items: Vec::new(),
            tabs: vec![initial_tab],
            active_tab: 0,
            ignore_change: false,
            last_line_count: 0,
            last_first_line: -1,
            out_prev_proc,
            child_proc: Arc::new(Mutex::new(None)),
            child_job: Arc::new(Mutex::new(None)),
            child_stdin: Arc::new(Mutex::new(None)),
        });

        populate_project_tree(&mut state);
        load_tab_into_editor(&mut state, 0);
        apply_theme_to_window(&state);

        let raw_ptr = Box::into_raw(state);
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw_ptr as isize);
            SetTimer(hwnd, ID_TIMER_SYNC, 80, null());
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
                } else if ctrl && msg.w_param == 0x4E {
                    unsafe { SendMessageW(hwnd, WM_COMMAND, ID_BTN_NEW, 0) };
                    continue;
                } else if ctrl && (msg.w_param == 0x46 || msg.w_param == 0x48) {
                    let st_ref = unsafe { &*raw_ptr };
                    if !st_ref.find_visible {
                        unsafe { SendMessageW(hwnd, WM_COMMAND, ID_BTN_FIND_TOGGLE, 0) };
                    } else {
                        unsafe { SetFocus(st_ref.hwnd_find_box) };
                    }
                    continue;
                } else if msg.w_param == VK_F3 {
                    unsafe { SendMessageW(hwnd, WM_COMMAND, ID_BTN_FIND_NEXT, 0) };
                    continue;
                } else if ctrl && msg.w_param == 0x41 && msg.hwnd == hwnd_code {
                    unsafe { SendMessageW(hwnd_code, EM_SETSEL, 0, -1) };
                    continue;
                } else if msg.w_param == VK_RETURN && msg.hwnd == hwnd_in {
                    unsafe { SendMessageW(hwnd, WM_COMMAND, ID_BTN_SEND, 0) };
                    continue;
                } else if msg.w_param == VK_RETURN && msg.hwnd == hwnd_find_box {
                    unsafe { SendMessageW(hwnd, WM_COMMAND, ID_BTN_FIND_NEXT, 0) };
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
