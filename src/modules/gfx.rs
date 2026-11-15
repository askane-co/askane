use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "gfx",
        &[
            ("window", gfx_window),
            ("is_open", gfx_is_open),
            ("clear", gfx_clear),
            ("rect", gfx_rect),
            ("circle", gfx_circle),
            ("line", gfx_line),
            ("pixel", gfx_pixel),
            ("present", gfx_present),
            ("key_down", gfx_key_down),
            ("close", gfx_close),
        ],
        &[],
    )
}

#[cfg(windows)]
mod win32_gfx {
    use std::cell::RefCell;
    use std::ffi::c_void;

    pub type Hwnd = *mut c_void;
    pub type Hdc = *mut c_void;
    pub type HInstance = *mut c_void;

    const WS_OVERLAPPED: u32 = 0x0000_0000;
    const WS_CAPTION: u32 = 0x00C0_0000;
    const WS_SYSMENU: u32 = 0x0008_0000;
    const WS_MINIMIZEBOX: u32 = 0x0002_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const WS_WINDOW: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE;

    const CW_USEDEFAULT: i32 = -2147483648;
    const PM_REMOVE: u32 = 0x0001;
    const WM_DESTROY: u32 = 0x0002;
    const WM_CLOSE: u32 = 0x0010;
    const DIB_RGB_COLORS: u32 = 0;
    const SRCCOPY: u32 = 0x00CC_0020;

    #[repr(C)]
    struct WndClassExW {
        cb_size: u32,
        style: u32,
        lpfn_wnd_proc: unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize,
        cb_cls_extra: i32,
        cb_wnd_extra: i32,
        h_instance: HInstance,
        h_icon: *mut c_void,
        h_cursor: *mut c_void,
        hbr_background: *mut c_void,
        lpsz_menu_name: *const u16,
        lpsz_class_name: *const u16,
        h_icon_sm: *mut c_void,
    }

