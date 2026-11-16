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
            ("text", gfx_text),
            ("rect_outline", gfx_rect_outline),
            ("circle_outline", gfx_circle_outline),
            ("load_image", gfx_load_image),
            ("draw_image", gfx_draw_image),
            ("draw_image_keyed", gfx_draw_image_keyed),
            ("image_width", gfx_image_width),
            ("image_height", gfx_image_height),
            ("set_title", gfx_set_title),
            ("delta", gfx_delta),
            ("present", gfx_present),
            ("key_down", gfx_key_down),
            ("mouse_x", gfx_mouse_x),
            ("mouse_y", gfx_mouse_y),
            ("mouse_down", gfx_mouse_down),
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
    const WM_PAINT: u32 = 0x000F;
    const WM_CLOSE: u32 = 0x0010;
    const DIB_RGB_COLORS: u32 = 0;
    const SRCCOPY: u32 = 0x00CC_0020;
    const TRANSPARENT: i32 = 1;

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct PaintStruct {
        hdc: Hdc,
        f_erase: i32,
        rc_paint: Rect,
        f_restore: i32,
        f_inc_update: i32,
        rgb_reserved: [u8; 32],
    }

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
        l_private: u32,
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
        fn BeginPaint(hWnd: Hwnd, lpPaint: *mut PaintStruct) -> Hdc;
        fn EndPaint(hWnd: Hwnd, lpPaint: *const PaintStruct) -> i32;
        fn GetCursorPos(lpPoint: *mut Point) -> i32;
        fn ScreenToClient(hWnd: Hwnd, lpPoint: *mut Point) -> i32;
        fn SetWindowTextW(hWnd: Hwnd, lpString: *const u16) -> i32;
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
        fn SetBkMode(hdc: Hdc, mode: i32) -> i32;
        fn SetTextColor(hdc: Hdc, color: u32) -> u32;
        fn TextOutW(hdc: Hdc, x: i32, y: i32, lpString: *const u16, c: i32) -> i32;
    }

    pub struct TextDraw {
        pub x: i32,
        pub y: i32,
        pub text: Vec<u16>,
        pub color_bgr: u32,
    }

    pub struct WindowState {
        pub hwnd: Hwnd,
        pub width: i32,
        pub height: i32,
        pub buffer: Vec<u32>,
        pub texts: Vec<TextDraw>,
        pub open: bool,
    }

    thread_local! {
        pub static WINDOW: RefCell<Option<WindowState>> = const { RefCell::new(None) };
    }

    unsafe fn paint_state_to_hdc(state: &WindowState, hdc: Hdc) {
        let bmi = BitmapInfo {
            bmi_header: BitmapInfoHeader {
                bi_size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                bi_width: state.width,
                bi_height: -state.height,
                bi_planes: 1,
                bi_bit_count: 32,
                bi_compression: 0,
                bi_size_image: 0,
                bi_x_pels_per_meter: 0,
                bi_y_pels_per_meter: 0,
                bi_clr_used: 0,
                bi_clr_important: 0,
            },
            bmi_colors: [0],
        };
        unsafe {
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
            SetBkMode(hdc, TRANSPARENT);
            for t in &state.texts {
                SetTextColor(hdc, t.color_bgr);
                TextOutW(hdc, t.x, t.y, t.text.as_ptr(), t.text.len() as i32);
            }
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: Hwnd,
        msg: u32,
        w_param: usize,
        l_param: isize,
    ) -> isize {
        match msg {
            WM_PAINT => {
                unsafe {
                    let mut ps: PaintStruct = std::mem::zeroed();
                    let hdc = BeginPaint(hwnd, &mut ps);
                    if !hdc.is_null() {
                        WINDOW.with(|w| {
                            if let Some(state) = w.borrow().as_ref() {
                                if state.hwnd == hwnd {
                                    paint_state_to_hdc(state, hdc);
                                }
                            }
                        });
                    }
                    EndPaint(hwnd, &ps);
                }
                0
            }
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
                    texts: Vec::new(),
                    open: true,
                });
            });
        }
        Ok(())
    }

    pub fn close_window() {
        WINDOW.with(|w| {
            let state = w.borrow_mut().take();
            if let Some(state) = state {
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
            unsafe {
                let hdc = GetDC(state.hwnd);
                if !hdc.is_null() {
                    paint_state_to_hdc(state, hdc);
                    ReleaseDC(state.hwnd, hdc);
                }
            }
            Ok(())
        })
    }

    pub fn mouse_pos() -> Result<(i32, i32), String> {
        WINDOW.with(|w| {
            let borrow = w.borrow();
            let state = borrow
                .as_ref()
                .ok_or_else(|| "No active gfx window".to_string())?;
            let mut pt = Point { x: 0, y: 0 };
            unsafe {
                GetCursorPos(&mut pt);
                ScreenToClient(state.hwnd, &mut pt);
            }
            Ok((pt.x, pt.y))
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

    pub struct BmpImage {
        pub width: i32,
        pub height: i32,
        pub pixels: Vec<u32>,
    }

    thread_local! {
        pub static IMAGES: RefCell<Vec<BmpImage>> = const { RefCell::new(Vec::new()) };
        pub static LAST_FRAME: RefCell<Option<std::time::Instant>> = const { RefCell::new(None) };
    }

    pub fn parse_bmp_safe(data: &[u8]) -> Result<BmpImage, String> {
        if data.len() < 54 || &data[0..2] != b"BM" {
            return Err("gfx.load_image(): invalid BMP header".to_string());
        }
        let offset = u32::from_le_bytes([data[10], data[11], data[12], data[13]]) as usize;
        let dib_size = u32::from_le_bytes([data[14], data[15], data[16], data[17]]);
        if dib_size < 40 {
            return Err("gfx.load_image(): unsupported BMP DIB header".to_string());
        }
        let width = i32::from_le_bytes([data[18], data[19], data[20], data[21]]);
        let raw_height = i32::from_le_bytes([data[22], data[23], data[24], data[25]]);
        let bpp = u16::from_le_bytes([data[28], data[29]]);
        let compression = u32::from_le_bytes([data[30], data[31], data[32], data[33]]);
        if compression != 0 || (bpp != 24 && bpp != 32) {
            return Err(
                "gfx.load_image(): only uncompressed 24-bit or 32-bit BMP is supported".to_string(),
            );
        }
        if width <= 0 || width > 4096 || raw_height == 0 || raw_height.unsigned_abs() > 4096 {
            return Err("gfx.load_image(): BMP dimensions out of 1..=4096 bounds".to_string());
        }
        let height = raw_height.abs();
        let top_down = raw_height < 0;
        let bytes_per_pixel = (bpp / 8) as usize;
        let row_bytes = (width as usize)
            .checked_mul(bytes_per_pixel)
            .ok_or_else(|| "BMP row overflow".to_string())?;
        let stride = (row_bytes + 3) & !3;
        let needed = offset
            .checked_add(
                stride
                    .checked_mul(height as usize)
                    .ok_or_else(|| "BMP size overflow".to_string())?,
            )
            .ok_or_else(|| "BMP offset overflow".to_string())?;
        if needed > data.len() {
            return Err("gfx.load_image(): truncated BMP pixel data".to_string());
        }
        let pixel_count = (width as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| "BMP pixel count overflow".to_string())?;
        let mut pixels = vec![0u32; pixel_count];
        for y in 0..height as usize {
            let src_y = if top_down {
                y
            } else {
                (height as usize - 1) - y
            };
            let row_start = offset + src_y * stride;
            for x in 0..width as usize {
                let p = row_start + x * bytes_per_pixel;
                let b = data[p] as u32;
                let g = data[p + 1] as u32;
                let r = data[p + 2] as u32;
                pixels[y * width as usize + x] = (r << 16) | (g << 8) | b;
            }
        }
        Ok(BmpImage {
            width,
            height,
            pixels,
        })
    }

    pub fn set_window_title(title: &str) -> Result<(), String> {
        WINDOW.with(|w| {
            let borrow = w.borrow();
            let state = borrow
                .as_ref()
                .ok_or_else(|| "No active gfx window".to_string())?;
            let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe {
                SetWindowTextW(state.hwnd, wide.as_ptr());
            }
            Ok(())
        })
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
        Value::Int(i) if (-32767..=32767).contains(i) => Ok(*i),
        Value::Float(f) if f.is_finite() && (-32767.0..=32767.0).contains(f) => Ok(*f as i64),
        Value::Int(_) | Value::Float(_) => Err(format!(
            "gfx.{}(): numeric argument must be finite and between -32767 and 32767",
            fn_name
        )),
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
        state.texts.clear();
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
        let x1 = x.saturating_add(rw).min(state.width);
        let y1 = y.saturating_add(rh).min(state.height);
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
fn gfx_text(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 6 {
        return Err("gfx.text() expects 6 arguments (x, y, text, r, g, b)".to_string());
    }
    let x = as_int(&args[0], "text")? as i32;
    let y = as_int(&args[1], "text")? as i32;
    let text = format!("{}", args[2]).encode_utf16().collect::<Vec<u16>>();
    let r = as_int(&args[3], "text")?.clamp(0, 255) as u32;
    let g = as_int(&args[4], "text")?.clamp(0, 255) as u32;
    let b = as_int(&args[5], "text")?.clamp(0, 255) as u32;
    let color_bgr = (b << 16) | (g << 8) | r;
    win32_gfx::WINDOW.with(|w| {
        let mut borrow = w.borrow_mut();
        let state = borrow
            .as_mut()
            .ok_or_else(|| "No active gfx window".to_string())?;
        state.texts.push(win32_gfx::TextDraw {
            x,
            y,
            text,
            color_bgr,
        });
        Ok(Value::Unit)
    })
}

#[cfg(windows)]
fn gfx_mouse_x(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.mouse_x() expects 0 arguments".to_string());
    }
    let (x, _) = win32_gfx::mouse_pos()?;
    Ok(Value::Int(x as i64))
}

#[cfg(windows)]
fn gfx_mouse_y(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.mouse_y() expects 0 arguments".to_string());
    }
    let (_, y) = win32_gfx::mouse_pos()?;
    Ok(Value::Int(y as i64))
}

#[cfg(windows)]
fn gfx_mouse_down(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(
            "gfx.mouse_down() expects 1 argument (\"left\" | \"right\" | \"middle\")".to_string(),
        );
    }
    let btn = match &args[0] {
        Value::Str(s) => s.to_lowercase(),
        other => {
            return Err(format!(
                "gfx.mouse_down() expects string button name, got '{}'",
                other.type_name()
            ));
        }
    };
    let vk = match btn.as_str() {
        "left" => 0x01,
        "right" => 0x02,
        "middle" => 0x04,
        _ => return Err(format!("Unknown mouse button '{}'", btn)),
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

#[cfg(windows)]
fn gfx_rect_outline(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 7 {
        return Err("gfx.rect_outline() expects 7 arguments (x, y, w, h, r, g, b)".to_string());
    }
    let x = as_int(&args[0], "rect_outline")? as i32;
    let y = as_int(&args[1], "rect_outline")? as i32;
    let rw = as_int(&args[2], "rect_outline")? as i32;
    let rh = as_int(&args[3], "rect_outline")? as i32;
    let color = rgb(
        as_int(&args[4], "rect_outline")?,
        as_int(&args[5], "rect_outline")?,
        as_int(&args[6], "rect_outline")?,
    );
    if rw <= 0 || rh <= 0 {
        return Ok(Value::Unit);
    }
    win32_gfx::WINDOW.with(|w| {
        let mut borrow = w.borrow_mut();
        let state = borrow
            .as_mut()
            .ok_or_else(|| "No active gfx window".to_string())?;
        let mut put = |px: i32, py: i32| {
            if px >= 0 && py >= 0 && px < state.width && py < state.height {
                let idx = (py * state.width + px) as usize;
                state.buffer[idx] = color;
            }
        };
        let x2 = x.saturating_add(rw - 1);
        let y2 = y.saturating_add(rh - 1);
        for px in x..=x2 {
            put(px, y);
            put(px, y2);
        }
        for py in y..=y2 {
            put(x, py);
            put(x2, py);
        }
        Ok(Value::Unit)
    })
}

#[cfg(windows)]
fn gfx_circle_outline(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 6 {
        return Err(
            "gfx.circle_outline() expects 6 arguments (cx, cy, radius, r, g, b)".to_string(),
        );
    }
    let cx = as_int(&args[0], "circle_outline")? as i32;
    let cy = as_int(&args[1], "circle_outline")? as i32;
    let radius = as_int(&args[2], "circle_outline")?.clamp(0, 4096) as i32;
    let color = rgb(
        as_int(&args[3], "circle_outline")?,
        as_int(&args[4], "circle_outline")?,
        as_int(&args[5], "circle_outline")?,
    );
    win32_gfx::WINDOW.with(|w| {
        let mut borrow = w.borrow_mut();
        let state = borrow
            .as_mut()
            .ok_or_else(|| "No active gfx window".to_string())?;
        let mut put = |px: i32, py: i32| {
            if px >= 0 && py >= 0 && px < state.width && py < state.height {
                let idx = (py * state.width + px) as usize;
                state.buffer[idx] = color;
            }
        };
        let mut x = radius;
        let mut y = 0;
        let mut err = 1 - radius;
        while x >= y {
            put(cx + x, cy + y);
            put(cx + y, cy + x);
            put(cx - y, cy + x);
            put(cx - x, cy + y);
            put(cx - x, cy - y);
            put(cx - y, cy - x);
            put(cx + y, cy - x);
            put(cx + x, cy - y);
            y += 1;
            if err < 0 {
                err += 2 * y + 1;
            } else {
                x -= 1;
                err += 2 * (y - x) + 1;
            }
        }
        Ok(Value::Unit)
    })
}

#[cfg(windows)]
fn gfx_load_image(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    use std::io::Read;
    if args.len() != 1 {
        return Err("gfx.load_image() expects 1 string argument (bmp path)".to_string());
    }
    let path = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "gfx.load_image() expects a string path, got '{}'",
                other.type_name()
            ));
        }
    };
    let resolved = vm.resolve_read_path(path)?;
    let file =
        std::fs::File::open(&resolved).map_err(|e| format!("gfx.load_image('{}'): {}", path, e))?;
    let mut bytes = Vec::new();
    const LIMIT: usize = 32 * 1024 * 1024;
    file.take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("gfx.load_image('{}'): {}", path, e))?;
    if bytes.len() > LIMIT {
        return Err("gfx.load_image(): BMP file exceeds 32 MiB limit".to_string());
    }
    let img = win32_gfx::parse_bmp_safe(&bytes)?;
    let id = win32_gfx::IMAGES.with(|imgs| {
        let mut list = imgs.borrow_mut();
        list.push(img);
        list.len() - 1
    });
    Ok(Value::Int(id as i64))
}

