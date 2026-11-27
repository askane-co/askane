use crate::modules::build_module;
use crate::surface::ClipRect;
use crate::value::Value;
use crate::vm::VM;

pub fn reset_state() {
    gfx_state::close_window(true);
}

pub fn module() -> Value {
    build_module(
        "gfx",
        &[
            ("window", gfx_window),
            ("headless", gfx_headless),
            ("is_open", gfx_is_open),
            ("clear", gfx_clear),
            ("rect", gfx_rect),
            ("rect_alpha", gfx_rect_alpha),
            ("circle", gfx_circle),
            ("circle_alpha", gfx_circle_alpha),
            ("line", gfx_line),
            ("pixel", gfx_pixel),
            ("text", gfx_text),
            ("rect_outline", gfx_rect_outline),
            ("circle_outline", gfx_circle_outline),
            ("width", gfx_width),
            ("height", gfx_height),
            ("load_image", gfx_load_image),
            ("draw_image", gfx_draw_image),
            ("draw_image_keyed", gfx_draw_image_keyed),
            ("draw_sprite_frame", gfx_draw_sprite_frame),
            ("draw_sprite_ex", gfx_draw_sprite_ex),
            ("draw_atlas", gfx_draw_atlas),
            ("atlas_rect", gfx_atlas_rect),
            ("make_anim", gfx_make_anim),
            ("anim_update", gfx_anim_update),
            ("anim_draw", gfx_anim_draw),
            ("anim_frame", gfx_anim_frame),
            ("image_width", gfx_image_width),
            ("image_height", gfx_image_height),
            ("set_title", gfx_set_title),
            ("delta", gfx_delta),
            ("present", gfx_present),
            ("key_down", gfx_key_down),
            ("key_pressed", gfx_key_pressed),
            ("mouse_x", gfx_mouse_x),
            ("mouse_y", gfx_mouse_y),
            ("mouse_down", gfx_mouse_down),
            ("mouse_pressed", gfx_mouse_pressed),
            ("close", gfx_close),
            // Stage 2: Clipping, Surfaces, Layers, Camera2D, Bitmap & BMFont Text
            ("clip", gfx_clip),
            ("reset_clip", gfx_reset_clip),
            ("surface_new", gfx_surface_new),
            ("surface_free", gfx_surface_free),
            ("surface_width", gfx_surface_width),
            ("surface_height", gfx_surface_height),
            ("surface_from_image", gfx_surface_from_image),
            ("draw_to", gfx_draw_to),
            ("draw_surface", gfx_draw_surface),
            ("draw_surface_scaled", gfx_draw_surface_scaled),
            ("draw_surface_keyed", gfx_draw_surface_keyed),
            ("draw_surface_alpha", gfx_draw_surface_alpha),
            ("read_pixel", gfx_read_pixel),
            ("frame_hash", gfx_frame_hash),
            ("layer_new", gfx_layer_new),
            ("layer_free", gfx_layer_free),
            ("layer_mode", gfx_layer_mode),
            ("layer_visible", gfx_layer_visible),
            ("layer_z", gfx_layer_z),
            ("layer_parallax", gfx_layer_parallax),
            ("draw_to_layer", gfx_draw_to_layer),
            ("compose_layers", gfx_compose_layers),
            ("set_camera", gfx_set_camera),
            ("camera", gfx_camera),
            ("reset_camera", gfx_reset_camera),
            ("set_camera_shake", gfx_set_camera_shake),
            ("update_camera", gfx_update_camera),
            ("world_to_screen", gfx_world_to_screen),
            ("screen_to_world", gfx_screen_to_world),
            ("mouse_world", gfx_mouse_world),
            ("font_bitmap", gfx_font_bitmap),
            ("font_load_bmfont", gfx_font_load_bmfont),
            ("font_free", gfx_font_free),
            ("draw_text", gfx_draw_text),
            ("measure_text", gfx_measure_text),
            ("text_line_height", gfx_text_line_height),
            ("text_width", gfx_text_width),
        ],
        &[],
    )
}

mod gfx_state {
    use std::cell::RefCell;
    use std::sync::Arc;

    use crate::budget::BudgetCore;
    use crate::camera::Camera2D;
    use crate::font_atlas::FontSystem;
    use crate::layers::LayerBank;
    use crate::surface::{ClipRect, Surface};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ActiveTarget {
        Surface(usize),
        Layer(usize),
    }

    #[derive(Clone, Copy, Debug)]
    pub struct TargetFrame {
        pub target: ActiveTarget,
        pub camera: Camera2D,
    }

    #[allow(dead_code)]
    pub struct TextDraw {
        pub x: i32,
        pub y: i32,
        pub text: Vec<u16>,
        pub color_bgr: u32,
    }

    pub struct FrameState {
        pub width: i32,
        pub height: i32,
        pub buffer: Vec<u32>,
        pub texts: Vec<TextDraw>,
        pub clip: Option<ClipRect>,
        pub open: bool,
        pub headless: bool,
    }

    thread_local! {
        pub static FRAME: RefCell<Option<FrameState>> = const { RefCell::new(None) };
        pub static SURFACES: RefCell<Vec<Option<Surface>>> = const { RefCell::new(Vec::new()) };
        pub static LAYERS: RefCell<LayerBank> = RefCell::new(LayerBank::new());
        pub static TARGET_STACK: RefCell<Vec<TargetFrame>> = const { RefCell::new(Vec::new()) };
        pub static ROOT_CAMERA: RefCell<Camera2D> = RefCell::new(Camera2D::default());
        pub static FONTS: RefCell<FontSystem> = RefCell::new(FontSystem::new());
        pub static LAST_FRAME: RefCell<Option<std::time::Instant>> = const { RefCell::new(None) };
        pub static PREV_KEYS: RefCell<[bool; 256]> = const { RefCell::new([false; 256]) };
        pub static HEADLESS_ENABLED: RefCell<bool> = const { RefCell::new(false) };
        pub static ACTIVE_BUDGET: RefCell<Option<Arc<BudgetCore>>> = const { RefCell::new(None) };
    }

    pub fn bind_budget(core: &Arc<BudgetCore>) {
        ACTIVE_BUDGET.with(|slot| {
            *slot.borrow_mut() = Some(Arc::clone(core));
        });
    }

    pub fn with_budget_opt<R>(f: impl FnOnce(Option<&BudgetCore>) -> R) -> R {
        ACTIVE_BUDGET.with(|slot| f(slot.borrow().as_deref()))
    }

    pub fn current_camera() -> Camera2D {
        TARGET_STACK.with(|st| {
            let s = st.borrow();
            if let Some(top) = s.last() {
                top.camera
            } else {
                ROOT_CAMERA.with(|c| *c.borrow())
            }
        })
    }

    pub fn set_current_camera(cam: Camera2D) {
        TARGET_STACK.with(|st| {
            let mut s = st.borrow_mut();
            if let Some(top) = s.last_mut() {
                top.camera = cam;
            } else {
                ROOT_CAMERA.with(|c| *c.borrow_mut() = cam);
            }
        });
    }

    pub fn is_headless_enabled() -> bool {
        HEADLESS_ENABLED.with(|h| *h.borrow())
    }

    pub fn set_headless_enabled(v: bool) {
        HEADLESS_ENABLED.with(|h| *h.borrow_mut() = v);
    }

    pub fn open_headless(width: i32, height: i32) {
        close_window(false);
        let buffer = vec![0u32; (width * height) as usize];
        FRAME.with(|f| {
            *f.borrow_mut() = Some(FrameState {
                width,
                height,
                buffer,
                texts: Vec::new(),
                clip: None,
                open: true,
                headless: true,
            });
        });
    }

    pub fn close_window(reset_headless: bool) {
        FRAME.with(|f| {
            *f.borrow_mut() = None;
        });
        #[cfg(windows)]
        super::win32_gfx::destroy_native_window();
        TARGET_STACK.with(|st| st.borrow_mut().clear());
        ROOT_CAMERA.with(|c| c.borrow_mut().reset());
        LAST_FRAME.with(|lf| *lf.borrow_mut() = None);
        PREV_KEYS.with(|pk| *pk.borrow_mut() = [false; 256]);
        if reset_headless {
            with_budget_opt(|b| {
                SURFACES.with(|surfs| {
                    for slot in surfs.borrow_mut().iter_mut() {
                        if let Some(mut s) = slot.take() {
                            s.refund_to(b);
                        }
                    }
                    surfs.borrow_mut().clear();
                });
                LAYERS.with(|layers| layers.borrow_mut().clear_all(b));
                FONTS.with(|fonts| fonts.borrow_mut().clear_all(b));
            });
            ACTIVE_BUDGET.with(|slot| *slot.borrow_mut() = None);
            HEADLESS_ENABLED.with(|h| *h.borrow_mut() = false);
        }
    }

    pub fn is_frame_headless() -> Option<bool> {
        FRAME.with(|f| f.borrow().as_ref().map(|s| s.headless))
    }

    #[cfg(test)]
    pub fn peek_pixel(x: i32, y: i32) -> Option<u32> {
        FRAME.with(|f| {
            let borrow = f.borrow();
            let s = borrow.as_ref()?;
            if x < 0 || y < 0 || x >= s.width || y >= s.height {
                return None;
            }
            Some(s.buffer[(y * s.width + x) as usize])
        })
    }

    #[cfg(test)]
    pub fn text_count() -> usize {
        FRAME.with(|f| f.borrow().as_ref().map_or(0, |s| s.texts.len()))
    }
}

const SURFACE_ID_BASE: i64 = 10_000;
const LAYER_ID_BASE: i64 = 20_000;

pub(crate) fn value_dims(v: &Value) -> Option<(i32, i32)> {
    match v {
        Value::Image(im) => Some((im.width as i32, im.height as i32)),
        Value::Texture(t) => Some((t.image.width as i32, t.image.height as i32)),
        Value::Int(h) => handle_dims(*h),
        _ => None,
    }
}

pub(crate) fn handle_dims(handle: i64) -> Option<(i32, i32)> {
    if (SURFACE_ID_BASE..LAYER_ID_BASE).contains(&handle) {
        let slot = (handle - SURFACE_ID_BASE) as usize;
        gfx_state::SURFACES.with(|s| {
            s.borrow()
                .get(slot)
                .and_then(|o| o.as_ref())
                .map(|surf| (surf.width, surf.height))
        })
    } else if handle >= LAYER_ID_BASE {
        let slot = (handle - LAYER_ID_BASE) as usize;
        gfx_state::LAYERS.with(|l| l.borrow().get(slot).ok().map(|lay| (lay.surface.width, lay.surface.height)))
    } else {
        None
    }
}

pub(crate) fn copy_handle_rgba(handle: i64) -> Result<(i32, i32, Vec<u32>), String> {
    if (SURFACE_ID_BASE..LAYER_ID_BASE).contains(&handle) {
        let slot = (handle - SURFACE_ID_BASE) as usize;
        gfx_state::SURFACES.with(|surfs| {
            let list = surfs.borrow();
            let surf = list
                .get(slot)
                .and_then(|s| s.as_ref())
                .ok_or_else(|| format!("Invalid or freed surface handle {}", handle))?;
            Ok((surf.width, surf.height, surf.pixels.clone()))
        })
    } else if handle >= LAYER_ID_BASE {
        let slot = (handle - LAYER_ID_BASE) as usize;
        gfx_state::LAYERS.with(|layers| {
            let b = layers.borrow();
            let lay = b.get(slot)?;
            Ok((lay.surface.width, lay.surface.height, lay.surface.pixels.clone()))
        })
    } else {
        Err(format!("Invalid surface or layer handle {}", handle))
    }
}

#[cfg(test)]
pub(crate) fn test_peek_pixel(x: i32, y: i32) -> Option<u32> {
    gfx_state::peek_pixel(x, y)
}