    #[repr(C)]
    struct Msg {
        hwnd: Hwnd,
        message: u32,
        w_param: usize,
        l_param: isize,
        time: u32,
        pt_x: i32,
        pt_y: i32,
    }

    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }

    #[repr(C)]
    struct BitmapInfoHeader {
        bi_size: u32,
        bi_width: i32,
        bi_height: i32,
        bi_planes: u16,
        bi_bit_count: u16,
        bi_compression: u32,
        bi_size_image: u32,
        bi_x_pels_per_meter: i32,
        bi_y_pels_per_meter: i32,
        bi_clr_used: u32,
        bi_clr_important: u32,
    }

    #[repr(C)]
    struct BitmapInfo {
        bmi_header: BitmapInfoHeader,
        bmi_colors: [u32; 1],
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
            hMenu: *mut c_void,
            hInstance: HInstance,
            lpParam: *mut c_void,
        ) -> Hwnd;
        fn DestroyWindow(hWnd: Hwnd) -> i32;
        fn DefWindowProcW(hWnd: Hwnd, Msg: u32, wParam: usize, lParam: isize) -> isize;
        fn PeekMessageW(
            lpMsg: *mut Msg,
            hWnd: Hwnd,
            wMsgFilterMin: u32,
            wMsgFilterMax: u32,
            wRemoveMsg: u32,
        ) -> i32;
        fn TranslateMessage(lpMsg: *const Msg) -> i32;
        fn DispatchMessageW(lpMsg: *const Msg) -> isize;
        fn AdjustWindowRect(lpRect: *mut Rect, dwStyle: u32, bMenu: i32) -> i32;
        fn GetDC(hWnd: Hwnd) -> Hdc;
        fn ReleaseDC(hWnd: Hwnd, hDC: Hdc) -> i32;
        fn GetAsyncKeyState(vKey: i32) -> i16;
        fn GetForegroundWindow() -> Hwnd;
        fn LoadCursorW(hInstance: HInstance, lpCursorName: *const u16) -> *mut c_void;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(lpModuleName: *const u16) -> HInstance;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn StretchDIBits(
            hdc: Hdc,
            xDest: i32,
            yDest: i32,
            DestWidth: i32,
            DestHeight: i32,
            xSrc: i32,
            ySrc: i32,
            SrcWidth: i32,
            SrcHeight: i32,
            lpBits: *const c_void,
            lpbmi: *const BitmapInfo,
            iUsage: u32,
            rop: u32,
        ) -> i32;
    }

    pub struct WindowState {
        pub hwnd: Hwnd,
        pub width: i32,
        pub height: i32,
        pub buffer: Vec<u32>,
        pub open: bool,
    }

    thread_local! {
        pub static WINDOW: RefCell<Option<WindowState>> = const { RefCell::new(None) };
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: Hwnd,
        msg: u32,
        w_param: usize,
        l_param: isize,
    ) -> isize {
        match msg {
            WM_CLOSE | WM_DESTROY => {
                WINDOW.with(|w| {
                    if let Some(state) = w.borrow_mut().as_mut() {
                        if state.hwnd == hwnd {
                            state.open = false;
                        }
                    }
                });
                if msg == WM_CLOSE {
                    unsafe {
                        DestroyWindow(hwnd);
                    }
                    return 0;
                }
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, w_param, l_param) },
        }
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn create_window(title: &str, width: i32, height: i32) -> Result<(), String> {
        close_window();

        let class_name = to_wide("AskaneGfxWindow");
        let title_w = to_wide(title);

        unsafe {
            let h_instance = GetModuleHandleW(std::ptr::null());
            let wc = WndClassExW {
                cb_size: std::mem::size_of::<WndClassExW>() as u32,
                style: 0,
                lpfn_wnd_proc: wnd_proc,
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance,
                h_icon: std::ptr::null_mut(),
                h_cursor: LoadCursorW(std::ptr::null_mut(), 32512 as *const u16),
                hbr_background: std::ptr::null_mut(),
                lpsz_menu_name: std::ptr::null(),
                lpsz_class_name: class_name.as_ptr(),
                h_icon_sm: std::ptr::null_mut(),
            };
            RegisterClassExW(&wc);

            let mut rect = Rect {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            AdjustWindowRect(&mut rect, WS_WINDOW, 0);

            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title_w.as_ptr(),
                WS_WINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                rect.right - rect.left,
                rect.bottom - rect.top,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                h_instance,
                std::ptr::null_mut(),
            );
            if hwnd.is_null() {
                return Err("Failed to create window".to_string());
            }

            let buffer = vec![0u32; (width * height) as usize];
            WINDOW.with(|w| {
                *w.borrow_mut() = Some(WindowState {
                    hwnd,
                    width,
                    height,
                    buffer,
                    open: true,
                });
            });
        }
        Ok(())
    }

    pub fn close_window() {
        WINDOW.with(|w| {
            if let Some(state) = w.borrow_mut().take() {
                if !state.hwnd.is_null() && state.open {
                    unsafe {
                        DestroyWindow(state.hwnd);
                    }
                }
            }
        });
    }

    pub fn pump_events() -> bool {
        unsafe {
            let mut msg: Msg = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        WINDOW.with(|w| w.borrow().as_ref().is_some_and(|s| s.open))
    }

    pub fn present() -> Result<(), String> {
        WINDOW.with(|w| {
            let borrow = w.borrow();
            let state = borrow
                .as_ref()
                .ok_or_else(|| "No active gfx window".to_string())?;
            if !state.open {
                return Ok(());
            }
            let bmi = BitmapInfo {
                bmi_header: BitmapInfoHeader {
                    bi_size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                    bi_width: state.width,
                    bi_height: -state.height, // top-down
                    bi_planes: 1,
                    bi_bit_count: 32,
                    bi_compression: 0, // BI_RGB
                    bi_size_image: 0,
                    bi_x_pels_per_meter: 0,
                    bi_y_pels_per_meter: 0,
                    bi_clr_used: 0,
                    bi_clr_important: 0,
                },
                bmi_colors: [0],
            };
            unsafe {
                let hdc = GetDC(state.hwnd);
                if !hdc.is_null() {
                    StretchDIBits(
                        hdc,
                        0,
                        0,
                        state.width,
                        state.height,
                        0,
                        0,
                        state.width,
                        state.height,
                        state.buffer.as_ptr() as *const c_void,
                        &bmi,
                        DIB_RGB_COLORS,
                        SRCCOPY,
                    );
                    ReleaseDC(state.hwnd, hdc);
                }
            }
            Ok(())
        })
    }

    pub fn key_down(vk: i32) -> bool {
        let focused = WINDOW.with(|w| {
            w.borrow()
                .as_ref()
                .is_some_and(|s| s.open && unsafe { GetForegroundWindow() == s.hwnd })
        });
        if !focused {
            return false;
        }
        unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
    }
}