#[cfg(windows)]
fn draw_image_impl(img_id: i64, dx: i32, dy: i32, key: Option<u32>) -> Result<(), String> {
    if img_id < 0 {
        return Err("Invalid image handle".to_string());
    }
    win32_gfx::IMAGES.with(|imgs| {
        let list = imgs.borrow();
        let img = list
            .get(img_id as usize)
            .ok_or_else(|| "Invalid image handle".to_string())?;
        win32_gfx::WINDOW.with(|w| {
            let mut borrow = w.borrow_mut();
            let state = borrow
                .as_mut()
                .ok_or_else(|| "No active gfx window".to_string())?;
            for sy in 0..img.height {
                let py = dy.saturating_add(sy);
                if py < 0 || py >= state.height {
                    continue;
                }
                for sx in 0..img.width {
                    let px = dx.saturating_add(sx);
                    if px < 0 || px >= state.width {
                        continue;
                    }
                    let color = img.pixels[(sy * img.width + sx) as usize];
                    if key == Some(color) {
                        continue;
                    }
                    state.buffer[(py * state.width + px) as usize] = color;
                }
            }
            Ok(())
        })
    })
}

#[cfg(windows)]
fn gfx_draw_image(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("gfx.draw_image() expects 3 arguments (img, x, y)".to_string());
    }
    let id = as_int(&args[0], "draw_image")?;
    let x = as_int(&args[1], "draw_image")? as i32;
    let y = as_int(&args[2], "draw_image")? as i32;
    draw_image_impl(id, x, y, None)?;
    Ok(Value::Unit)
}