#[cfg(test)]
pub(crate) fn test_text_count() -> usize {
    gfx_state::text_count()
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
        fn LoadIconW(hInstance: HInstance, lpIconName: *const u16) -> *mut c_void;
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

    thread_local! {
        pub static WIN32_HWND: RefCell<Hwnd> = const { RefCell::new(std::ptr::null_mut()) };
    }

    unsafe fn paint_state_to_hdc(state: &super::gfx_state::FrameState, hdc: Hdc) {
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
                        let matches = WIN32_HWND.with(|h| *h.borrow() == hwnd);
                        if matches {
                            super::gfx_state::FRAME.with(|w| {
                                if let Ok(borrow) = w.try_borrow() {
                                    if let Some(state) = borrow.as_ref() {
                                        if !state.headless {
                                            paint_state_to_hdc(state, hdc);
                                        }
                                    }
                                }
                            });
                        }
                    }
                    EndPaint(hwnd, &ps);
                }
                0
            }
            WM_CLOSE | WM_DESTROY => {
                let matches = WIN32_HWND.with(|h| *h.borrow() == hwnd);
                if matches {
                    super::gfx_state::FRAME.with(|w| {
                        if let Ok(mut borrow) = w.try_borrow_mut() {
                            if let Some(state) = borrow.as_mut() {
                                state.open = false;
                            }
                        }
                    });
                    if msg == WM_DESTROY {
                        WIN32_HWND.with(|h| *h.borrow_mut() = std::ptr::null_mut());
                    }
                }
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
        super::gfx_state::close_window(false);

        let class_name = to_wide("AskaneGfxWindow");
        let title_w = to_wide(title);

        unsafe {
            let h_instance = GetModuleHandleW(std::ptr::null());
            let app_icon = LoadIconW(h_instance, 1usize as *const u16);
            let wc = WndClassExW {
                cb_size: std::mem::size_of::<WndClassExW>() as u32,
                style: 0,
                lpfn_wnd_proc: wnd_proc,
                cb_cls_extra: 0,
                cb_wnd_extra: 0,
                h_instance,
                h_icon: app_icon,
                h_cursor: LoadCursorW(std::ptr::null_mut(), 32512 as *const u16),
                hbr_background: std::ptr::null_mut(),
                lpsz_menu_name: std::ptr::null(),
                lpsz_class_name: class_name.as_ptr(),
                h_icon_sm: app_icon,
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
            WIN32_HWND.with(|h| *h.borrow_mut() = hwnd);
            super::gfx_state::FRAME.with(|w| {
                *w.borrow_mut() = Some(super::gfx_state::FrameState {
                    width,
                    height,
                    buffer,
                    texts: Vec::new(),
                    clip: None,
                    open: true,
                    headless: false,
                });
            });
        }
        Ok(())
    }

    pub fn destroy_native_window() {
        let hwnd = WIN32_HWND.with(|h| {
            let old = *h.borrow();
            *h.borrow_mut() = std::ptr::null_mut();
            old
        });
        if !hwnd.is_null() {
            unsafe {
                DestroyWindow(hwnd);
            }
        }
    }

    pub fn pump_events() -> bool {
        unsafe {
            let mut msg: Msg = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        super::gfx_state::FRAME.with(|w| w.borrow().as_ref().is_some_and(|s| s.open))
    }

    pub fn present_native() -> Result<(), String> {
        let hwnd = WIN32_HWND.with(|h| *h.borrow());
        super::gfx_state::FRAME.with(|w| {
            let borrow = w.borrow();
            let state = borrow
                .as_ref()
                .ok_or_else(|| "No active gfx window".to_string())?;
            if !state.open || hwnd.is_null() {
                return Ok(());
            }
            unsafe {
                let hdc = GetDC(hwnd);
                if !hdc.is_null() {
                    paint_state_to_hdc(state, hdc);
                    ReleaseDC(hwnd, hdc);
                }
            }
            let focused = unsafe { GetForegroundWindow() == hwnd };
            super::gfx_state::PREV_KEYS.with(|pk| {
                let mut arr = pk.borrow_mut();
                if !focused {
                    *arr = [false; 256];
                } else {
                    for (vk, slot) in arr.iter_mut().enumerate().skip(1) {
                        *slot = unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 };
                    }
                }
            });
            Ok(())
        })
    }

    pub fn mouse_pos() -> Result<(i32, i32), String> {
        let hwnd = WIN32_HWND.with(|h| *h.borrow());
        if hwnd.is_null() {
            return Err("No active gfx window".to_string());
        }
        let mut pt = Point { x: 0, y: 0 };
        unsafe {
            GetCursorPos(&mut pt);
            ScreenToClient(hwnd, &mut pt);
        }
        Ok((pt.x, pt.y))
    }

    pub fn key_down(vk: i32) -> bool {
        let hwnd = WIN32_HWND.with(|h| *h.borrow());
        if hwnd.is_null() {
            return false;
        }
        let focused = super::gfx_state::FRAME.with(|w| {
            w.borrow()
                .as_ref()
                .is_some_and(|s| s.open && !s.headless && unsafe { GetForegroundWindow() == hwnd })
        });
        if !focused {
            return false;
        }
        unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
    }

    pub fn key_pressed(vk: i32) -> bool {
        if !(1..=254).contains(&vk) {
            return false;
        }
        key_down(vk) && !super::gfx_state::PREV_KEYS.with(|pk| pk.borrow()[vk as usize])
    }

    pub fn set_window_title(title: &str) -> Result<(), String> {
        let hwnd = WIN32_HWND.with(|h| *h.borrow());
        if hwnd.is_null() {
            return Err("No active gfx window".to_string());
        }
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            SetWindowTextW(hwnd, wide.as_ptr());
        }
        Ok(())
    }
}

fn rgb(r: i64, g: i64, b: i64) -> u32 {
    let r = r.clamp(0, 255) as u32;
    let g = g.clamp(0, 255) as u32;
    let b = b.clamp(0, 255) as u32;
    (r << 16) | (g << 8) | b
}

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

fn gfx_headless(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 && args.len() != 3 {
        return Err(
            "gfx.headless() expects 1 argument (enabled) or 3 arguments (enabled, width, height)"
                .to_string(),
        );
    }
    let enabled = match &args[0] {
        Value::Bool(b) => *b,
        other => {
            return Err(format!(
                "gfx.headless() expects boolean enabled flag, got '{}'",
                other.type_name()
            ));
        }
    };
    if !enabled {
        gfx_state::set_headless_enabled(false);
        if gfx_state::is_frame_headless() == Some(true) {
            gfx_state::close_window(true);
        }
        return Ok(Value::Unit);
    }
    let (w, h) = if args.len() == 3 {
        let w = as_int(&args[1], "headless")?;
        let h = as_int(&args[2], "headless")?;
        if !(1..=4096).contains(&w) || !(1..=4096).contains(&h) {
            return Err("gfx.headless() width and height must be between 1 and 4096".to_string());
        }
        (w as i32, h as i32)
    } else {
        (640, 480)
    };
    vm.charge_alloc(
        (w as usize)
            .saturating_mul(h as usize)
            .saturating_mul(std::mem::size_of::<u32>()),
    )?;
    gfx_state::bind_budget(vm.budget_core());
    gfx_state::set_headless_enabled(true);
    gfx_state::open_headless(w, h);
    Ok(Value::Unit)
}

fn gfx_window(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
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
    vm.charge_alloc(
        (w as usize)
            .saturating_mul(h as usize)
            .saturating_mul(std::mem::size_of::<u32>()),
    )?;
    gfx_state::bind_budget(vm.budget_core());
    if gfx_state::is_headless_enabled() {
        let _ = title;
        gfx_state::open_headless(w as i32, h as i32);
        return Ok(Value::Unit);
    }
    #[cfg(windows)]
    {
        win32_gfx::create_window(title, w as i32, h as i32)?;
        Ok(Value::Unit)
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        Err("gfx.window() requires Windows or gfx.headless(true)".to_string())
    }
}

fn gfx_is_open(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.is_open() expects 0 arguments".to_string());
    }
    match gfx_state::is_frame_headless() {
        Some(true) => Ok(Value::Bool(
            gfx_state::FRAME.with(|f| f.borrow().as_ref().is_some_and(|s| s.open)),
        )),
        Some(false) => {
            #[cfg(windows)]
            {
                Ok(Value::Bool(win32_gfx::pump_events()))
            }
            #[cfg(not(windows))]
            {
                Ok(Value::Bool(false))
            }
        }
        None => Ok(Value::Bool(false)),
    }
}

fn with_active_target<R>(
    f: impl FnOnce(&[u32], i32, i32, ClipRect, bool, &crate::camera::Camera2D) -> Result<R, String>,
) -> Result<R, String> {
    let top = gfx_state::TARGET_STACK.with(|st| st.borrow().last().copied());
    match top {
        Some(tf) => match tf.target {
            gfx_state::ActiveTarget::Surface(slot) => gfx_state::SURFACES.with(|surfs| {
                let list = surfs.borrow();
                let surf = list
                    .get(slot)
                    .and_then(|o| o.as_ref())
                    .ok_or_else(|| "Active target surface was freed".to_string())?;
                let clip = surf.effective_clip();
                f(&surf.pixels, surf.width, surf.height, clip, true, &tf.camera)
            }),
            gfx_state::ActiveTarget::Layer(slot) => gfx_state::LAYERS.with(|layers| {
                let bank = layers.borrow();
                let lay = bank.get(slot)?;
                let clip = lay.surface.effective_clip();
                f(
                    &lay.surface.pixels,
                    lay.surface.width,
                    lay.surface.height,
                    clip,
                    true,
                    &tf.camera,
                )
            }),
        },
        None => {
            let cam = gfx_state::ROOT_CAMERA.with(|c| *c.borrow());
            gfx_state::FRAME.with(|w| {
                let b = w.borrow();
                let st = b.as_ref().ok_or_else(|| "No active gfx window".to_string())?;
                let clip = crate::surface::effective_clip(st.width, st.height, st.clip);
                f(&st.buffer, st.width, st.height, clip, false, &cam)
            })
        }
    }
}

fn with_active_target_mut<R>(
    f: impl FnOnce(&mut [u32], i32, i32, ClipRect, bool, &crate::camera::Camera2D) -> Result<R, String>,
) -> Result<R, String> {
    let top = gfx_state::TARGET_STACK.with(|st| st.borrow().last().copied());
    match top {
        Some(tf) => match tf.target {
            gfx_state::ActiveTarget::Surface(slot) => gfx_state::SURFACES.with(|surfs| {
                let mut list = surfs.borrow_mut();
                let surf = list
                    .get_mut(slot)
                    .and_then(|o| o.as_mut())
                    .ok_or_else(|| "Active target surface was freed".to_string())?;
                let clip = surf.effective_clip();
                f(&mut surf.pixels, surf.width, surf.height, clip, true, &tf.camera)
            }),
            gfx_state::ActiveTarget::Layer(slot) => gfx_state::LAYERS.with(|layers| {
                let mut bank = layers.borrow_mut();
                let lay = bank.get_mut(slot)?;
                let clip = lay.surface.effective_clip();
                f(
                    &mut lay.surface.pixels,
                    lay.surface.width,
                    lay.surface.height,
                    clip,
                    true,
                    &tf.camera,
                )
            }),
        },
        None => {
            let cam = gfx_state::ROOT_CAMERA.with(|c| *c.borrow());
            gfx_state::FRAME.with(|w| {
                let mut b = w.borrow_mut();
                let st = b.as_mut().ok_or_else(|| "No active gfx window".to_string())?;
                let clip = crate::surface::effective_clip(st.width, st.height, st.clip);
                f(&mut st.buffer, st.width, st.height, clip, false, &cam)
            })
        }
    }
}

fn set_active_clip(clip: Option<ClipRect>) -> Result<(), String> {
    let top = gfx_state::TARGET_STACK.with(|st| st.borrow().last().copied());
    match top {
        Some(tf) => match tf.target {
            gfx_state::ActiveTarget::Surface(slot) => gfx_state::SURFACES.with(|surfs| {
                let mut list = surfs.borrow_mut();
                let surf = list
                    .get_mut(slot)
                    .and_then(|o| o.as_mut())
                    .ok_or_else(|| "Active target surface was freed".to_string())?;
                surf.clip = clip;
                Ok(())
            }),
            gfx_state::ActiveTarget::Layer(slot) => gfx_state::LAYERS.with(|layers| {
                let mut bank = layers.borrow_mut();
                let lay = bank.get_mut(slot)?;
                lay.surface.clip = clip;
                Ok(())
            }),
        },
        None => gfx_state::FRAME.with(|w| {
            let mut b = w.borrow_mut();
            let st = b.as_mut().ok_or_else(|| "No active gfx window".to_string())?;
            st.clip = clip;
            Ok(())
        }),
    }
}

fn is_target_active_on_stack(target: gfx_state::ActiveTarget) -> bool {
    gfx_state::TARGET_STACK.with(|st| st.borrow().iter().any(|tf| tf.target == target))
}

#[inline]
fn target_solid_color(rgb_col: u32, is_offscreen: bool) -> u32 {
    if is_offscreen {
        0xFF00_0000 | (rgb_col & 0x00FF_FFFF)
    } else {
        rgb_col & 0x00FF_FFFF
    }
}

fn gfx_clear(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("gfx.clear() expects 3 arguments (r, g, b)".to_string());
    }
    let color = rgb(
        as_int(&args[0], "clear")?,
        as_int(&args[1], "clear")?,
        as_int(&args[2], "clear")?,
    );
    let top = gfx_state::TARGET_STACK.with(|st| st.borrow().last().copied());
    match top {
        Some(tf) => match tf.target {
            gfx_state::ActiveTarget::Surface(slot) => gfx_state::SURFACES.with(|surfs| {
                let mut list = surfs.borrow_mut();
                let surf = list
                    .get_mut(slot)
                    .and_then(|o| o.as_mut())
                    .ok_or_else(|| "Active target surface was freed".to_string())?;
                surf.clear_rgba(0xFF00_0000 | color);
                Ok(Value::Unit)
            }),
            gfx_state::ActiveTarget::Layer(slot) => gfx_state::LAYERS.with(|layers| {
                let mut bank = layers.borrow_mut();
                let lay = bank.get_mut(slot)?;
                lay.surface.clear_rgba(0xFF00_0000 | color);
                Ok(Value::Unit)
            }),
        },
        None => gfx_state::FRAME.with(|w| {
            let mut borrow = w.borrow_mut();
            let state = borrow
                .as_mut()
                .ok_or_else(|| "No active gfx window".to_string())?;
            if let Some(clip) = state.clip {
                let eff = crate::surface::effective_clip(state.width, state.height, Some(clip));
                crate::surface::fill_rect_clipped(
                    &mut state.buffer,
                    state.width,
                    eff,
                    eff.x0,
                    eff.y0,
                    eff.x1 - eff.x0,
                    eff.y1 - eff.y0,
                    color,
                );
            } else {
                state.buffer.fill(color);
            }
            state.texts.clear();
            Ok(Value::Unit)
        }),
    }
}

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
    with_active_target_mut(|buf, w, h, clip, is_offscreen, cam| {
        let solid = target_solid_color(color, is_offscreen);
        let (px, py) = if cam.is_identity_rect() {
            (x, y)
        } else {
            let (sx, sy) = cam.world_to_screen(x as f64, y as f64, w, h);
            (sx.round() as i32, sy.round() as i32)
        };
        crate::surface::put_pixel_clipped(buf, w, clip, px, py, solid);
        Ok(Value::Unit)
    })
}