#[cfg(windows)]
fn rgb(r: i64, g: i64, b: i64) -> u32 {
    let r = r.clamp(0, 255) as u32;
    let g = g.clamp(0, 255) as u32;
    let b = b.clamp(0, 255) as u32;
    (r << 16) | (g << 8) | b
}

#[cfg(windows)]
fn as_int(v: &Value, fn_name: &str) -> Result<i64, String> {
    match v {
        Value::Int(i) => Ok(*i),
        Value::Float(f) => Ok(*f as i64),
        other => Err(format!(
            "gfx.{}() expects integer, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

#[cfg(windows)]
fn gfx_window(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("gfx.window() expects 3 arguments (title, width, height)".to_string());
    }
    let title = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "gfx.window() expects string title, got '{}'",
                other.type_name()
            ));
        }
    };
    let w = as_int(&args[1], "window")?;
    let h = as_int(&args[2], "window")?;
    if !(1..=4096).contains(&w) || !(1..=4096).contains(&h) {
        return Err("gfx.window() width and height must be between 1 and 4096".to_string());
    }
    win32_gfx::create_window(title, w as i32, h as i32)?;
    Ok(Value::Unit)
}

#[cfg(windows)]
fn gfx_is_open(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.is_open() expects 0 arguments".to_string());
    }
    Ok(Value::Bool(win32_gfx::pump_events()))
}

#[cfg(windows)]
fn gfx_clear(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("gfx.clear() expects 3 arguments (r, g, b)".to_string());
    }
    let color = rgb(
        as_int(&args[0], "clear")?,
        as_int(&args[1], "clear")?,
        as_int(&args[2], "clear")?,
    );
    win32_gfx::WINDOW.with(|w| {
        let mut borrow = w.borrow_mut();
        let state = borrow
            .as_mut()
            .ok_or_else(|| "No active gfx window".to_string())?;
        state.buffer.fill(color);
        Ok(Value::Unit)
    })
}

#[cfg(windows)]
fn gfx_pixel(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 5 {
        return Err("gfx.pixel() expects 5 arguments (x, y, r, g, b)".to_string());
    }
    let x = as_int(&args[0], "pixel")? as i32;
    let y = as_int(&args[1], "pixel")? as i32;
    let color = rgb(
        as_int(&args[2], "pixel")?,
        as_int(&args[3], "pixel")?,
        as_int(&args[4], "pixel")?,
    );
    win32_gfx::WINDOW.with(|w| {
        let mut borrow = w.borrow_mut();
        let state = borrow
            .as_mut()
            .ok_or_else(|| "No active gfx window".to_string())?;
        if x >= 0 && y >= 0 && x < state.width && y < state.height {
            let idx = (y * state.width + x) as usize;
            state.buffer[idx] = color;
        }
        Ok(Value::Unit)
    })
}

#[cfg(windows)]
fn gfx_rect(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 7 {
        return Err("gfx.rect() expects 7 arguments (x, y, w, h, r, g, b)".to_string());
    }
    let x = as_int(&args[0], "rect")? as i32;
    let y = as_int(&args[1], "rect")? as i32;
    let rw = as_int(&args[2], "rect")? as i32;
    let rh = as_int(&args[3], "rect")? as i32;
    let color = rgb(
        as_int(&args[4], "rect")?,
        as_int(&args[5], "rect")?,
        as_int(&args[6], "rect")?,
    );
    win32_gfx::WINDOW.with(|w| {
        let mut borrow = w.borrow_mut();
        let state = borrow
            .as_mut()
            .ok_or_else(|| "No active gfx window".to_string())?;
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + rw).min(state.width);
        let y1 = (y + rh).min(state.height);
        for py in y0..y1 {
            let row = (py * state.width) as usize;
            for px in x0..x1 {
                state.buffer[row + px as usize] = color;
            }
        }
        Ok(Value::Unit)
    })
}