#[cfg(windows)]
fn gfx_draw_image_keyed(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 6 {
        return Err("gfx.draw_image_keyed() expects 6 arguments (img, x, y, r, g, b)".to_string());
    }
    let id = as_int(&args[0], "draw_image_keyed")?;
    let x = as_int(&args[1], "draw_image_keyed")? as i32;
    let y = as_int(&args[2], "draw_image_keyed")? as i32;
    let key = rgb(
        as_int(&args[3], "draw_image_keyed")?,
        as_int(&args[4], "draw_image_keyed")?,
        as_int(&args[5], "draw_image_keyed")?,
    );
    draw_image_impl(id, x, y, Some(key))?;
    Ok(Value::Unit)
}

#[cfg(windows)]
fn gfx_image_width(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.image_width() expects 1 argument (img)".to_string());
    }
    let id = as_int(&args[0], "image_width")?;
    win32_gfx::IMAGES.with(|imgs| {
        let list = imgs.borrow();
        let img = usize::try_from(id)
            .ok()
            .and_then(|idx| list.get(idx))
            .ok_or_else(|| "Invalid image handle".to_string())?;
        Ok(Value::Int(img.width as i64))
    })
}

#[cfg(windows)]
fn gfx_image_height(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.image_height() expects 1 argument (img)".to_string());
    }
    let id = as_int(&args[0], "image_height")?;
    win32_gfx::IMAGES.with(|imgs| {
        let list = imgs.borrow();
        let img = usize::try_from(id)
            .ok()
            .and_then(|idx| list.get(idx))
            .ok_or_else(|| "Invalid image handle".to_string())?;
        Ok(Value::Int(img.height as i64))
    })
}