fn rect_fill_impl(
    x: i32,
    y: i32,
    rw: i32,
    rh: i32,
    color: u32,
    alpha: Option<u32>,
) -> Result<(), String> {
    with_active_target_mut(|buf, w, h, clip, is_offscreen, cam| {
        if rw <= 0 || rh <= 0 || alpha == Some(0) {
            return Ok(());
        }
        let preserve_dst_alpha = !is_offscreen;
        if cam.is_identity_rect() {
            match alpha.filter(|&a| a < 255) {
                Some(a) => crate::surface::fill_rect_alpha_clipped(
                    buf,
                    w,
                    clip,
                    x,
                    y,
                    rw,
                    rh,
                    color,
                    a,
                    preserve_dst_alpha,
                ),
                None => crate::surface::fill_rect_clipped(
                    buf,
                    w,
                    clip,
                    x,
                    y,
                    rw,
                    rh,
                    target_solid_color(color, is_offscreen),
                ),
            }
            return Ok(());
        }
        if cam.angle == 0.0 {
            let (sx, sy) = cam.world_to_screen(x as f64, y as f64, w, h);
            let sw = ((rw as f64) * cam.zoom).round().max(0.0) as i32;
            let sh = ((rh as f64) * cam.zoom).round().max(0.0) as i32;
            let dx = sx.round() as i32;
            let dy = sy.round() as i32;
            match alpha.filter(|&a| a < 255) {
                Some(a) => crate::surface::fill_rect_alpha_clipped(
                    buf,
                    w,
                    clip,
                    dx,
                    dy,
                    sw,
                    sh,
                    color,
                    a,
                    preserve_dst_alpha,
                ),
                None => crate::surface::fill_rect_clipped(
                    buf,
                    w,
                    clip,
                    dx,
                    dy,
                    sw,
                    sh,
                    target_solid_color(color, is_offscreen),
                ),
            }
            return Ok(());
        }
        let corners = [
            cam.world_to_screen(x as f64, y as f64, w, h),
            cam.world_to_screen((x + rw) as f64, y as f64, w, h),
            cam.world_to_screen(x as f64, (y + rh) as f64, w, h),
            cam.world_to_screen((x + rw) as f64, (y + rh) as f64, w, h),
        ];
        let min_x = (corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min).floor() as i32).max(clip.x0);
        let min_y = (corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min).floor() as i32).max(clip.y0);
        let max_x = (corners.iter().map(|c| c.0).fold(f64::NEG_INFINITY, f64::max).ceil() as i32).min(clip.x1);
        let max_y = (corners.iter().map(|c| c.1).fold(f64::NEG_INFINITY, f64::max).ceil() as i32).min(clip.y1);
        let a = alpha.unwrap_or(255);
        let wu = w as usize;
        for py in min_y..max_y {
            let row = (py as usize) * wu;
            for px in min_x..max_x {
                let (wx, wy) = cam.screen_to_world(px as f64 + 0.5, py as f64 + 0.5, w, h);
                if wx >= x as f64 && wx < (x + rw) as f64 && wy >= y as f64 && wy < (y + rh) as f64 {
                    let slot = &mut buf[row + px as usize];
                    *slot = crate::surface::blend_rgb_alpha_on(*slot, color, a, preserve_dst_alpha);
                }
            }
        }
        Ok(())
    })
}

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
    rect_fill_impl(x, y, rw, rh, color, None)?;
    Ok(Value::Unit)
}

fn gfx_rect_alpha(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 8 {
        return Err("gfx.rect_alpha() expects 8 arguments (x, y, w, h, r, g, b, a)".to_string());
    }
    let x = as_int(&args[0], "rect_alpha")? as i32;
    let y = as_int(&args[1], "rect_alpha")? as i32;
    let rw = as_int(&args[2], "rect_alpha")? as i32;
    let rh = as_int(&args[3], "rect_alpha")? as i32;
    let color = rgb(
        as_int(&args[4], "rect_alpha")?,
        as_int(&args[5], "rect_alpha")?,
        as_int(&args[6], "rect_alpha")?,
    );
    let alpha = as_int(&args[7], "rect_alpha")?.clamp(0, 255) as u32;
    rect_fill_impl(x, y, rw, rh, color, Some(alpha))?;
    Ok(Value::Unit)
}

fn circle_fill_impl(
    cx: i32,
    cy: i32,
    rad: i64,
    color: u32,
    alpha: Option<u32>,
) -> Result<(), String> {
    with_active_target_mut(|buf, w, h, clip, is_offscreen, cam| {
        if rad <= 0 || alpha == Some(0) {
            return Ok(());
        }
        let (scx, scy, srad) = if cam.is_identity_rect() {
            (cx, cy, rad.min(4096) as i32)
        } else {
            let (sx, sy) = cam.world_to_screen(cx as f64, cy as f64, w, h);
            let sr = ((rad.min(4096) as f64) * cam.zoom).round().clamp(0.0, 4096.0) as i32;
            (sx.round() as i32, sy.round() as i32, sr)
        };
        crate::surface::fill_circle_clipped(
            buf,
            w,
            clip,
            scx,
            scy,
            srad,
            color,
            alpha.unwrap_or(255),
            !is_offscreen,
        );
        Ok(())
    })
}

fn gfx_circle(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 6 {
        return Err("gfx.circle() expects 6 arguments (cx, cy, radius, r, g, b)".to_string());
    }
    let cx = as_int(&args[0], "circle")? as i32;
    let cy = as_int(&args[1], "circle")? as i32;
    let rad = as_int(&args[2], "circle")?;
    let color = rgb(
        as_int(&args[3], "circle")?,
        as_int(&args[4], "circle")?,
        as_int(&args[5], "circle")?,
    );
    circle_fill_impl(cx, cy, rad, color, None)?;
    Ok(Value::Unit)
}

fn gfx_circle_alpha(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 7 {
        return Err(
            "gfx.circle_alpha() expects 7 arguments (cx, cy, radius, r, g, b, a)".to_string(),
        );
    }
    let cx = as_int(&args[0], "circle_alpha")? as i32;
    let cy = as_int(&args[1], "circle_alpha")? as i32;
    let rad = as_int(&args[2], "circle_alpha")?;
    let color = rgb(
        as_int(&args[3], "circle_alpha")?,
        as_int(&args[4], "circle_alpha")?,
        as_int(&args[5], "circle_alpha")?,
    );
    let alpha = as_int(&args[6], "circle_alpha")?.clamp(0, 255) as u32;
    circle_fill_impl(cx, cy, rad, color, Some(alpha))?;
    Ok(Value::Unit)
}

fn gfx_width(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.width() expects 0 arguments".to_string());
    }
    with_active_target(|_buf, w, _h, _clip, _off, _cam| Ok(Value::Int(w as i64)))
}

fn gfx_height(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.height() expects 0 arguments".to_string());
    }
    with_active_target(|_buf, _w, h, _clip, _off, _cam| Ok(Value::Int(h as i64)))
}

fn gfx_line(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 7 {
        return Err("gfx.line() expects 7 arguments (x1, y1, x2, y2, r, g, b)".to_string());
    }
    let ax = as_int(&args[0], "line")? as i32;
    let ay = as_int(&args[1], "line")? as i32;
    let bx = as_int(&args[2], "line")? as i32;
    let by = as_int(&args[3], "line")? as i32;
    let color = rgb(
        as_int(&args[4], "line")?,
        as_int(&args[5], "line")?,
        as_int(&args[6], "line")?,
    );
    with_active_target_mut(|buf, w, h, clip, is_offscreen, cam| {
        let solid = target_solid_color(color, is_offscreen);
        let (x0, y0, x1, y1) = if cam.is_identity_rect() {
            (ax, ay, bx, by)
        } else {
            let (sx0, sy0) = cam.world_to_screen(ax as f64, ay as f64, w, h);
            let (sx1, sy1) = cam.world_to_screen(bx as f64, by as f64, w, h);
            (
                sx0.round() as i32,
                sy0.round() as i32,
                sx1.round() as i32,
                sy1.round() as i32,
            )
        };
        crate::surface::stroke_line_clipped(buf, w, clip, x0, y0, x1, y1, solid);
        Ok(Value::Unit)
    })
}

fn gfx_present(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.present() expects 0 arguments".to_string());
    }
    let dt_opt = if vm.is_deterministic() {
        Some(1.0 / 60.0)
    } else {
        None
    };
    gfx_state::TARGET_STACK.with(|st| {
        if let Some(top) = st.borrow_mut().last_mut() {
            if let Some(dt) = dt_opt {
                top.camera.tick_shake(dt);
            }
        } else {
            gfx_state::ROOT_CAMERA.with(|c| {
                let mut cam = c.borrow_mut();
                if let Some(dt) = dt_opt {
                    cam.tick_shake(dt);
                }
            });
        }
    });
    match gfx_state::is_frame_headless() {
        Some(true) => {
            gfx_state::PREV_KEYS.with(|pk| *pk.borrow_mut() = [false; 256]);
            Ok(Value::Unit)
        }
        Some(false) => {
            #[cfg(windows)]
            {
                win32_gfx::present_native()?;
                Ok(Value::Unit)
            }
            #[cfg(not(windows))]
            {
                Err("No active gfx window".to_string())
            }
        }
        None => Err("No active gfx window".to_string()),
    }
}

fn parse_vk_name(arg: &Value, ctx: &str) -> Result<i32, String> {
    let name = match arg {
        Value::Str(s) => s.to_uppercase(),
        other => {
            return Err(format!(
                "gfx.{}() expects a string key name, got '{}'",
                ctx,
                other.type_name()
            ));
        }
    };
    match name.as_str() {
        "LEFT" => Ok(0x25),
        "UP" => Ok(0x26),
        "RIGHT" => Ok(0x27),
        "DOWN" => Ok(0x28),
        "SPACE" => Ok(0x20),
        "ENTER" => Ok(0x0D),
        "TAB" => Ok(0x09),
        "BACKSPACE" => Ok(0x08),
        "ESC" | "ESCAPE" => Ok(0x1B),
        "SHIFT" => Ok(0x10),
        "CTRL" => Ok(0x11),
        s if s.len() == 1 => {
            let b = s.as_bytes()[0];
            if b.is_ascii_alphanumeric() {
                Ok(b as i32)
            } else {
                Err(format!("Unknown key '{}'", name))
            }
        }
        _ => Err(format!("Unknown key '{}'", name)),
    }
}

fn gfx_key_down(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.key_down() expects 1 argument (key_name)".to_string());
    }
    let vk = parse_vk_name(&args[0], "key_down")?;
    match gfx_state::is_frame_headless() {
        Some(true) => {
            let _ = vk;
            Ok(Value::Bool(false))
        }
        Some(false) => {
            #[cfg(windows)]
            {
                Ok(Value::Bool(win32_gfx::key_down(vk)))
            }
            #[cfg(not(windows))]
            {
                let _ = vk;
                Ok(Value::Bool(false))
            }
        }
        None => Err("No active gfx window".to_string()),
    }
}

fn gfx_key_pressed(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.key_pressed() expects 1 argument (key_name)".to_string());
    }
    let vk = parse_vk_name(&args[0], "key_pressed")?;
    match gfx_state::is_frame_headless() {
        Some(true) => {
            let _ = vk;
            Ok(Value::Bool(false))
        }
        Some(false) => {
            #[cfg(windows)]
            {
                Ok(Value::Bool(win32_gfx::key_pressed(vk)))
            }
            #[cfg(not(windows))]
            {
                let _ = vk;
                Ok(Value::Bool(false))
            }
        }
        None => Err("No active gfx window".to_string()),
    }
}

fn gfx_text(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 6 {
        return Err("gfx.text() expects 6 arguments (x, y, text, r, g, b)".to_string());
    }
    let x = as_int(&args[0], "text")? as i32;
    let y = as_int(&args[1], "text")? as i32;
    let mut text = format!("{}", args[2]).encode_utf16().collect::<Vec<u16>>();
    if text.len() > 4096 {
        text.truncate(4096);
    }
    let r = as_int(&args[3], "text")?.clamp(0, 255) as u32;
    let g = as_int(&args[4], "text")?.clamp(0, 255) as u32;
    let b = as_int(&args[5], "text")?.clamp(0, 255) as u32;
    let color_bgr = (b << 16) | (g << 8) | r;
    if !gfx_state::TARGET_STACK.with(|st| st.borrow().is_empty()) {
        return Err(
            "gfx.text() only targets the main window; use gfx.draw_text() for surfaces/layers"
                .to_string(),
        );
    }
    gfx_state::FRAME.with(|w| {
        let mut borrow = w.borrow_mut();
        let state = borrow
            .as_mut()
            .ok_or_else(|| "No active gfx window".to_string())?;
        if state.texts.len() >= 4096 {
            return Err("gfx.text(): maximum per-frame text count (4096) exceeded".to_string());
        }
        state.texts.push(gfx_state::TextDraw {
            x,
            y,
            text,
            color_bgr,
        });
        Ok(Value::Unit)
    })
}