#[cfg(windows)]
fn gfx_circle(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 6 {
        return Err("gfx.circle() expects 6 arguments (cx, cy, radius, r, g, b)".to_string());
    }
    let cx = as_int(&args[0], "circle")? as i32;
    let cy = as_int(&args[1], "circle")? as i32;
    let rad = as_int(&args[2], "circle")?;
    if rad < 0 {
        return Ok(Value::Unit);
    }
    let rad = rad.min(4096) as i32;
    let color = rgb(
        as_int(&args[3], "circle")?,
        as_int(&args[4], "circle")?,
        as_int(&args[5], "circle")?,
    );
    win32_gfx::WINDOW.with(|w| {
        let mut borrow = w.borrow_mut();
        let state = borrow
            .as_mut()
            .ok_or_else(|| "No active gfx window".to_string())?;
        let x0 = (cx - rad).max(0);
        let y0 = (cy - rad).max(0);
        let x1 = (cx + rad).min(state.width - 1);
        let y1 = (cy + rad).min(state.height - 1);
        let r2 = rad * rad;
        for py in y0..=y1 {
            let dy = py - cy;
            let row = (py * state.width) as usize;
            for px in x0..=x1 {
                let dx = px - cx;
                if dx * dx + dy * dy <= r2 {
                    state.buffer[row + px as usize] = color;
                }
            }
        }
        Ok(Value::Unit)
    })
}

#[cfg(windows)]
fn gfx_line(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 7 {
        return Err("gfx.line() expects 7 arguments (x1, y1, x2, y2, r, g, b)".to_string());
    }
    let mut x0 = as_int(&args[0], "line")? as i32;
    let mut y0 = as_int(&args[1], "line")? as i32;
    let x1 = as_int(&args[2], "line")? as i32;
    let y1 = as_int(&args[3], "line")? as i32;
    let color = rgb(
        as_int(&args[4], "line")?,
        as_int(&args[5], "line")?,
        as_int(&args[6], "line")?,
    );
    win32_gfx::WINDOW.with(|w| {
        let mut borrow = w.borrow_mut();
        let state = borrow
            .as_mut()
            .ok_or_else(|| "No active gfx window".to_string())?;
        let dx = (x1 - x0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let dy = -(y1 - y0).abs();
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            if x0 >= 0 && y0 >= 0 && x0 < state.width && y0 < state.height {
                state.buffer[(y0 * state.width + x0) as usize] = color;
            }
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
        Ok(Value::Unit)
    })
}

#[cfg(windows)]
fn gfx_present(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.present() expects 0 arguments".to_string());
    }
    win32_gfx::present()?;
    Ok(Value::Unit)
}

#[cfg(windows)]
fn gfx_key_down(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.key_down() expects 1 argument (key_name)".to_string());
    }
    let name = match &args[0] {
        Value::Str(s) => s.to_uppercase(),
        other => {
            return Err(format!(
                "gfx.key_down() expects a string key name, got '{}'",
                other.type_name()
            ));
        }
    };
    let vk = match name.as_str() {
        "LEFT" => 0x25,
        "UP" => 0x26,
        "RIGHT" => 0x27,
        "DOWN" => 0x28,
        "SPACE" => 0x20,
        "ENTER" => 0x0D,
        "ESC" | "ESCAPE" => 0x1B,
        "SHIFT" => 0x10,
        "CTRL" => 0x11,
        s if s.len() == 1 => {
            let b = s.as_bytes()[0];
            if b.is_ascii_alphanumeric() {
                b as i32
            } else {
                return Err(format!("Unknown key '{}'", name));
            }
        }
        _ => return Err(format!("Unknown key '{}'", name)),
    };
    Ok(Value::Bool(win32_gfx::key_down(vk)))
}

#[cfg(windows)]
fn gfx_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.close() expects 0 arguments".to_string());
    }
    win32_gfx::close_window();
    Ok(Value::Unit)
}

#[cfg(not(windows))]
fn unsupported(_vm: &mut VM, _args: &[Value]) -> Result<Value, String> {
    Err("gfx module is currently only supported on Windows".to_string())
}

#[cfg(not(windows))]
use unsupported as gfx_window;
#[cfg(not(windows))]
use unsupported as gfx_is_open;
#[cfg(not(windows))]
use unsupported as gfx_clear;
#[cfg(not(windows))]
use unsupported as gfx_rect;
#[cfg(not(windows))]
use unsupported as gfx_circle;
#[cfg(not(windows))]
use unsupported as gfx_line;
#[cfg(not(windows))]
use unsupported as gfx_pixel;
#[cfg(not(windows))]
use unsupported as gfx_present;
#[cfg(not(windows))]
use unsupported as gfx_key_down;
#[cfg(not(windows))]
use unsupported as gfx_close;