#[cfg(windows)]
fn gfx_set_title(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.set_title() expects 1 string argument".to_string());
    }
    let title = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "gfx.set_title() expects a string, got '{}'",
                other.type_name()
            ));
        }
    };
    win32_gfx::set_window_title(title)?;
    Ok(Value::Unit)
}

#[cfg(windows)]
fn gfx_delta(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.delta() expects 0 arguments".to_string());
    }
    let now = std::time::Instant::now();
    let dt = win32_gfx::LAST_FRAME.with(|lf| {
        let mut slot = lf.borrow_mut();
        let dt = slot
            .map(|prev| now.duration_since(prev).as_secs_f64())
            .unwrap_or(0.0);
        *slot = Some(now);
        dt
    });
    Ok(Value::Float(dt))
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
#[cfg(not(windows))]
use unsupported as gfx_mouse_down;
#[cfg(not(windows))]
use unsupported as gfx_mouse_x;
#[cfg(not(windows))]
use unsupported as gfx_mouse_y;
#[cfg(not(windows))]
use unsupported as gfx_text;
#[cfg(not(windows))]
use unsupported as gfx_rect_outline;
#[cfg(not(windows))]
use unsupported as gfx_circle_outline;
#[cfg(not(windows))]
use unsupported as gfx_load_image;
#[cfg(not(windows))]
use unsupported as gfx_draw_image;
#[cfg(not(windows))]
use unsupported as gfx_draw_image_keyed;
#[cfg(not(windows))]
use unsupported as gfx_image_width;
#[cfg(not(windows))]
use unsupported as gfx_image_height;
#[cfg(not(windows))]
use unsupported as gfx_set_title;
#[cfg(not(windows))]
use unsupported as gfx_delta;