fn gfx_mouse_x(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.mouse_x() expects 0 arguments".to_string());
    }
    match gfx_state::is_frame_headless() {
        Some(true) => Ok(Value::Int(0)),
        Some(false) => {
            #[cfg(windows)]
            {
                let (x, _) = win32_gfx::mouse_pos()?;
                Ok(Value::Int(x as i64))
            }
            #[cfg(not(windows))]
            {
                Ok(Value::Int(0))
            }
        }
        None => Err("No active gfx window".to_string()),
    }
}

fn gfx_mouse_y(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.mouse_y() expects 0 arguments".to_string());
    }
    match gfx_state::is_frame_headless() {
        Some(true) => Ok(Value::Int(0)),
        Some(false) => {
            #[cfg(windows)]
            {
                let (_, y) = win32_gfx::mouse_pos()?;
                Ok(Value::Int(y as i64))
            }
            #[cfg(not(windows))]
            {
                Ok(Value::Int(0))
            }
        }
        None => Err("No active gfx window".to_string()),
    }
}

fn parse_mouse_vk(arg: &Value, fn_name: &str) -> Result<i32, String> {
    let btn = match arg {
        Value::Str(s) => s.to_lowercase(),
        other => {
            return Err(format!(
                "gfx.{}() expects string button name, got '{}'",
                fn_name,
                other.type_name()
            ));
        }
    };
    match btn.as_str() {
        "left" => Ok(0x01),
        "right" => Ok(0x02),
        "middle" => Ok(0x04),
        _ => Err(format!("Unknown mouse button '{}'", btn)),
    }
}

fn gfx_mouse_down(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(
            "gfx.mouse_down() expects 1 argument (\"left\" | \"right\" | \"middle\")".to_string(),
        );
    }
    let vk = parse_mouse_vk(&args[0], "mouse_down")?;
    match gfx_state::is_frame_headless() {
        Some(true) => {
            let _ = vk;
            Ok(Value::Bool(false))
        }
        Some(false) => {
            #[cfg(windows)]
            {
                Ok(Value::Bool(win32_gfx::key_down(vk)))
            }
            #[cfg(not(windows))]
            {
                let _ = vk;
                Ok(Value::Bool(false))
            }
        }
        None => Err("No active gfx window".to_string()),
    }
}

fn gfx_mouse_pressed(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err(
            "gfx.mouse_pressed() expects 1 argument (\"left\" | \"right\" | \"middle\")"
                .to_string(),
        );
    }
    let vk = parse_mouse_vk(&args[0], "mouse_pressed")?;
    match gfx_state::is_frame_headless() {
        Some(true) => {
            let _ = vk;
            Ok(Value::Bool(false))
        }
        Some(false) => {
            #[cfg(windows)]
            {
                Ok(Value::Bool(win32_gfx::key_pressed(vk)))
            }
            #[cfg(not(windows))]
            {
                let _ = vk;
                Ok(Value::Bool(false))
            }
        }
        None => Err("No active gfx window".to_string()),
    }
}

fn gfx_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.close() expects 0 arguments".to_string());
    }
    gfx_state::close_window(false);
    Ok(Value::Unit)
}

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
    with_active_target_mut(|buf, w, _h, clip, is_offscreen, cam| {
        if rw <= 0 || rh <= 0 {
            return Ok(Value::Unit);
        }
        let solid = target_solid_color(color, is_offscreen);
        let (dx, dy, dw, dh) = if cam.is_identity_rect() {
            (x, y, rw, rh)
        } else {
            let (sx, sy) = cam.world_to_screen(x as f64, y as f64, w, _h);
            (
                sx.round() as i32,
                sy.round() as i32,
                ((rw as f64) * cam.zoom).round().max(1.0) as i32,
                ((rh as f64) * cam.zoom).round().max(1.0) as i32,
            )
        };
        let x2 = dx.saturating_add(dw - 1);
        let y2 = dy.saturating_add(dh - 1);
        for px in dx..=x2 {
            crate::surface::put_pixel_clipped(buf, w, clip, px, dy, solid);
            crate::surface::put_pixel_clipped(buf, w, clip, px, y2, solid);
        }
        for py in dy..=y2 {
            crate::surface::put_pixel_clipped(buf, w, clip, dx, py, solid);
            crate::surface::put_pixel_clipped(buf, w, clip, x2, py, solid);
        }
        Ok(Value::Unit)
    })
}

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
    with_active_target_mut(|buf, w, h, clip, is_offscreen, cam| {
        let solid = target_solid_color(color, is_offscreen);
        let (scx, scy, srad) = if cam.is_identity_rect() {
            (cx, cy, radius)
        } else {
            let (sx, sy) = cam.world_to_screen(cx as f64, cy as f64, w, h);
            let sr = ((radius as f64) * cam.zoom).round().clamp(0.0, 4096.0) as i32;
            (sx.round() as i32, sy.round() as i32, sr)
        };
        let mut x = srad;
        let mut y = 0;
        let mut err = 1 - srad;
        while x >= y {
            crate::surface::put_pixel_clipped(buf, w, clip, scx + x, scy + y, solid);
            crate::surface::put_pixel_clipped(buf, w, clip, scx + y, scy + x, solid);
            crate::surface::put_pixel_clipped(buf, w, clip, scx - y, scy + x, solid);
            crate::surface::put_pixel_clipped(buf, w, clip, scx - x, scy + y, solid);
            crate::surface::put_pixel_clipped(buf, w, clip, scx - x, scy - y, solid);
            crate::surface::put_pixel_clipped(buf, w, clip, scx - y, scy - x, solid);
            crate::surface::put_pixel_clipped(buf, w, clip, scx + y, scy - x, solid);
            crate::surface::put_pixel_clipped(buf, w, clip, scx + x, scy - y, solid);
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

fn gfx_load_image(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("gfx.load_image() expects (path, [opts])".to_string());
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
    let opts = crate::media::image::parse_decode_opts(args.get(1), "gfx.load_image()")?;
    let (img, _) = crate::media::cache::load_image_cached(vm, path, &opts, false)?;
    Ok(Value::Image(img))
}

fn blit_to_active_target(
    img: &crate::media::image::MediaImage,
    world_x: f64,
    world_y: f64,
    mut opts: crate::media::compose::BlitOpts,
) -> Result<(), String> {
    with_active_target_mut(|buf, w, h, clip, is_offscreen, cam| {
        if cam.is_identity_rect() {
            opts.dst_x = world_x;
            opts.dst_y = world_y;
        } else {
            let (sx, sy) = cam.world_to_screen(world_x, world_y, w, h);
            opts.dst_x = sx;
            opts.dst_y = sy;
            opts.scale_x *= cam.zoom;
            opts.scale_y *= cam.zoom;
        }
        let mut target = crate::media::compose::BlitTarget {
            pixels: buf,
            width: w,
            height: h,
            clip: Some(clip),
            is_offscreen,
        };
        crate::media::compose::blit_image(&mut target, img, &opts);
        Ok(())
    })
}

fn parse_tint_rgb(v: &Value, ctx: &str) -> Result<u32, String> {
    match v {
        Value::Int(n) => Ok((*n as u32) & 0x00FF_FFFF),
        Value::Str(s) => {
            let rgb = crate::modules::ui::parse_color_str(
                s,
                &crate::modules::ui::Theme::light(),
            )?;
            Ok(rgb & 0x00FF_FFFF)
        }
        Value::List(l) => {
            let b = l.borrow();
            if b.len() < 3 || b.len() > 4 {
                return Err(crate::media::err_bad_opt(format!("{} tint list must have 3 or 4 items", ctx)));
            }
            let ch = |idx: usize| -> Result<u32, String> {
                match b[idx] {
                    Value::Int(i) if (0..=255).contains(&i) => Ok(i as u32),
                    _ => Err(crate::media::err_bad_opt("tint channel must be int 0..=255")),
                }
            };
            Ok((ch(0)? << 16) | (ch(1)? << 8) | ch(2)?)
        }
        other => Err(crate::media::err_bad_opt(format!(
            "{} tint expects int/str/[r,g,b], got '{}'",
            ctx,
            other.type_name()
        ))),
    }
}

fn apply_sprite_ex_opts(
    opts_val: Option<&Value>,
    opts: &mut crate::media::compose::BlitOpts,
    ctx: &str,
) -> Result<(), String> {
    let Some(v) = opts_val else {
        return Ok(());
    };
    if matches!(v, Value::Unit) {
        return Ok(());
    }
    let Value::Dict(dict) = v else {
        return Err(crate::media::err_bad_opt(format!(
            "{} opts must be a dict, got '{}'",
            ctx,
            v.type_name()
        )));
    };
    for (k, fv) in dict.borrow().iter() {
        match k.as_str() {
            "scale" => {
                let s = as_f64_gfx(fv, ctx)?;
                opts.scale_x = s;
                opts.scale_y = s;
            }
            "scale_x" => opts.scale_x = as_f64_gfx(fv, ctx)?,
            "scale_y" => opts.scale_y = as_f64_gfx(fv, ctx)?,
            "rotation" | "angle" => opts.rotation_deg = as_f64_gfx(fv, ctx)?,
            "origin_x" => opts.origin_x = as_f64_gfx(fv, ctx)?,
            "origin_y" => opts.origin_y = as_f64_gfx(fv, ctx)?,
            "flip_x" => match fv {
                Value::Bool(b) => opts.flip_x = *b,
                _ => return Err(crate::media::err_bad_opt("flip_x must be bool")),
            },
            "flip_y" => match fv {
                Value::Bool(b) => opts.flip_y = *b,
                _ => return Err(crate::media::err_bad_opt("flip_y must be bool")),
            },
            "tint" => opts.tint = parse_tint_rgb(fv, ctx)?,
            "opacity" | "alpha" => match fv {
                Value::Int(i) if (0..=255).contains(i) => opts.opacity = *i as u8,
                Value::Float(f) if f.is_finite() && (0.0..=1.0).contains(f) => {
                    opts.opacity = (f * 255.0).round() as u8;
                }
                _ => {
                    return Err(crate::media::err_bad_opt(
                        "opacity/alpha must be int 0..=255 or float 0.0..=1.0",
                    ));
                }
            },
            "src_x" => opts.src_x = as_int(fv, ctx)?.clamp(0, i32::MAX as i64) as i32,
            "src_y" => opts.src_y = as_int(fv, ctx)?.clamp(0, i32::MAX as i64) as i32,
            "src_w" => opts.src_w = Some(as_int(fv, ctx)?.clamp(1, i32::MAX as i64) as i32),
            "src_h" => opts.src_h = Some(as_int(fv, ctx)?.clamp(1, i32::MAX as i64) as i32),
            "w" | "dst_w" => opts.dst_w = Some(as_f64_gfx(fv, ctx)?),
            "h" | "dst_h" => opts.dst_h = Some(as_f64_gfx(fv, ctx)?),
            "filter" => {
                let s = fv
                    .as_str()
                    .ok_or_else(|| crate::media::err_bad_opt("filter must be str"))?;
                opts.filter = crate::media::image::Filter::from_name(s)?;
            }
            "color_key" => opts.color_key = Some(parse_tint_rgb(fv, ctx)?),
            unknown => {
                return Err(crate::media::err_bad_opt(format!(
                    "{}: unknown option '{}'",
                    ctx, unknown
                )));
            }
        }
    }
    Ok(())
}

fn gfx_draw_image(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("gfx.draw_image() expects 3 arguments (img, x, y)".to_string());
    }
    let im = crate::modules::image::extract_image(&args[0], "gfx.draw_image()")?;
    let x = as_f64_gfx(&args[1], "draw_image")?;
    let y = as_f64_gfx(&args[2], "draw_image")?;
    blit_to_active_target(&im, x, y, crate::media::compose::BlitOpts::default())?;
    Ok(Value::Unit)
}

fn gfx_draw_image_keyed(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 6 {
        return Err("gfx.draw_image_keyed() expects 6 arguments (img, x, y, r, g, b)".to_string());
    }
    let im = crate::modules::image::extract_image(&args[0], "gfx.draw_image_keyed()")?;
    let x = as_f64_gfx(&args[1], "draw_image_keyed")?;
    let y = as_f64_gfx(&args[2], "draw_image_keyed")?;
    let key = rgb(
        as_int(&args[3], "draw_image_keyed")?,
        as_int(&args[4], "draw_image_keyed")?,
        as_int(&args[5], "draw_image_keyed")?,
    );
    let opts = crate::media::compose::BlitOpts {
        color_key: Some(key & 0x00FF_FFFF),
        ..crate::media::compose::BlitOpts::default()
    };
    blit_to_active_target(&im, x, y, opts)?;
    Ok(Value::Unit)
}

fn gfx_draw_sprite_frame(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 6 && args.len() != 7 {
        return Err(
            "gfx.draw_sprite_frame() expects 6 or 7 arguments (img, frame_idx, cols, rows, x, y, [scale])"
                .to_string(),
        );
    }
    let im = crate::modules::image::extract_image(&args[0], "gfx.draw_sprite_frame()")?;
    let frame_idx = as_int(&args[1], "draw_sprite_frame")?;
    let cols = as_int(&args[2], "draw_sprite_frame")?;
    let rows = as_int(&args[3], "draw_sprite_frame")?;
    let dx = as_f64_gfx(&args[4], "draw_sprite_frame")?;
    let dy = as_f64_gfx(&args[5], "draw_sprite_frame")?;
    let scale = if args.len() == 7 {
        as_f64_gfx(&args[6], "draw_sprite_frame")?
    } else {
        1.0
    };
    if !scale.is_finite() || scale <= 0.0 || scale > 64.0 {
        return Err("gfx.draw_sprite_frame() scale must be finite and in (0, 64]".to_string());
    }
    if !(1..=4096).contains(&cols) || !(1..=4096).contains(&rows) {
        return Err("gfx.draw_sprite_frame() cols and rows must be in 1..=4096".to_string());
    }
    let total_frames = cols
        .checked_mul(rows)
        .ok_or_else(|| "gfx.draw_sprite_frame(): grid dimensions overflow".to_string())?;
    if frame_idx < 0 || frame_idx >= total_frames {
        return Err(format!(
            "gfx.draw_sprite_frame(): frame index {} out of bounds 0..{}",
            frame_idx, total_frames
        ));
    }
    let frame_w = (im.width as i32) / (cols as i32);
    let frame_h = (im.height as i32) / (rows as i32);
    if frame_w <= 0 || frame_h <= 0 {
        return Err("gfx.draw_sprite_frame(): sprite grid exceeds image dimensions".to_string());
    }
    let col = (frame_idx % cols) as i32;
    let row = (frame_idx / cols) as i32;
    let opts = crate::media::compose::BlitOpts {
        src_x: col * frame_w,
        src_y: row * frame_h,
        src_w: Some(frame_w),
        src_h: Some(frame_h),
        scale_x: scale,
        scale_y: scale,
        ..crate::media::compose::BlitOpts::default()
    };
    blit_to_active_target(&im, dx, dy, opts)?;
    Ok(Value::Unit)
}

fn gfx_draw_sprite_ex(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 3 || args.len() > 4 {
        return Err("gfx.draw_sprite_ex() expects (img_or_tex, x, y, [opts])".to_string());
    }
    let mut opts = crate::media::compose::BlitOpts::default();
    let im = match &args[0] {
        Value::Image(im) => im.clone(),
        Value::Texture(t) => {
            opts.filter = t.filter;
            t.image.clone()
        }
        other => {
            return Err(crate::media::err_bad_opt(format!(
                "gfx.draw_sprite_ex() expects image or texture, got '{}'",
                other.type_name()
            )));
        }
    };
    let x = as_f64_gfx(&args[1], "draw_sprite_ex")?;
    let y = as_f64_gfx(&args[2], "draw_sprite_ex")?;
    apply_sprite_ex_opts(args.get(3), &mut opts, "gfx.draw_sprite_ex()")?;
    blit_to_active_target(&im, x, y, opts)?;
    Ok(Value::Unit)
}

fn gfx_atlas_rect(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("gfx.atlas_rect() expects (atlas, name_or_index)".to_string());
    }
    let (_, frame) = crate::media::atlas::resolve_atlas_frame(&args[0], &args[1])?;
    let mut map = crate::value::new_dict_with_capacity(6);
    map.insert("name".to_string(), Value::from_str(frame.name));
    map.insert("x".to_string(), Value::Int(frame.x as i64));
    map.insert("y".to_string(), Value::Int(frame.y as i64));
    map.insert("w".to_string(), Value::Int(frame.w as i64));
    map.insert("h".to_string(), Value::Int(frame.h as i64));
    map.insert("index".to_string(), Value::Int(frame.index as i64));
    Ok(Value::dict(map))
}

fn gfx_draw_atlas(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 4 || args.len() > 5 {
        return Err("gfx.draw_atlas() expects (atlas, name_or_index, x, y, [opts])".to_string());
    }
    let (im, frame) = crate::media::atlas::resolve_atlas_frame(&args[0], &args[1])?;
    let x = as_f64_gfx(&args[2], "draw_atlas")?;
    let y = as_f64_gfx(&args[3], "draw_atlas")?;
    let mut opts = crate::media::compose::BlitOpts {
        src_x: frame.x as i32,
        src_y: frame.y as i32,
        src_w: Some(frame.w as i32),
        src_h: Some(frame.h as i32),
        ..crate::media::compose::BlitOpts::default()
    };
    apply_sprite_ex_opts(args.get(4), &mut opts, "gfx.draw_atlas()")?;
    blit_to_active_target(&im, x, y, opts)?;
    Ok(Value::Unit)
}

fn gfx_make_anim(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("gfx.make_anim() expects (frames_or_atlas_or_sheet, [opts_or_fps])".to_string());
    }
    let mut frame_ms = 100u32;
    let mut durations_opt: Option<Vec<u32>> = None;
    let mut mode = crate::media::anim::AnimMode::Loop;
    let mut cols: Option<u32> = None;
    let mut rows: Option<u32> = None;
    let mut subset_frames: Option<Vec<Value>> = None;

    if let Some(opt_val) = args.get(1) {
        match opt_val {
            Value::Unit => {}
            Value::Int(fps) if *fps >= 1 && *fps <= 1000 => {
                frame_ms = (1000 / (*fps as u32)).max(1);
            }
            Value::Float(fps) if fps.is_finite() && *fps > 0.0 && *fps <= 1000.0 => {
                frame_ms = (1000.0 / *fps).round().max(1.0) as u32;
            }
            Value::Dict(d) => {
                for (k, fv) in d.borrow().iter() {
                    match k.as_str() {
                        "fps" => {
                            let fps = as_f64_gfx(fv, "gfx.make_anim()")?;
                            if !(0.001..=1000.0).contains(&fps) {
                                return Err(crate::media::err_bad_opt("anim fps out of range"));
                            }
                            frame_ms = (1000.0 / fps).round().max(1.0) as u32;
                        }
                        "frame_ms" => {
                            let ms = as_int(fv, "make_anim frame_ms")?;
                            if !(1..=600_000).contains(&ms) {
                                return Err(crate::media::err_bad_opt("frame_ms out of 1..=600000"));
                            }
                            frame_ms = ms as u32;
                        }
                        "durations" => {
                            let Value::List(l) = fv else {
                                return Err(crate::media::err_bad_opt("durations must be list of ints"));
                            };
                            let mut durs = Vec::with_capacity(l.borrow().len());
                            for item in l.borrow().iter() {
                                let ms = as_int(item, "make_anim duration")?;
                                if !(1..=600_000).contains(&ms) {
                                    return Err(crate::media::err_bad_opt("duration must be > 0 ms"));
                                }
                                durs.push(ms as u32);
                            }
                            durations_opt = Some(durs);
                        }
                        "mode" => {
                            let s = fv.as_str().ok_or_else(|| {
                                crate::media::err_bad_opt("anim mode must be str")
                            })?;
                            mode = crate::media::anim::AnimMode::from_name(s)?;
                        }
                        "loop" => match fv {
                            Value::Bool(true) => mode = crate::media::anim::AnimMode::Loop,
                            Value::Bool(false) => mode = crate::media::anim::AnimMode::Once,
                            _ => return Err(crate::media::err_bad_opt("anim loop must be bool")),
                        },
                        "cols" => {
                            let c = as_int(fv, "make_anim cols")?;
                            if !(1..=4096).contains(&c) {
                                return Err(crate::media::err_bad_opt("anim cols must be 1..=4096"));
                            }
                            cols = Some(c as u32);
                        }
                        "rows" => {
                            let r = as_int(fv, "make_anim rows")?;
                            if !(1..=4096).contains(&r) {
                                return Err(crate::media::err_bad_opt("anim rows must be 1..=4096"));
                            }
                            rows = Some(r as u32);
                        }
                        "frames" => {
                            let Value::List(l) = fv else {
                                return Err(crate::media::err_bad_opt("anim frames must be list"));
                            };
                            subset_frames = Some(l.borrow().clone());
                        }
                        unknown => {
                            return Err(crate::media::err_bad_opt(format!(
                                "gfx.make_anim(): unknown option '{}'",
                                unknown
                            )));
                        }
                    }
                }
            }
            other => {
                return Err(crate::media::err_bad_opt(format!(
                    "gfx.make_anim() 2nd arg must be fps or opts dict, got '{}'",
                    other.type_name()
                )));
            }
        }
    }

    let source = match &args[0] {
        Value::List(l) => {
            let mut frames = Vec::with_capacity(l.borrow().len());
            for item in l.borrow().iter() {
                frames.push(crate::modules::image::extract_image(item, "gfx.make_anim()")?);
            }
            crate::media::anim::AnimSource::Frames(frames)
        }
        Value::Image(_) | Value::Texture(_) => {
            let im = crate::modules::image::extract_image(&args[0], "gfx.make_anim()")?;
            if cols.is_none() && rows.is_none() && im.frame_count() > 1 {
                let count = im.frame_count();
                let mut frames = Vec::with_capacity(count);
                for i in 0..count {
                    let bytes = im.frame_data(i).unwrap_or(&[]).to_vec();
                    let fi = crate::media::image::MediaImage::from_vec(
                        im.width,
                        im.height,
                        im.format,
                        im.color_space,
                        crate::media::image::SourceFormat::Derived,
                        bytes,
                        Some(im.has_alpha),
                        None,
                        None,
                    )?;
                    frames.push(fi);
                }
                if durations_opt.is_none() && im.delays_ms.len() == count {
                    durations_opt = Some(im.delays_ms.iter().map(|&d| (d as u32).max(1)).collect());
                }
                crate::media::anim::AnimSource::Frames(frames)
            } else {
                let c = cols.ok_or_else(|| {
                    crate::media::err_bad_opt("gfx.make_anim() spritesheet requires 'cols' option")
                })?;
                let r = rows.ok_or_else(|| {
                    crate::media::err_bad_opt("gfx.make_anim() spritesheet requires 'rows' option")
                })?;
                if im.width / c == 0 || im.height / r == 0 {
                    return Err(crate::media::err_bad_opt(
                        "gfx.make_anim() cols/rows exceed image dimensions",
                    ));
                }
                let total = c * r;
                let indices = if let Some(ref sub) = subset_frames {
                    let mut idxs = Vec::with_capacity(sub.len());
                    for item in sub {
                        let i = as_int(item, "make_anim frame index")?;
                        if i < 0 || (i as u32) >= total {
                            return Err(crate::media::err_bad_opt("anim frame index out of bounds"));
                        }
                        idxs.push(i as u32);
                    }
                    idxs
                } else {
                    (0..total).collect()
                };
                crate::media::anim::AnimSource::Sheet {
                    image: im,
                    cols: c,
                    rows: r,
                    indices,
                }
            }
        }
        Value::Dict(_) => {
            let keys: Vec<Value> = if let Some(ref sub) = subset_frames {
                sub.clone()
            } else if let Value::Dict(root) = &args[0] {
                if let Some(Value::List(ord)) = root.borrow().get("order") {
                    ord.borrow().clone()
                } else {
                    return Err(crate::media::err_bad_opt(
                        "gfx.make_anim() atlas missing 'order' or 'frames' option",
                    ));
                }
            } else {
                unreachable!();
            };
            if keys.is_empty() {
                return Err(crate::media::err_bad_opt("anim frames list cannot be empty"));
            }
            let (first_img, _) = crate::media::atlas::resolve_atlas_frame(&args[0], &keys[0])?;
            let mut rects = Vec::with_capacity(keys.len());
            for k in &keys {
                let (_, fr) = crate::media::atlas::resolve_atlas_frame(&args[0], k)?;
                rects.push([fr.x, fr.y, fr.w, fr.h]);
            }
            crate::media::anim::AnimSource::Atlas {
                image: first_img,
                rects,
            }
        }
        other => {
            return Err(crate::media::err_bad_opt(format!(
                "gfx.make_anim() unsupported source '{}'",
                other.type_name()
            )));
        }
    };

    let count = source.len();
    let durations = durations_opt.unwrap_or_else(|| vec![frame_ms; count]);
    let anim = crate::media::anim::SpriteAnim::new(source, durations, mode)?;
    let handle = crate::media::anim::insert_anim(anim)?;
    Ok(Value::Int(handle))
}

fn gfx_anim_update(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("gfx.anim_update() expects (anim, dt_ms)".to_string());
    }
    let handle = as_handle_raw(&args[0], "anim_update")?;
    let dt_ms = match &args[1] {
        Value::Int(ms) if *ms >= 0 => *ms as u64,
        Value::Float(ms) if ms.is_finite() && *ms >= 0.0 => ms.round() as u64,
        _ => return Err(crate::media::err_bad_opt("gfx.anim_update() dt_ms must be >= 0")),
    };
    let idx = crate::media::anim::update_anim(handle, dt_ms)?;
    Ok(Value::Int(idx))
}

fn gfx_anim_frame(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.anim_frame() expects (anim)".to_string());
    }
    let handle = as_handle_raw(&args[0], "anim_frame")?;
    Ok(Value::Int(crate::media::anim::get_anim_frame_idx(handle)?))
}

fn gfx_anim_draw(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 3 || args.len() > 4 {
        return Err("gfx.anim_draw() expects (anim, x, y, [opts])".to_string());
    }
    let handle = as_handle_raw(&args[0], "anim_draw")?;
    let x = as_f64_gfx(&args[1], "anim_draw")?;
    let y = as_f64_gfx(&args[2], "anim_draw")?;
    let (im, [sx, sy, sw, sh]) = crate::media::anim::get_anim_current_draw(handle)?;
    let mut opts = crate::media::compose::BlitOpts {
        src_x: sx,
        src_y: sy,
        src_w: Some(sw),
        src_h: Some(sh),
        ..crate::media::compose::BlitOpts::default()
    };
    apply_sprite_ex_opts(args.get(3), &mut opts, "gfx.anim_draw()")?;
    blit_to_active_target(&im, x, y, opts)?;
    Ok(Value::Unit)
}

fn gfx_image_width(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.image_width() expects 1 argument (img)".to_string());
    }
    let im = crate::modules::image::extract_image(&args[0], "gfx.image_width()")?;
    Ok(Value::Int(im.width as i64))
}

fn gfx_image_height(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.image_height() expects 1 argument (img)".to_string());
    }
    let im = crate::modules::image::extract_image(&args[0], "gfx.image_height()")?;
    Ok(Value::Int(im.height as i64))
}

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
    match gfx_state::is_frame_headless() {
        Some(true) => {
            let _ = title;
            Ok(Value::Unit)
        }
        Some(false) => {
            #[cfg(windows)]
            {
                win32_gfx::set_window_title(title)?;
                Ok(Value::Unit)
            }
            #[cfg(not(windows))]
            {
                let _ = title;
                Err("No active gfx window".to_string())
            }
        }
        None => Err("No active gfx window".to_string()),
    }
}

fn gfx_delta(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.delta() expects 0 arguments".to_string());
    }
    let now = std::time::Instant::now();
    let dt = gfx_state::LAST_FRAME.with(|lf| {
        let mut slot = lf.borrow_mut();
        let dt = slot
            .map(|prev| now.duration_since(prev).as_secs_f64().clamp(0.0, 0.1))
            .unwrap_or(0.0);
        *slot = Some(now);
        dt
    });
    Ok(Value::Float(dt))
}



fn as_f64_gfx(v: &Value, fn_name: &str) -> Result<f64, String> {
    match v {
        Value::Int(i) => Ok(*i as f64),
        Value::Float(f) if f.is_finite() => Ok(*f),
        Value::Float(_) => Err(format!("gfx.{}(): float argument must be finite", fn_name)),
        other => Err(format!(
            "gfx.{}() expects a number, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

fn as_handle_raw(v: &Value, fn_name: &str) -> Result<i64, String> {
    match v {
        Value::Int(i) => Ok(*i),
        other => Err(format!(
            "gfx.{}() expects integer handle, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

fn surface_slot(handle: i64) -> Result<usize, String> {
    if !(SURFACE_ID_BASE..LAYER_ID_BASE).contains(&handle) {
        return Err(format!("Invalid surface handle {}", handle));
    }
    Ok((handle - SURFACE_ID_BASE) as usize)
}

fn layer_slot(handle: i64) -> Result<usize, String> {
    if handle < LAYER_ID_BASE {
        return Err(format!("Invalid layer handle {}", handle));
    }
    Ok((handle - LAYER_ID_BASE) as usize)
}

fn parse_color_argb(val: &Value) -> Result<u32, String> {
    match val {
        Value::Int(n) => {
            let u = *n as u64;
            if u <= 0x00FF_FFFF {
                Ok(0xFF00_0000 | (u as u32))
            } else {
                Ok(u as u32)
            }
        }
        Value::Str(s) => {
            let t = s.trim();
            if let Some(hex) = t.strip_prefix('#') {
                if hex.len() == 8 {
                    return u32::from_str_radix(hex, 16)
                        .map_err(|_| format!("Invalid #AARRGGBB color '{}'", s));
                }
            }
            let rgb = crate::modules::ui::parse_color_str(
                t,
                &crate::modules::ui::Theme::default(),
            )?;
            Ok(0xFF00_0000 | (rgb & 0x00FF_FFFF))
        }
        Value::List(items) => {
            let b = items.borrow();
            if b.len() != 3 && b.len() != 4 {
                return Err("Color list must have 3 [r,g,b] or 4 [r,g,b,a] elements".to_string());
            }
            let r = as_int(&b[0], "color[0]")?.clamp(0, 255) as u32;
            let g = as_int(&b[1], "color[1]")?.clamp(0, 255) as u32;
            let bl = as_int(&b[2], "color[2]")?.clamp(0, 255) as u32;
            let a = if b.len() == 4 {
                as_int(&b[3], "color[3]")?.clamp(0, 255) as u32
            } else {
                255
            };
            Ok((a << 24) | (r << 16) | (g << 8) | bl)
        }
        other => Err(format!(
            "gfx.draw_text() expects color int, '#RRGGBB'/'#AARRGGBB', or [r,g,b(,a)], got '{}'",
            other.type_name()
        )),
    }
}

fn gfx_clip(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 4 {
        return Err("gfx.clip() expects 4 arguments (x, y, w, h)".to_string());
    }
    let x = as_int(&args[0], "clip")? as i32;
    let y = as_int(&args[1], "clip")? as i32;
    let w = (as_int(&args[2], "clip")? as i32).max(0);
    let h = (as_int(&args[3], "clip")? as i32).max(0);
    set_active_clip(Some(ClipRect {
        x0: x,
        y0: y,
        x1: x.saturating_add(w),
        y1: y.saturating_add(h),
    }))?;
    Ok(Value::Unit)
}

fn gfx_reset_clip(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.reset_clip() expects 0 arguments".to_string());
    }
    set_active_clip(None)?;
    Ok(Value::Unit)
}

fn gfx_surface_new(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("gfx.surface_new() expects 2 arguments (width, height)".to_string());
    }
    let w = as_int(&args[0], "surface_new")? as i32;
    let h = as_int(&args[1], "surface_new")? as i32;
    if !(1..=crate::surface::MAX_USER_SURFACE_DIM).contains(&w)
        || !(1..=crate::surface::MAX_USER_SURFACE_DIM).contains(&h)
    {
        return Err(format!(
            "gfx.surface_new() dimensions must be in 1..={}",
            crate::surface::MAX_USER_SURFACE_DIM
        ));
    }
    gfx_state::bind_budget(vm.budget_core());
    gfx_state::SURFACES.with(|surfs| {
        let mut list = surfs.borrow_mut();
        let active = list.iter().filter(|s| s.is_some()).count();
        if active >= crate::surface::MAX_SURFACES {
            return Err(format!(
                "gfx.surface_new(): maximum surfaces ({}) reached",
                crate::surface::MAX_SURFACES
            ));
        }
        let surf = crate::surface::Surface::new_rgba8(w, h, 0x0000_0000, Some(vm.budget_core()))?;
        let slot = if let Some(idx) = list.iter().position(|s| s.is_none()) {
            list[idx] = Some(surf);
            idx
        } else {
            list.push(Some(surf));
            list.len() - 1
        };
        Ok(Value::Int(SURFACE_ID_BASE + slot as i64))
    })
}

fn gfx_surface_free(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.surface_free() expects 1 argument (surface)".to_string());
    }
    let handle = as_handle_raw(&args[0], "surface_free")?;
    let slot = surface_slot(handle)?;
    if is_target_active_on_stack(gfx_state::ActiveTarget::Surface(slot)) {
        return Err("gfx.surface_free(): cannot free surface while active in draw_to".to_string());
    }
    gfx_state::bind_budget(vm.budget_core());
    gfx_state::SURFACES.with(|surfs| {
        let mut list = surfs.borrow_mut();
        let Some(entry) = list.get_mut(slot) else {
            return Err(format!("Invalid or freed surface handle {}", handle));
        };
        let Some(mut surf) = entry.take() else {
            return Err(format!("Invalid or freed surface handle {}", handle));
        };
        surf.refund_to(Some(vm.budget_core()));
        Ok(Value::Unit)
    })
}

fn gfx_surface_width(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.surface_width() expects 1 argument (surface)".to_string());
    }
    let handle = as_handle_raw(&args[0], "surface_width")?;
    let slot = surface_slot(handle)?;
    gfx_state::SURFACES.with(|surfs| {
        let list = surfs.borrow();
        let surf = list
            .get(slot)
            .and_then(|o| o.as_ref())
            .ok_or_else(|| format!("Invalid or freed surface handle {}", handle))?;
        Ok(Value::Int(surf.width as i64))
    })
}

fn gfx_surface_height(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.surface_height() expects 1 argument (surface)".to_string());
    }
    let handle = as_handle_raw(&args[0], "surface_height")?;
    let slot = surface_slot(handle)?;
    gfx_state::SURFACES.with(|surfs| {
        let list = surfs.borrow();
        let surf = list
            .get(slot)
            .and_then(|o| o.as_ref())
            .ok_or_else(|| format!("Invalid or freed surface handle {}", handle))?;
        Ok(Value::Int(surf.height as i64))
    })
}

fn gfx_surface_from_image(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.surface_from_image() expects 1 argument (img)".to_string());
    }
    let im = crate::modules::image::extract_image(&args[0], "gfx.surface_from_image()")?;
    let w = im.width as i32;
    let h = im.height as i32;
    let pixels = im.to_argb_u32_vec();
    gfx_state::bind_budget(vm.budget_core());
    gfx_state::SURFACES.with(|surfs| {
        let mut list = surfs.borrow_mut();
        let active = list.iter().filter(|s| s.is_some()).count();
        if active >= crate::surface::MAX_SURFACES {
            return Err(format!(
                "gfx.surface_from_image(): maximum surfaces ({}) reached",
                crate::surface::MAX_SURFACES
            ));
        }
        let surf = crate::surface::Surface::from_rgba_pixels(w, h, pixels, Some(vm.budget_core()))?;
        let slot = if let Some(idx) = list.iter().position(|s| s.is_none()) {
            list[idx] = Some(surf);
            idx
        } else {
            list.push(Some(surf));
            list.len() - 1
        };
        Ok(Value::Int(SURFACE_ID_BASE + slot as i64))
    })
}

struct TargetPopGuard;
impl Drop for TargetPopGuard {
    fn drop(&mut self) {
        gfx_state::TARGET_STACK.with(|st| {
            st.borrow_mut().pop();
        });
    }
}

fn run_on_pushed_target(
    vm: &mut VM,
    target: gfx_state::ActiveTarget,
    callback: Value,
    ctx: &str,
) -> Result<Value, String> {
    if is_target_active_on_stack(target) {
        return Err(format!(
            "gfx.{}(): self-targeting active surface or layer is forbidden",
            ctx
        ));
    }
    let parent_cam = gfx_state::current_camera();
    gfx_state::TARGET_STACK.with(|st| -> Result<(), String> {
        let mut stack = st.borrow_mut();
        if stack.len() >= crate::surface::MAX_TARGET_DEPTH {
            return Err(format!(
                "gfx.{}(): maximum target stack depth ({}) exceeded",
                ctx,
                crate::surface::MAX_TARGET_DEPTH
            ));
        }
        stack.push(gfx_state::TargetFrame {
            target,
            camera: parent_cam,
        });
        Ok(())
    })?;
    let _guard = TargetPopGuard;
    vm.call_value(callback, &[])
}

fn gfx_draw_to(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("gfx.draw_to() expects 2 arguments (surface, callback)".to_string());
    }
    let handle = as_handle_raw(&args[0], "draw_to")?;
    let slot = surface_slot(handle)?;
    gfx_state::SURFACES.with(|surfs| {
        let list = surfs.borrow();
        if list.get(slot).and_then(|o| o.as_ref()).is_none() {
            return Err(format!("Invalid or freed surface handle {}", handle));
        }
        Ok(())
    })?;
    run_on_pushed_target(vm, gfx_state::ActiveTarget::Surface(slot), args[1].clone(), "draw_to")
}

#[allow(clippy::too_many_arguments)]
fn draw_surface_blit_to_buf(
    dst: &mut [u32],
    dst_w: i32,
    dst_h: i32,
    dst_clip: ClipRect,
    preserve_dst_alpha: bool,
    cam: &crate::camera::Camera2D,
    src: &[u32],
    src_w: i32,
    src_h: i32,
    dx: i32,
    dy: i32,
    sx: f64,
    sy: f64,
    key: Option<u32>,
    alpha: u8,
) {
    if cam.is_identity_rect() {
        crate::surface::blit_rgba_surface(
            dst,
            dst_w,
            dst_clip,
            preserve_dst_alpha,
            src,
            src_w,
            src_h,
            dx,
            dy,
            sx,
            sy,
            key,
            alpha,
        );
        return;
    }
    if cam.angle == 0.0 {
        let (tx, ty) = cam.world_to_screen(dx as f64, dy as f64, dst_w, dst_h);
        crate::surface::blit_rgba_surface(
            dst,
            dst_w,
            dst_clip,
            preserve_dst_alpha,
            src,
            src_w,
            src_h,
            tx.round() as i32,
            ty.round() as i32,
            sx * cam.zoom,
            sy * cam.zoom,
            key,
            alpha,
        );
        return;
    }
    let dw = (src_w as f64) * sx;
    let dh = (src_h as f64) * sy;
    let corners = [
        cam.world_to_screen(dx as f64, dy as f64, dst_w, dst_h),
        cam.world_to_screen(dx as f64 + dw, dy as f64, dst_w, dst_h),
        cam.world_to_screen(dx as f64, dy as f64 + dh, dst_w, dst_h),
        cam.world_to_screen(dx as f64 + dw, dy as f64 + dh, dst_w, dst_h),
    ];
    let min_x = (corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min).floor() as i32).max(dst_clip.x0);
    let min_y = (corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min).floor() as i32).max(dst_clip.y0);
    let max_x = (corners.iter().map(|c| c.0).fold(f64::NEG_INFINITY, f64::max).ceil() as i32).min(dst_clip.x1);
    let max_y = (corners.iter().map(|c| c.1).fold(f64::NEG_INFINITY, f64::max).ceil() as i32).min(dst_clip.y1);
    let dst_wu = dst_w as usize;
    let src_wu = src_w as usize;
    for py in min_y..max_y {
        let drow = (py as usize) * dst_wu;
        for px in min_x..max_x {
            let (wx, wy) = cam.screen_to_world(px as f64 + 0.5, py as f64 + 0.5, dst_w, dst_h);
            let lx = (wx - dx as f64) / sx;
            let ly = (wy - dy as f64) / sy;
            if lx >= 0.0 && lx < src_w as f64 && ly >= 0.0 && ly < src_h as f64 {
                let sc = src[(ly as usize) * src_wu + (lx as usize)];
                if let Some(k) = key {
                    if (sc & 0x00FF_FFFF) == (k & 0x00FF_FFFF) {
                        continue;
                    }
                }
                let sa = (sc >> 24) & 0xFF;
                let eff_a = if alpha == 255 {
                    sa
                } else {
                    (sa * (alpha as u32) + 127) / 255
                };
                let d_idx = drow + px as usize;
                dst[d_idx] = crate::surface::blend_straight_argb(
                    dst[d_idx],
                    (sc >> 16) & 0xFF,
                    (sc >> 8) & 0xFF,
                    sc & 0xFF,
                    eff_a,
                    preserve_dst_alpha,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_surface_impl(
    vm: &mut VM,
    handle: i64,
    dx: i32,
    dy: i32,
    sx: f64,
    sy: f64,
    key: Option<u32>,
    alpha: u8,
) -> Result<Value, String> {
    let slot = surface_slot(handle)?;
    if is_target_active_on_stack(gfx_state::ActiveTarget::Surface(slot)) {
        return Err("gfx.draw_surface(): self-targeting active surface is forbidden".to_string());
    }
    let top = gfx_state::TARGET_STACK.with(|st| st.borrow().last().copied());
    let work_pixels = match top {
        Some(tf) => match tf.target {
            gfx_state::ActiveTarget::Surface(dst_slot) => {
                debug_assert_ne!(slot, dst_slot);
                gfx_state::SURFACES.with(|surfs| -> Result<u64, String> {
                    let mut list = surfs.borrow_mut();
                    let len = list.len();
                    if slot >= len || dst_slot >= len {
                        return Err(format!("Invalid surface handle {}", handle));
                    }
                    let (src_opt, dst_opt) = if slot < dst_slot {
                        let (l, r) = list.split_at_mut(dst_slot);
                        (&l[slot], &mut r[0])
                    } else {
                        let (l, r) = list.split_at_mut(slot);
                        (&r[0], &mut l[dst_slot])
                    };
                    let src = src_opt
                        .as_ref()
                        .ok_or_else(|| format!("Invalid or freed surface handle {}", handle))?;
                    let dst = dst_opt
                        .as_mut()
                        .ok_or_else(|| "Active target surface was freed".to_string())?;
                    let clip = dst.effective_clip();
                    let px_work = (((src.width as f64) * sx).abs() * ((src.height as f64) * sy).abs())
                        .round()
                        .max(1.0) as u64;
                    draw_surface_blit_to_buf(
                        &mut dst.pixels,
                        dst.width,
                        dst.height,
                        clip,
                        false,
                        &tf.camera,
                        &src.pixels,
                        src.width,
                        src.height,
                        dx,
                        dy,
                        sx,
                        sy,
                        key,
                        alpha,
                    );
                    Ok(px_work)
                })?
            }
            gfx_state::ActiveTarget::Layer(dst_slot) => gfx_state::SURFACES.with(|surfs| {
                let list = surfs.borrow();
                let src = list
                    .get(slot)
                    .and_then(|o| o.as_ref())
                    .ok_or_else(|| format!("Invalid or freed surface handle {}", handle))?;
                let px_work = (((src.width as f64) * sx).abs() * ((src.height as f64) * sy).abs())
                    .round()
                    .max(1.0) as u64;
                gfx_state::LAYERS.with(|layers| -> Result<u64, String> {
                    let mut bank = layers.borrow_mut();
                    let dst = bank.get_mut(dst_slot)?;
                    let clip = dst.surface.effective_clip();
                    draw_surface_blit_to_buf(
                        &mut dst.surface.pixels,
                        dst.surface.width,
                        dst.surface.height,
                        clip,
                        false,
                        &tf.camera,
                        &src.pixels,
                        src.width,
                        src.height,
                        dx,
                        dy,
                        sx,
                        sy,
                        key,
                        alpha,
                    );
                    Ok(px_work)
                })
            })?,
        },
        None => gfx_state::SURFACES.with(|surfs| {
            let list = surfs.borrow();
            let src = list
                .get(slot)
                .and_then(|o| o.as_ref())
                .ok_or_else(|| format!("Invalid or freed surface handle {}", handle))?;
            let px_work = (((src.width as f64) * sx).abs() * ((src.height as f64) * sy).abs())
                .round()
                .max(1.0) as u64;
            let cam = gfx_state::ROOT_CAMERA.with(|c| *c.borrow());
            gfx_state::FRAME.with(|w| -> Result<u64, String> {
                let mut borrow = w.borrow_mut();
                let state = borrow
                    .as_mut()
                    .ok_or_else(|| "No active gfx window".to_string())?;
                let clip = crate::surface::effective_clip(state.width, state.height, state.clip);
                draw_surface_blit_to_buf(
                    &mut state.buffer,
                    state.width,
                    state.height,
                    clip,
                    true,
                    &cam,
                    &src.pixels,
                    src.width,
                    src.height,
                    dx,
                    dy,
                    sx,
                    sy,
                    key,
                    alpha,
                );
                Ok(px_work)
            })
        })?,
    };
    vm.charge_work((work_pixels / 64).max(1))?;
    Ok(Value::Unit)
}

fn gfx_draw_surface(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("gfx.draw_surface() expects 3 arguments (surface, x, y)".to_string());
    }
    let handle = as_handle_raw(&args[0], "draw_surface")?;
    let x = as_int(&args[1], "draw_surface")? as i32;
    let y = as_int(&args[2], "draw_surface")? as i32;
    draw_surface_impl(vm, handle, x, y, 1.0, 1.0, None, 255)
}

fn gfx_draw_surface_scaled(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 5 {
        return Err("gfx.draw_surface_scaled() expects 5 arguments (surface, x, y, sx, sy)".to_string());
    }
    let handle = as_handle_raw(&args[0], "draw_surface_scaled")?;
    let x = as_int(&args[1], "draw_surface_scaled")? as i32;
    let y = as_int(&args[2], "draw_surface_scaled")? as i32;
    let sx = as_f64_gfx(&args[3], "draw_surface_scaled")?;
    let sy = as_f64_gfx(&args[4], "draw_surface_scaled")?;
    if sx <= 0.0 || sx > 64.0 || sy <= 0.0 || sy > 64.0 {
        return Err("gfx.draw_surface_scaled() scales must be in (0, 64]".to_string());
    }
    draw_surface_impl(vm, handle, x, y, sx, sy, None, 255)
}

fn gfx_draw_surface_keyed(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 6 {
        return Err(
            "gfx.draw_surface_keyed() expects 6 arguments (surface, x, y, r, g, b)".to_string(),
        );
    }
    let handle = as_handle_raw(&args[0], "draw_surface_keyed")?;
    let x = as_int(&args[1], "draw_surface_keyed")? as i32;
    let y = as_int(&args[2], "draw_surface_keyed")? as i32;
    let key = rgb(
        as_int(&args[3], "draw_surface_keyed")?,
        as_int(&args[4], "draw_surface_keyed")?,
        as_int(&args[5], "draw_surface_keyed")?,
    );
    draw_surface_impl(vm, handle, x, y, 1.0, 1.0, Some(key), 255)
}

fn gfx_draw_surface_alpha(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 4 {
        return Err("gfx.draw_surface_alpha() expects 4 arguments (surface, x, y, alpha)".to_string());
    }
    let handle = as_handle_raw(&args[0], "draw_surface_alpha")?;
    let x = as_int(&args[1], "draw_surface_alpha")? as i32;
    let y = as_int(&args[2], "draw_surface_alpha")? as i32;
    let alpha = as_int(&args[3], "draw_surface_alpha")?.clamp(0, 255) as u8;
    draw_surface_impl(vm, handle, x, y, 1.0, 1.0, None, alpha)
}

fn gfx_read_pixel(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("gfx.read_pixel() expects 2 arguments (x, y)".to_string());
    }
    let x = as_int(&args[0], "read_pixel")? as i32;
    let y = as_int(&args[1], "read_pixel")? as i32;
    with_active_target(|buf, w, h, _clip, _off, _cam| {
        if x < 0 || y < 0 || x >= w || y >= h {
            return Err(format!("gfx.read_pixel({}, {}) out of bounds {}x{}", x, y, w, h));
        }
        Ok(Value::Int(buf[(y as usize) * (w as usize) + (x as usize)] as i64))
    })
}

fn gfx_frame_hash(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.frame_hash() expects 0 arguments".to_string());
    }
    with_active_target(|buf, w, h, _clip, _off, _cam| {
        Ok(Value::from_str(crate::surface::hash_rgba_buffer(w, h, buf)))
    })
}

fn gfx_layer_new(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("gfx.layer_new() expects 2 arguments (width, height)".to_string());
    }
    let w = as_int(&args[0], "layer_new")? as i32;
    let h = as_int(&args[1], "layer_new")? as i32;
    if !(1..=crate::surface::MAX_USER_SURFACE_DIM).contains(&w)
        || !(1..=crate::surface::MAX_USER_SURFACE_DIM).contains(&h)
    {
        return Err(format!(
            "gfx.layer_new() dimensions must be in 1..={}",
            crate::surface::MAX_USER_SURFACE_DIM
        ));
    }
    gfx_state::bind_budget(vm.budget_core());
    gfx_state::LAYERS.with(|layers| {
        let slot = layers.borrow_mut().create(w, h, Some(vm.budget_core()))?;
        Ok(Value::Int(LAYER_ID_BASE + slot as i64))
    })
}

fn gfx_layer_free(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.layer_free() expects 1 argument (layer)".to_string());
    }
    let handle = as_handle_raw(&args[0], "layer_free")?;
    let slot = layer_slot(handle)?;
    if is_target_active_on_stack(gfx_state::ActiveTarget::Layer(slot)) {
        return Err("gfx.layer_free(): cannot free layer while active in draw_to_layer".to_string());
    }
    gfx_state::bind_budget(vm.budget_core());
    gfx_state::LAYERS.with(|layers| {
        layers.borrow_mut().free(slot, Some(vm.budget_core()))?;
        Ok(Value::Unit)
    })
}

fn gfx_layer_mode(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("gfx.layer_mode() expects 2 arguments (layer, mode)".to_string());
    }
    let handle = as_handle_raw(&args[0], "layer_mode")?;
    let slot = layer_slot(handle)?;
    let Value::Str(m) = &args[1] else {
        return Err("gfx.layer_mode() mode must be string ('normal'|'add'|'multiply')".to_string());
    };
    let mode = crate::layers::BlendMode::from_str(m.as_str())?;
    gfx_state::LAYERS.with(|layers| {
        layers.borrow_mut().get_mut(slot)?.mode = mode;
        Ok(Value::Unit)
    })
}

fn gfx_layer_visible(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("gfx.layer_visible() expects 2 arguments (layer, visible)".to_string());
    }
    let handle = as_handle_raw(&args[0], "layer_visible")?;
    let slot = layer_slot(handle)?;
    let Value::Bool(v) = args[1] else {
        return Err("gfx.layer_visible() expects bool visibility".to_string());
    };
    gfx_state::LAYERS.with(|layers| {
        layers.borrow_mut().get_mut(slot)?.visible = v;
        Ok(Value::Unit)
    })
}

fn gfx_layer_z(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("gfx.layer_z() expects 2 arguments (layer, z_order)".to_string());
    }
    let handle = as_handle_raw(&args[0], "layer_z")?;
    let slot = layer_slot(handle)?;
    let z = as_int(&args[1], "layer_z")? as i32;
    gfx_state::LAYERS.with(|layers| {
        layers.borrow_mut().get_mut(slot)?.z_order = z;
        Ok(Value::Unit)
    })
}

fn gfx_layer_parallax(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("gfx.layer_parallax() expects 3 arguments (layer, px, py)".to_string());
    }
    let handle = as_handle_raw(&args[0], "layer_parallax")?;
    let slot = layer_slot(handle)?;
    let px = as_f64_gfx(&args[1], "layer_parallax")?;
    let py = as_f64_gfx(&args[2], "layer_parallax")?;
    if !(-4.0..=4.0).contains(&px) || !(-4.0..=4.0).contains(&py) {
        return Err("gfx.layer_parallax() factors must be in -4.0..=4.0".to_string());
    }
    gfx_state::LAYERS.with(|layers| {
        let mut bank = layers.borrow_mut();
        let lay = bank.get_mut(slot)?;
        lay.parallax_x = px;
        lay.parallax_y = py;
        Ok(Value::Unit)
    })
}

fn gfx_draw_to_layer(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("gfx.draw_to_layer() expects 2 arguments (layer, callback)".to_string());
    }
    let handle = as_handle_raw(&args[0], "draw_to_layer")?;
    let slot = layer_slot(handle)?;
    gfx_state::LAYERS.with(|layers| {
        let _ = layers.borrow().get(slot)?;
        Ok::<_, String>(())
    })?;
    run_on_pushed_target(vm, gfx_state::ActiveTarget::Layer(slot), args[1].clone(), "draw_to_layer")
}

fn gfx_compose_layers(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.compose_layers() expects 0 arguments".to_string());
    }
    let layer_active = gfx_state::TARGET_STACK.with(|st| {
        st.borrow()
            .iter()
            .any(|tf| matches!(tf.target, gfx_state::ActiveTarget::Layer(_)))
    });
    if layer_active {
        return Err("gfx.compose_layers() cannot be called inside draw_to_layer".to_string());
    }
    let visited = gfx_state::LAYERS.with(|layers| {
        let bank = layers.borrow();
        with_active_target_mut(|buf, w, h, clip, is_offscreen, cam| {
            Ok(crate::compose::compose_layers(
                buf,
                w,
                h,
                clip,
                is_offscreen,
                &bank,
                cam,
            ))
        })
    })?;
    vm.charge_work((visited / 64).max(1))?;
    Ok(Value::Unit)
}

fn gfx_set_camera(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 4 {
        return Err("gfx.set_camera() expects (x, y, [zoom], [angle])".to_string());
    }
    let x = as_f64_gfx(&args[0], "set_camera")?;
    let y = as_f64_gfx(&args[1], "set_camera")?;
    let zoom = if args.len() >= 3 {
        as_f64_gfx(&args[2], "set_camera")?
    } else {
        gfx_state::current_camera().zoom
    };
    let angle = if args.len() >= 4 {
        as_f64_gfx(&args[3], "set_camera")?
    } else {
        gfx_state::current_camera().angle
    };
    let mut cam = gfx_state::current_camera();
    cam.set(x, y, zoom, angle)?;
    gfx_state::set_current_camera(cam);
    Ok(Value::Unit)
}

fn gfx_camera(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.camera() expects 0 arguments".to_string());
    }
    vm.charge_alloc(192)?;
    let cam = gfx_state::current_camera();
    let mut map = crate::value::new_dict_with_capacity(6);
    map.insert("enabled".to_string(), Value::Bool(cam.enabled));
    map.insert("x".to_string(), Value::Float(cam.x));
    map.insert("y".to_string(), Value::Float(cam.y));
    map.insert("zoom".to_string(), Value::Float(cam.zoom));
    map.insert("angle".to_string(), Value::Float(cam.angle));
    map.insert("shake".to_string(), Value::Float(cam.current_shake_amp()));
    Ok(Value::dict(map))
}

fn gfx_reset_camera(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.reset_camera() expects 0 arguments".to_string());
    }
    let mut cam = gfx_state::current_camera();
    cam.reset();
    gfx_state::set_current_camera(cam);
    Ok(Value::Unit)
}

fn gfx_set_camera_shake(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("gfx.set_camera_shake() expects (intensity, duration)".to_string());
    }
    let intensity = as_f64_gfx(&args[0], "set_camera_shake")?;
    let duration = as_f64_gfx(&args[1], "set_camera_shake")?;
    let seed = if vm.is_deterministic() {
        Some(0xA5A4_E202_0502_0001)
    } else {
        None
    };
    let mut cam = gfx_state::current_camera();
    cam.set_shake(intensity, duration, seed)?;
    gfx_state::set_current_camera(cam);
    Ok(Value::Unit)
}

fn gfx_update_camera(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.update_camera() expects 1 argument (dt)".to_string());
    }
    let dt = as_f64_gfx(&args[0], "update_camera")?;
    if dt < 0.0 {
        return Err("gfx.update_camera() dt must be >= 0".to_string());
    }
    let mut cam = gfx_state::current_camera();
    cam.tick_shake(dt);
    gfx_state::set_current_camera(cam);
    Ok(Value::Unit)
}

fn gfx_world_to_screen(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("gfx.world_to_screen() expects 2 arguments (wx, wy)".to_string());
    }
    let wx = as_f64_gfx(&args[0], "world_to_screen")?;
    let wy = as_f64_gfx(&args[1], "world_to_screen")?;
    with_active_target(|_buf, w, h, _clip, _off, cam| {
        let (sx, sy) = cam.world_to_screen(wx, wy, w, h);
        Ok(Value::Vec2(sx as f32, sy as f32))
    })
}

fn gfx_screen_to_world(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("gfx.screen_to_world() expects 2 arguments (sx, sy)".to_string());
    }
    let sx = as_f64_gfx(&args[0], "screen_to_world")?;
    let sy = as_f64_gfx(&args[1], "screen_to_world")?;
    with_active_target(|_buf, w, h, _clip, _off, cam| {
        let (wx, wy) = cam.screen_to_world(sx, sy, w, h);
        Ok(Value::Vec2(wx as f32, wy as f32))
    })
}

fn gfx_mouse_world(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.mouse_world() expects 0 arguments".to_string());
    }
    let (mx, my) = match gfx_state::is_frame_headless() {
        Some(true) => (0.0, 0.0),
        Some(false) => {
            #[cfg(windows)]
            {
                let (x, y) = win32_gfx::mouse_pos()?;
                (x as f64, y as f64)
            }
            #[cfg(not(windows))]
            {
                (0.0, 0.0)
            }
        }
        None => return Err("No active gfx window".to_string()),
    };
    with_active_target(|_buf, w, h, _clip, _off, cam| {
        let (wx, wy) = cam.screen_to_world(mx, my, w, h);
        Ok(Value::Vec2(wx as f32, wy as f32))
    })
}

fn gfx_font_bitmap(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("gfx.font_bitmap() expects 0 arguments".to_string());
    }
    Ok(Value::Int(0))
}

fn gfx_font_load_bmfont(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    use std::io::Read;
    if args.len() != 1 {
        return Err("gfx.font_load_bmfont() expects 1 argument (fnt_path)".to_string());
    }
    let path = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "gfx.font_load_bmfont() expects a string path, got '{}'",
                other.type_name()
            ));
        }
    };
    gfx_state::bind_budget(vm.budget_core());
    let fnt_resolved = vm.resolve_read_path(path)?;
    let fnt_file = std::fs::File::open(&fnt_resolved)
        .map_err(|e| format!("gfx.font_load_bmfont('{}'): {}", path, e))?;
    let mut fnt_bytes = Vec::new();
    fnt_file
        .take(crate::font_bmfont::MAX_FNT_FILE_BYTES as u64 + 1)
        .read_to_end(&mut fnt_bytes)
        .map_err(|e| format!("gfx.font_load_bmfont('{}'): {}", path, e))?;
    if fnt_bytes.len() > crate::font_bmfont::MAX_FNT_FILE_BYTES {
        return Err("gfx.font_load_bmfont(): .fnt file exceeds 2 MiB limit".to_string());
    }
    let fnt_text = std::str::from_utf8(&fnt_bytes)
        .map_err(|_| "gfx.font_load_bmfont(): .fnt file must be valid UTF-8 ASCII".to_string())?;
    let base_dir = fnt_resolved
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| vm.sandbox_root().clone());

    let bmfont = crate::font_bmfont::parse_bmfont_ascii(fnt_text, Some(vm.budget_core()), |_, file| {
        let resolved_page = vm.resolve_read_path_from(&base_dir, file)?;
        let page_file = std::fs::File::open(&resolved_page)
            .map_err(|e| format!("gfx.font_load_bmfont page '{}': {}", file, e))?;
        const LIMIT: usize = 32 * 1024 * 1024;
        let mut bytes = Vec::new();
        page_file
            .take(LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("gfx.font_load_bmfont page '{}': {}", file, e))?;
        if bytes.len() > LIMIT {
            return Err("BMFont page image exceeds 32 MiB limit".to_string());
        }
        crate::font_bmfont::decode_page_bytes_r8(&bytes, &resolved_page)
    })?;

    gfx_state::FONTS.with(|fonts| {
        let id = fonts.borrow_mut().register_bmfont(bmfont)?;
        Ok(Value::Int(id as i64))
    })
}

fn gfx_font_free(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("gfx.font_free() expects 1 argument (font_id)".to_string());
    }
    let id = as_handle_raw(&args[0], "font_free")?;
    gfx_state::bind_budget(vm.budget_core());
    gfx_state::FONTS.with(|fonts| {
        fonts.borrow_mut().free_font(id, Some(vm.budget_core()))?;
        Ok(Value::Unit)
    })
}

fn gfx_draw_text(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 6 && args.len() != 7 {
        return Err(
            "gfx.draw_text() expects 6 or 7 arguments (font, x, y, text, size, color, [align])"
                .to_string(),
        );
    }
    let font_id = as_handle_raw(&args[0], "draw_text")?;
    let x = as_int(&args[1], "draw_text")? as i32;
    let y = as_int(&args[2], "draw_text")? as i32;
    let text = format!("{}", args[3]);
    let size = as_int(&args[4], "draw_text")? as i32;
    let color_argb = parse_color_argb(&args[5])?;
    let align = if let Some(a) = args.get(6) {
        let Value::Str(s) = a else {
            return Err("gfx.draw_text() align must be 'left', 'center', or 'right'".to_string());
        };
        match s.as_str() {
            "left" | "center" | "right" => s.as_str().to_string(),
            other => return Err(format!("gfx.draw_text() unknown align '{}'", other)),
        }
    } else {
        "left".to_string()
    };

    gfx_state::bind_budget(vm.budget_core());
    vm.charge_work(((text.chars().count() as u64) / 16).max(1))?;
    gfx_state::FONTS.with(|fonts| {
        let mut fs = fonts.borrow_mut();
        with_active_target_mut(|buf, w, h, clip, is_offscreen, cam| {
            fs.draw_text(
                buf,
                w,
                h,
                clip,
                cam,
                font_id,
                size,
                x,
                y,
                &text,
                color_argb,
                &align,
                !is_offscreen,
                Some(vm.budget_core()),
            )?;
            Ok(Value::Unit)
        })
    })
}

fn gfx_measure_text(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("gfx.measure_text() expects 3 arguments (font, text, size)".to_string());
    }
    let font_id = as_handle_raw(&args[0], "measure_text")?;
    let text = format!("{}", args[1]);
    let size = as_int(&args[2], "measure_text")? as i32;
    let (w, h, lh, lines) = gfx_state::FONTS.with(|fonts| {
        fonts.borrow().measure_text(font_id, size, &text)
    })?;
    vm.charge_alloc(128)?;
    let mut map = crate::value::new_dict_with_capacity(4);
    map.insert("width".to_string(), Value::Int(w as i64));
    map.insert("height".to_string(), Value::Int(h as i64));
    map.insert("line_height".to_string(), Value::Int(lh as i64));
    map.insert("lines".to_string(), Value::Int(lines as i64));
    Ok(Value::dict(map))
}

fn gfx_text_line_height(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("gfx.text_line_height() expects 2 arguments (font, size)".to_string());
    }
    let font_id = as_handle_raw(&args[0], "text_line_height")?;
    let size = as_int(&args[1], "text_line_height")? as i32;
    let lh = gfx_state::FONTS.with(|fonts| fonts.borrow().line_height(font_id, size))?;
    Ok(Value::Int(lh as i64))
}

fn gfx_text_width(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("gfx.text_width() expects 3 arguments (font, text, size)".to_string());
    }
    let font_id = as_handle_raw(&args[0], "text_width")?;
    let text = format!("{}", args[1]);
    let size = as_int(&args[2], "text_width")? as i32;
    let (w, _, _, _) = gfx_state::FONTS.with(|fonts| fonts.borrow().measure_text(font_id, size, &text))?;
    Ok(Value::Int(w as i64))
}

#[cfg(test)]
pub(crate) fn test_flush_font_atlas() -> u64 {
    gfx_state::with_budget_opt(|b| {
        gfx_state::FONTS.with(|fonts| {
            let mut fs = fonts.borrow_mut();
            fs.flush_atlas(b);
            fs.flush_count
        })
    })
}
