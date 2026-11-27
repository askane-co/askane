use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

#[derive(Clone, Debug)]
pub struct Theme {
    pub bg: u32,
    pub surface: u32,
    pub surface_hover: u32,
    pub surface_active: u32,
    pub border: u32,
    pub border_focus: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub accent_hover: u32,
    pub accent_text: u32,
    pub padding: i32,
    pub spacing: i32,
    pub font_size: i32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg: 0x17_1A_21,
            surface: 0x22_27_33,
            surface_hover: 0x2D_34_44,
            surface_active: 0x36_3F_52,
            border: 0x3D_46_59,
            border_focus: 0x4F_8F_F7,
            text: 0xEC_F0_F6,
            muted: 0x94_9E_B3,
            accent: 0x4F_8F_F7,
            accent_hover: 0x67_A0_FA,
            accent_text: 0xFF_FF_FF,
            padding: 16,
            spacing: 10,
            font_size: 16,
        }
    }
}

impl Theme {
    pub fn light() -> Self {
        Self {
            bg: 0xF5_F7_FA,
            surface: 0xFF_FF_FF,
            surface_hover: 0xEA_EE_F5,
            surface_active: 0xDC_E3_EE,
            border: 0xC9_D2_E0,
            border_focus: 0x25_63_EB,
            text: 0x1A_20_2C,
            muted: 0x64_74_8B,
            accent: 0x25_63_EB,
            accent_hover: 0x1D_4E_D8,
            accent_text: 0xFF_FF_FF,
            padding: 16,
            spacing: 10,
            font_size: 16,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LayoutRect {
    pub id: Option<String>,
    pub kind: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Clone, Debug)]
enum WidgetKind {
    Column,
    Row,
    Label,
    Button,
    TextInput,
    Checkbox,
    Slider,
    Select,
    Spacer,
    Rect,
    Image,
    NineSlice,
    IconButton,
}

#[derive(Clone, Debug)]
struct WidgetNode {
    kind: WidgetKind,
    kind_name: &'static str,
    id: String,
    explicit_id: bool,
    text: String,
    placeholder: String,
    checked: bool,
    value_num: f64,
    min_num: f64,
    max_num: f64,
    step_num: f64,
    options: Vec<String>,
    selected_idx: usize,
    image_val: Option<Value>,
    slice: [i32; 4],
    width_hint: Option<i32>,
    height_hint: Option<i32>,
    padding: Option<i32>,
    spacing: Option<i32>,
    bg_color: Option<u32>,
    text_color: Option<u32>,
    border_color: Option<u32>,
    disabled: bool,
    password: bool,
    on_click: Option<Value>,
    on_change: Option<Value>,
    on_submit: Option<Value>,
    children: Vec<WidgetNode>,
}

#[derive(Clone, Debug, Default)]
struct WidgetRuntimeState {
    text_val: Option<String>,
    last_text_prop: Option<String>,
    checked_val: Option<bool>,
    last_checked_prop: Option<bool>,
    slider_val: Option<f64>,
    last_slider_prop: Option<f64>,
    select_idx: Option<usize>,
    last_select_prop: Option<usize>,
}

#[derive(Clone, Debug)]
struct TextDrawCmd {
    x: i32,
    y: i32,
    color: u32,
    text: String,
}

#[derive(Clone, Debug)]
struct PendingClick {
    x: i32,
    y: i32,
}

struct UiSession {
    width: i32,
    height: i32,
    min_width: i32,
    min_height: i32,
    dpi_scale: f64,
    open: bool,
    theme: Theme,
    buffer: Vec<u32>,
    texts: Vec<TextDrawCmd>,
    mouse_x: i32,
    mouse_y: i32,
    mouse_down: bool,
    prev_mouse_down: bool,
    focused_id: Option<String>,
    active_slider: Option<String>,
    typed_chars: Vec<char>,
    pressed_vks: Vec<i32>,
    pending_clicks: Vec<PendingClick>,
    events: Vec<Value>,
    widgets: HashMap<String, WidgetRuntimeState>,
    next_auto_id: usize,
    #[cfg(windows)]
    win32: Option<win32_ui::Win32Window>,
}

impl UiSession {
    fn new(width: i32, height: i32, theme: Theme) -> Self {
        Self {
            width,
            height,
            min_width: 240,
            min_height: 160,
            dpi_scale: 1.0,
            open: true,
            theme,
            buffer: vec![0u32; (width.max(1) * height.max(1)) as usize],
            texts: Vec::new(),
            mouse_x: -1,
            mouse_y: -1,
            mouse_down: false,
            prev_mouse_down: false,
            focused_id: None,
            active_slider: None,
            typed_chars: Vec::new(),
            pressed_vks: Vec::new(),
            pending_clicks: Vec::new(),
            events: Vec::new(),
            widgets: HashMap::new(),
            next_auto_id: 0,
            #[cfg(windows)]
            win32: None,
        }
    }
}

thread_local! {
    static UI_SESSION: RefCell<Option<UiSession>> = const { RefCell::new(None) };
}

pub fn reset_state() {
    UI_SESSION.with(|slot| {
        #[cfg(windows)]
        if let Some(mut sess) = slot.borrow_mut().take() {
            if let Some(w) = sess.win32.take() {
                w.destroy();
            }
            return;
        }
        *slot.borrow_mut() = None;
    });
}

pub fn module() -> Value {
    build_module(
        "ui",
        &[
            ("rgb", mod_rgb),
            ("hex", mod_hex),
            ("color", mod_color),
            ("theme", mod_theme),
            ("column", mod_column),
            ("row", mod_row),
            ("label", mod_label),
            ("button", mod_button),
            ("text_input", mod_text_input),
            ("checkbox", mod_checkbox),
            ("slider", mod_slider),
            ("select", mod_select),
            ("spacer", mod_spacer),
            ("rect", mod_rect),
            ("image", mod_image),
            ("nine_slice", mod_nine_slice),
            ("icon_button", mod_icon_button),
            ("window", mod_window),
            ("is_open", mod_is_open),
            ("close", mod_close),
            ("width", mod_width),
            ("height", mod_height),
            ("dpi_scale", mod_dpi_scale),
            ("set_theme", mod_set_theme),
            ("render", mod_render),
            ("events", mod_events),
            ("layout", mod_layout),
            ("state", mod_state),
            ("set_state", mod_set_state),
            ("inject_click", mod_inject_click),
            ("inject_key", mod_inject_key),
            ("inject_resize", mod_inject_resize),
            ("wait", mod_wait),
        ],
        &[],
    )
}

fn parse_color_val(val: &Value, theme: &Theme) -> Result<u32, String> {
    match val {
        Value::Int(n) => Ok((*n as u32) & 0x00FF_FFFF),
        Value::Str(s) => parse_color_str(s, theme),
        Value::List(items) => {
            let b = items.borrow();
            if b.len() != 3 {
                return Err("Color list must have 3 elements [r, g, b]".to_string());
            }
            let r = as_i32(&b[0], "color[0]")?.clamp(0, 255) as u32;
            let g = as_i32(&b[1], "color[1]")?.clamp(0, 255) as u32;
            let bl = as_i32(&b[2], "color[2]")?.clamp(0, 255) as u32;
            Ok((r << 16) | (g << 8) | bl)
        }
        other => Err(format!(
            "Expected color (#RRGGBB, int, or [r,g,b]), got '{}'",
            other.type_name()
        )),
    }
}

pub(crate) fn parse_color_str(s: &str, theme: &Theme) -> Result<u32, String> {
    let trimmed = s.trim();
    if let Some(hex) = trimmed.strip_prefix('#') {
        if hex.len() == 6 {
            return u32::from_str_radix(hex, 16).map_err(|_| format!("Invalid hex color: '{}'", s));
        }
        if hex.len() == 3 {
            let mut full = String::with_capacity(6);
            for ch in hex.chars() {
                full.push(ch);
                full.push(ch);
            }
            return u32::from_str_radix(&full, 16)
                .map_err(|_| format!("Invalid short hex color: '{}'", s));
        }
        return Err(format!("Invalid hex color length: '{}'", s));
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "bg" | "background" => Ok(theme.bg),
        "surface" => Ok(theme.surface),
        "border" => Ok(theme.border),
        "text" => Ok(theme.text),
        "muted" => Ok(theme.muted),
        "accent" | "primary" => Ok(theme.accent),
        "white" => Ok(0xFF_FF_FF),
        "black" => Ok(0x00_00_00),
        "red" | "danger" => Ok(0xE0_52_52),
        "green" | "success" => Ok(0x3F_B9_50),
        "blue" => Ok(0x4F_8F_F7),
        "yellow" | "warning" => Ok(0xE3_B3_41),
        "gray" | "grey" => Ok(0x8B_94_9E),
        _ => Err(format!("Unknown color name or format: '{}'", s)),
    }
}

fn as_i32(val: &Value, ctx: &str) -> Result<i32, String> {
    match val {
        Value::Int(i) => i32::try_from(*i).map_err(|_| format!("{} integer out of i32 range", ctx)),
        Value::Float(f) if f.is_finite() && *f >= (i32::MIN as f64) && *f <= (i32::MAX as f64) => {
            Ok(*f as i32)
        }
        other => Err(format!("{} expects int, got '{}'", ctx, other.type_name())),
    }
}

fn as_f64(val: &Value, ctx: &str) -> Result<f64, String> {
    match val {
        Value::Int(i) => Ok(*i as f64),
        Value::Float(f) if f.is_finite() => Ok(*f),
        other => Err(format!(
            "{} expects number, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

fn as_str<'a>(val: &'a Value, ctx: &str) -> Result<&'a str, String> {
    match val {
        Value::Str(s) => Ok(s.as_str()),
        other => Err(format!(
            "{} expects string, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

fn theme_to_value(t: &Theme) -> Value {
    let mut map = crate::value::new_dict_with_capacity(14);
    map.insert("bg".to_string(), Value::Int(t.bg as i64));
    map.insert("surface".to_string(), Value::Int(t.surface as i64));
    map.insert(
        "surface_hover".to_string(),
        Value::Int(t.surface_hover as i64),
    );
    map.insert(
        "surface_active".to_string(),
        Value::Int(t.surface_active as i64),
    );
    map.insert("border".to_string(), Value::Int(t.border as i64));
    map.insert(
        "border_focus".to_string(),
        Value::Int(t.border_focus as i64),
    );
    map.insert("text".to_string(), Value::Int(t.text as i64));
    map.insert("muted".to_string(), Value::Int(t.muted as i64));
    map.insert("accent".to_string(), Value::Int(t.accent as i64));
    map.insert(
        "accent_hover".to_string(),
        Value::Int(t.accent_hover as i64),
    );
    map.insert("accent_text".to_string(), Value::Int(t.accent_text as i64));
    map.insert("padding".to_string(), Value::Int(t.padding as i64));
    map.insert("spacing".to_string(), Value::Int(t.spacing as i64));
    map.insert("font_size".to_string(), Value::Int(t.font_size as i64));
    Value::dict(map)
}

fn parse_theme(val: Option<&Value>, base: &Theme) -> Result<Theme, String> {
    let Some(v) = val else {
        return Ok(base.clone());
    };
    match v {
        Value::Str(name) => match name.to_ascii_lowercase().as_str() {
            "dark" | "default" => Ok(Theme::default()),
            "light" => Ok(Theme::light()),
            other => Err(format!("Unknown built-in theme: '{}'", other)),
        },
        Value::Dict(d) => {
            let map = d.borrow();
            let mut t = if let Some(Value::Str(preset)) = map.get("preset") {
                if preset.eq_ignore_ascii_case("light") {
                    Theme::light()
                } else {
                    Theme::default()
                }
            } else {
                base.clone()
            };
            if let Some(x) = map.get("bg") {
                t.bg = parse_color_val(x, &t)?;
            }
            if let Some(x) = map.get("surface") {
                t.surface = parse_color_val(x, &t)?;
            }
            if let Some(x) = map.get("surface_hover") {
                t.surface_hover = parse_color_val(x, &t)?;
            }
            if let Some(x) = map.get("surface_active") {
                t.surface_active = parse_color_val(x, &t)?;
            }
            if let Some(x) = map.get("border") {
                t.border = parse_color_val(x, &t)?;
            }
            if let Some(x) = map.get("border_focus") {
                t.border_focus = parse_color_val(x, &t)?;
            }
            if let Some(x) = map.get("text") {
                t.text = parse_color_val(x, &t)?;
            }
            if let Some(x) = map.get("muted") {
                t.muted = parse_color_val(x, &t)?;
            }
            if let Some(x) = map.get("accent") {
                t.accent = parse_color_val(x, &t)?;
            }
            if let Some(x) = map.get("accent_hover") {
                t.accent_hover = parse_color_val(x, &t)?;
            }
            if let Some(x) = map.get("accent_text") {
                t.accent_text = parse_color_val(x, &t)?;
            }
            if let Some(x) = map.get("padding") {
                t.padding = as_i32(x, "theme.padding")?.clamp(0, 128);
            }
            if let Some(x) = map.get("spacing") {
                t.spacing = as_i32(x, "theme.spacing")?.clamp(0, 128);
            }
            if let Some(x) = map.get("font_size") {
                t.font_size = as_i32(x, "theme.font_size")?.clamp(8, 48);
            }
            Ok(t)
        }
        other => Err(format!(
            "ui.theme() expects preset name or dict, got '{}'",
            other.type_name()
        )),
    }
}

fn mod_rgb(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("ui.rgb() expects 3 arguments (r, g, b)".to_string());
    }
    let r = as_i32(&args[0], "ui.rgb")?.clamp(0, 255) as u32;
    let g = as_i32(&args[1], "ui.rgb")?.clamp(0, 255) as u32;
    let b = as_i32(&args[2], "ui.rgb")?.clamp(0, 255) as u32;
    Ok(Value::Int(((r << 16) | (g << 8) | b) as i64))
}

fn mod_hex(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("ui.hex() expects 1 string argument".to_string());
    }
    let s = as_str(&args[0], "ui.hex")?;
    let c = parse_color_str(s, &Theme::default())?;
    Ok(Value::Int(c as i64))
}

fn mod_color(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("ui.color() expects 1 argument".to_string());
    }
    let c = parse_color_val(&args[0], &Theme::default())?;
    Ok(Value::Int(c as i64))
}

fn mod_theme(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 1 {
        return Err("ui.theme() expects 0 or 1 argument".to_string());
    }
    let t = parse_theme(args.first(), &Theme::default())?;
    Ok(theme_to_value(&t))
}

fn make_node_dict(
    kind: &str,
    opts: Option<&Value>,
    mut fields: crate::value::DictMap,
) -> Result<Value, String> {
    fields.insert("__ui_node".to_string(), Value::from_str(kind));
    if let Some(opt_val) = opts {
        let Value::Dict(d) = opt_val else {
            return Err(format!("ui.{}() options must be a dict", kind));
        };
        for (k, v) in d.borrow().iter() {
            if k != "__ui_node" {
                fields.insert(k.clone(), v.clone());
            }
        }
    }
    Ok(Value::dict(fields))
}

fn mod_column(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("ui.column() expects (children, [opts])".to_string());
    }
    let mut fields = crate::value::new_dict_with_capacity(2);
    fields.insert("children".to_string(), args[0].clone());
    make_node_dict("column", args.get(1), fields)
}

fn mod_row(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("ui.row() expects (children, [opts])".to_string());
    }
    let mut fields = crate::value::new_dict_with_capacity(2);
    fields.insert("children".to_string(), args[0].clone());
    make_node_dict("row", args.get(1), fields)
}

fn mod_label(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("ui.label() expects (text, [opts])".to_string());
    }
    let mut fields = crate::value::new_dict_with_capacity(2);
    fields.insert("text".to_string(), Value::from_str(args[0].to_string()));
    make_node_dict("label", args.get(1), fields)
}

fn mod_button(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("ui.button() expects (text, [opts])".to_string());
    }
    let mut fields = crate::value::new_dict_with_capacity(2);
    fields.insert("text".to_string(), Value::from_str(args[0].to_string()));
    make_node_dict("button", args.get(1), fields)
}

fn mod_text_input(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("ui.text_input() expects (value, [opts])".to_string());
    }
    let mut fields = crate::value::new_dict_with_capacity(2);
    fields.insert("value".to_string(), Value::from_str(args[0].to_string()));
    make_node_dict("text_input", args.get(1), fields)
}

fn mod_checkbox(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("ui.checkbox() expects (label, checked, [opts])".to_string());
    }
    let checked = matches!(args[1], Value::Bool(true));
    let mut fields = crate::value::new_dict_with_capacity(3);
    fields.insert("text".to_string(), Value::from_str(args[0].to_string()));
    fields.insert("checked".to_string(), Value::Bool(checked));
    make_node_dict("checkbox", args.get(2), fields)
}

fn mod_slider(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 3 || args.len() > 4 {
        return Err("ui.slider() expects (value, min, max, [opts])".to_string());
    }
    let val = as_f64(&args[0], "ui.slider value")?;
    let min = as_f64(&args[1], "ui.slider min")?;
    let max = as_f64(&args[2], "ui.slider max")?;
    if min > max {
        return Err("ui.slider() min cannot be greater than max".to_string());
    }
    let mut fields = crate::value::new_dict_with_capacity(4);
    fields.insert("value".to_string(), Value::Float(val.clamp(min, max)));
    fields.insert("min".to_string(), Value::Float(min));
    fields.insert("max".to_string(), Value::Float(max));
    make_node_dict("slider", args.get(3), fields)
}

fn mod_select(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("ui.select() expects (options, selected, [opts])".to_string());
    }
    let mut fields = crate::value::new_dict_with_capacity(3);
    fields.insert("options".to_string(), args[0].clone());
    fields.insert("selected".to_string(), args[1].clone());
    make_node_dict("select", args.get(2), fields)
}

fn mod_spacer(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 1 {
        return Err("ui.spacer() expects 0 or 1 argument (size)".to_string());
    }
    let mut fields = crate::value::new_dict_map();
    if let Some(sz) = args.first() {
        fields.insert("size".to_string(), sz.clone());
    }
    make_node_dict("spacer", None, fields)
}

fn mod_rect(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() > 1 {
        return Err("ui.rect() expects 0 or 1 argument (opts)".to_string());
    }
    let fields = crate::value::new_dict_map();
    make_node_dict("rect", args.first(), fields)
}

fn validate_ui_image_val(v: &Value, ctx: &str) -> Result<Value, String> {
    match v {
        Value::Image(_) | Value::Texture(_) | Value::Int(_) => Ok(v.clone()),
        other => Err(format!(
            "{} expects image, texture, or surface handle int, got '{}'",
            ctx,
            other.type_name()
        )),
    }
}

fn mod_image(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.is_empty() || args.len() > 2 {
        return Err("ui.image() expects (handle, [opts])".to_string());
    }
    let img = validate_ui_image_val(&args[0], "ui.image handle")?;
    let mut fields = crate::value::new_dict_with_capacity(2);
    fields.insert("image".to_string(), img);
    make_node_dict("image", args.get(1), fields)
}

fn mod_nine_slice(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 5 || args.len() > 6 {
        return Err("ui.nine_slice() expects (handle, left, top, right, bottom, [opts])".to_string());
    }
    let img = validate_ui_image_val(&args[0], "ui.nine_slice handle")?;
    let left = as_i32(&args[1], "ui.nine_slice left")?.max(0);
    let top = as_i32(&args[2], "ui.nine_slice top")?.max(0);
    let right = as_i32(&args[3], "ui.nine_slice right")?.max(0);
    let bottom = as_i32(&args[4], "ui.nine_slice bottom")?.max(0);
    let mut fields = crate::value::new_dict_with_capacity(6);
    fields.insert("image".to_string(), img);
    fields.insert("slice_l".to_string(), Value::Int(left as i64));
    fields.insert("slice_t".to_string(), Value::Int(top as i64));
    fields.insert("slice_r".to_string(), Value::Int(right as i64));
    fields.insert("slice_b".to_string(), Value::Int(bottom as i64));
    make_node_dict("nine_slice", args.get(5), fields)
}

fn mod_icon_button(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 2 || args.len() > 3 {
        return Err("ui.icon_button() expects (handle, text, [opts])".to_string());
    }
    let img = validate_ui_image_val(&args[0], "ui.icon_button handle")?;
    let mut fields = crate::value::new_dict_with_capacity(3);
    fields.insert("image".to_string(), img);
    fields.insert("text".to_string(), Value::from_str(args[1].to_string()));
    make_node_dict("icon_button", args.get(2), fields)
}

fn parse_widget_node(
    val: &Value,
    theme: &Theme,
    auto_id: &mut usize,
    node_count: &mut usize,
    depth: usize,
) -> Result<WidgetNode, String> {
    if depth > 64 {
        return Err("UI widget tree exceeds maximum nesting depth (64)".to_string());
    }
    *node_count += 1;
    if *node_count > 10_000 {
        return Err("UI widget tree exceeds maximum node limit (10000)".to_string());
    }
    let Value::Dict(d) = val else {
        return Err(format!(
            "UI element must be a widget dict, got '{}'",
            val.type_name()
        ));
    };
    let map = d.borrow();
    let kind_str = match map.get("__ui_node") {
        Some(Value::Str(s)) => s.as_str(),
        _ => return Err("Dict is not a valid UI widget (missing __ui_node)".to_string()),
    };
    let (kind, kind_name): (WidgetKind, &'static str) = match kind_str {
        "column" => (WidgetKind::Column, "column"),
        "row" => (WidgetKind::Row, "row"),
        "label" => (WidgetKind::Label, "label"),
        "button" => (WidgetKind::Button, "button"),
        "text_input" => (WidgetKind::TextInput, "text_input"),
        "checkbox" => (WidgetKind::Checkbox, "checkbox"),
        "slider" => (WidgetKind::Slider, "slider"),
        "select" => (WidgetKind::Select, "select"),
        "spacer" => (WidgetKind::Spacer, "spacer"),
        "rect" => (WidgetKind::Rect, "rect"),
        "image" => (WidgetKind::Image, "image"),
        "nine_slice" => (WidgetKind::NineSlice, "nine_slice"),
        "icon_button" => (WidgetKind::IconButton, "icon_button"),
        other => return Err(format!("Unknown UI widget kind: '{}'", other)),
    };

    let (id, explicit_id) = if let Some(Value::Str(s)) = map.get("id") {
        (s.as_ref().clone(), true)
    } else {
        *auto_id += 1;
        (format!("__auto_{}_{}", kind_name, *auto_id), false)
    };

    let text = match kind {
        WidgetKind::TextInput => map.get("value").map(|v| v.to_string()).unwrap_or_default(),
        _ => map.get("text").map(|v| v.to_string()).unwrap_or_default(),
    };
    let placeholder = map
        .get("placeholder")
        .map(|v| v.to_string())
        .unwrap_or_default();
    let checked = matches!(map.get("checked"), Some(Value::Bool(true)));
    let min_num = match map.get("min") {
        Some(v) => as_f64(v, "slider.min")?,
        None => 0.0,
    };
    let max_num = match map.get("max") {
        Some(v) => as_f64(v, "slider.max")?,
        None => 100.0,
    };
    if matches!(kind, WidgetKind::Slider) && min_num > max_num {
        return Err("slider.min cannot be greater than slider.max".to_string());
    }
    let value_num = match map.get("value") {
        Some(v) if matches!(kind, WidgetKind::Slider) => as_f64(v, "slider.value")?,
        _ => min_num,
    };
    let step_num = match map.get("step") {
        Some(v) => as_f64(v, "slider.step")?.abs(),
        None => 0.0,
    };

    let mut options = Vec::new();
    if let Some(Value::List(list)) = map.get("options") {
        let items = list.borrow();
        if items.len() > 10_000 {
            return Err("ui.select() options list exceeds 10000 items limit".to_string());
        }
        for item in items.iter() {
            options.push(item.to_string());
        }
    }
    let selected_idx = match map.get("selected") {
        Some(Value::Int(i)) => {
            if options.is_empty() {
                0
            } else {
                (*i).clamp(0, options.len() as i64 - 1) as usize
            }
        }
        Some(Value::Str(s)) => options.iter().position(|o| o == s.as_str()).unwrap_or(0),
        _ => 0,
    };

    let size_opt = match map.get("size") {
        Some(v) => Some(as_i32(v, "widget.size")?.clamp(0, 4096)),
        None => None,
    };
    let width_hint = match map.get("width") {
        Some(v) => Some(as_i32(v, "widget.width")?.clamp(16, 4096)),
        None => size_opt,
    };
    let height_hint = match map.get("height") {
        Some(v) => Some(as_i32(v, "widget.height")?.clamp(8, 4096)),
        None => size_opt,
    };
    let padding = match map.get("padding") {
        Some(v) => Some(as_i32(v, "widget.padding")?.clamp(0, 128)),
        None => None,
    };
    let spacing = match map.get("spacing") {
        Some(v) => Some(as_i32(v, "widget.spacing")?.clamp(0, 128)),
        None => None,
    };
    let bg_color = match map.get("bg") {
        Some(v) => Some(parse_color_val(v, theme)?),
        None => None,
    };
    let text_color = match map.get("color") {
        Some(v) => Some(parse_color_val(v, theme)?),
        None => None,
    };
    let border_color = match map.get("border") {
        Some(v) => Some(parse_color_val(v, theme)?),
        None => None,
    };
    let disabled = matches!(map.get("disabled"), Some(Value::Bool(true)));
    let password = matches!(map.get("password"), Some(Value::Bool(true)));
    let image_val = match map.get("image") {
        Some(v) => Some(validate_ui_image_val(v, "widget.image")?),
        None => None,
    };
    let slice = [
        match map.get("slice_l") {
            Some(v) => as_i32(v, "widget.slice_l")?.clamp(0, 2048),
            None => 0,
        },
        match map.get("slice_t") {
            Some(v) => as_i32(v, "widget.slice_t")?.clamp(0, 2048),
            None => 0,
        },
        match map.get("slice_r") {
            Some(v) => as_i32(v, "widget.slice_r")?.clamp(0, 2048),
            None => 0,
        },
        match map.get("slice_b") {
            Some(v) => as_i32(v, "widget.slice_b")?.clamp(0, 2048),
            None => 0,
        },
    ];

    let on_click = map.get("on_click").cloned();
    let on_change = map.get("on_change").cloned();
    let on_submit = map.get("on_submit").cloned();

    let mut children = Vec::new();
    if let Some(Value::List(list)) = map.get("children") {
        for ch in list.borrow().iter() {
            children.push(parse_widget_node(
                ch,
                theme,
                auto_id,
                node_count,
                depth + 1,
            )?);
        }
    }

    Ok(WidgetNode {
        kind,
        kind_name,
        id,
        explicit_id,
        text,
        placeholder,
        checked,
        value_num,
        min_num,
        max_num,
        step_num,
        options,
        selected_idx,
        image_val,
        slice,
        width_hint,
        height_hint,
        padding,
        spacing,
        bg_color,
        text_color,
        border_color,
        disabled,
        password,
        on_click,
        on_change,
        on_submit,
        children,
    })
}

fn measure_node(node: &WidgetNode, theme: &Theme) -> (i32, i32) {
    let f = theme.font_size;
    let base_h = (f + 16).max(28);
    let text_chars = node.text.chars().count().min(4096) as i32;
    match node.kind {
        WidgetKind::Label => {
            let w = node
                .width_hint
                .unwrap_or_else(|| text_chars * (f * 6 / 10).max(7) + 8);
            let h = node.height_hint.unwrap_or(f + 8);
            (w, h)
        }
        WidgetKind::Button => {
            let text_w = text_chars * (f * 6 / 10).max(8) + 28;
            let w = node.width_hint.unwrap_or_else(|| text_w.max(96));
            let h = node.height_hint.unwrap_or(base_h.max(34));
            (w, h)
        }
        WidgetKind::TextInput => {
            let w = node.width_hint.unwrap_or(220);
            let h = node.height_hint.unwrap_or(base_h.max(34));
            (w, h)
        }
        WidgetKind::Checkbox => {
            let label_w = text_chars * (f * 6 / 10).max(7);
            let w = node.width_hint.unwrap_or(28 + label_w + 8);
            let h = node.height_hint.unwrap_or(26.max(f + 8));
            (w, h)
        }
        WidgetKind::Slider => {
            let w = node.width_hint.unwrap_or(200);
            let h = node.height_hint.unwrap_or(28);
            (w, h)
        }
        WidgetKind::Select => {
            let longest = node
                .options
                .iter()
                .map(|o| o.chars().count())
                .max()
                .unwrap_or(6)
                .min(4096) as i32;
            let w = node
                .width_hint
                .unwrap_or_else(|| (longest * 8 + 44).max(160));
            let h = node.height_hint.unwrap_or(base_h.max(34));
            (w, h)
        }
        WidgetKind::Spacer => (
            node.width_hint.unwrap_or(12),
            node.height_hint.unwrap_or(12),
        ),
        WidgetKind::Rect => (
            node.width_hint.unwrap_or(32),
            node.height_hint.unwrap_or(32),
        ),
        WidgetKind::Image | WidgetKind::NineSlice => {
            let (iw, ih) = node
                .image_val
                .as_ref()
                .and_then(crate::modules::gfx::value_dims)
                .unwrap_or((32, 32));
            (
                node.width_hint.unwrap_or(iw.max(1)),
                node.height_hint.unwrap_or(ih.max(1)),
            )
        }
        WidgetKind::IconButton => {
            let icon_side = (base_h - 8).max(16);
            let label_w = if node.text.is_empty() {
                0
            } else {
                text_chars * (f * 6 / 10).max(8) + 8
            };
            let w = node.width_hint.unwrap_or_else(|| (16 + icon_side + label_w).max(40));
            let h = node.height_hint.unwrap_or(base_h.max(34));
            (w, h)
        }
        WidgetKind::Column => {
            let pad = node.padding.unwrap_or(0);
            let sp = node.spacing.unwrap_or(theme.spacing);
            let mut max_w = 0i32;
            let mut sum_h = 0i32;
            for (idx, ch) in node.children.iter().enumerate() {
                let (cw, ch_h) = measure_node(ch, theme);
                max_w = max_w.max(cw);
                sum_h = sum_h.saturating_add(ch_h);
                if idx + 1 < node.children.len() {
                    sum_h = sum_h.saturating_add(sp);
                }
            }
            (
                node.width_hint.unwrap_or(max_w.saturating_add(pad * 2)),
                node.height_hint.unwrap_or(sum_h.saturating_add(pad * 2)),
            )
        }
        WidgetKind::Row => {
            let pad = node.padding.unwrap_or(0);
            let sp = node.spacing.unwrap_or(theme.spacing);
            let mut sum_w = 0i32;
            let mut max_h = 0i32;
            for (idx, ch) in node.children.iter().enumerate() {
                let (cw, ch_h) = measure_node(ch, theme);
                sum_w = sum_w.saturating_add(cw);
                if idx + 1 < node.children.len() {
                    sum_w = sum_w.saturating_add(sp);
                }
                max_h = max_h.max(ch_h);
            }
            (
                node.width_hint.unwrap_or(sum_w.saturating_add(pad * 2)),
                node.height_hint.unwrap_or(max_h.saturating_add(pad * 2)),
            )
        }
    }
}

fn layout_node(
    node: &WidgetNode,
    x: i32,
    y: i32,
    avail_w: i32,
    avail_h: i32,
    theme: &Theme,
    out: &mut Vec<LayoutRect>,
) {
    let (mw, mh) = measure_node(node, theme);
    let w = match node.kind {
        WidgetKind::Column | WidgetKind::Row => node.width_hint.unwrap_or(avail_w).min(avail_w),
        WidgetKind::TextInput | WidgetKind::Slider | WidgetKind::Button | WidgetKind::Select => {
            node.width_hint.unwrap_or_else(|| mw.min(avail_w))
        }
        _ => mw.min(avail_w.max(mw)),
    };
    let h = mh.min(avail_h.max(mh));

    out.push(LayoutRect {
        id: if node.explicit_id {
            Some(node.id.clone())
        } else {
            None
        },
        kind: node.kind_name.to_string(),
        x,
        y,
        w,
        h,
    });

    match node.kind {
        WidgetKind::Column => {
            let pad = node.padding.unwrap_or(0);
            let sp = node.spacing.unwrap_or(theme.spacing);
            let inner_w = (w - pad * 2).max(16);
            let mut cy = y + pad;
            let end_y = y + avail_h - pad;
            for ch in &node.children {
                let (_, ch_h) = measure_node(ch, theme);
                let rem_h = (end_y - cy).max(0);
                layout_node(ch, x + pad, cy, inner_w, rem_h, theme, out);
                cy += ch_h + sp;
            }
        }
        WidgetKind::Row => {
            let pad = node.padding.unwrap_or(0);
            let sp = node.spacing.unwrap_or(theme.spacing);
            let inner_h = (h - pad * 2).max(16);
            let mut cx = x + pad;
            let end_x = x + avail_w - pad;
            for ch in &node.children {
                let (cw, _) = measure_node(ch, theme);
                let rem_w = (end_x - cx).max(0);
                layout_node(ch, cx, y + pad, cw.min(rem_w.max(cw)), inner_h, theme, out);
                cx += cw + sp;
            }
        }
        _ => {}
    }
}

fn fill_rect(buf: &mut [u32], bw: i32, bh: i32, x: i32, y: i32, w: i32, h: i32, color: u32) {
    if bw <= 0 || bh <= 0 || w <= 0 || h <= 0 {
        return;
    }
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = x.saturating_add(w).min(bw);
    let y1 = y.saturating_add(h).min(bh);
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let bw_usize = bw as usize;
    for py in y0..y1 {
        let Some(row) = (py as usize).checked_mul(bw_usize) else {
            break;
        };
        for px in x0..x1 {
            if let Some(cell) = buf.get_mut(row + px as usize) {
                *cell = color;
            }
        }
    }
}

fn stroke_rect(buf: &mut [u32], bw: i32, bh: i32, x: i32, y: i32, w: i32, h: i32, color: u32) {
    if w <= 0 || h <= 0 {
        return;
    }
    fill_rect(buf, bw, bh, x, y, w, 1, color);
    fill_rect(
        buf,
        bw,
        bh,
        x,
        y.saturating_add(h).saturating_sub(1),
        w,
        1,
        color,
    );
    fill_rect(buf, bw, bh, x, y, 1, h, color);
    fill_rect(
        buf,
        bw,
        bh,
        x.saturating_add(w).saturating_sub(1),
        y,
        1,
        h,
        color,
    );
}

#[allow(clippy::too_many_arguments)]
fn blit_scaled_ui(
    buf: &mut [u32],
    bw: i32,
    bh: i32,
    dx: i32,
    dy: i32,
    dw: i32,
    dh: i32,
    src: &[u32],
    sw: i32,
    _sh: i32,
    sx0: i32,
    sy0: i32,
    sub_w: i32,
    sub_h: i32,
) {
    if dw <= 0 || dh <= 0 || sub_w <= 0 || sub_h <= 0 || bw <= 0 || bh <= 0 {
        return;
    }
    let x0 = dx.max(0);
    let y0 = dy.max(0);
    let x1 = dx.saturating_add(dw).min(bw);
    let y1 = dy.saturating_add(dh).min(bh);
    for py in y0..y1 {
        let sy = sy0 + (((py - dy) as i64 * sub_h as i64) / dh as i64).clamp(0, sub_h as i64 - 1) as i32;
        let srow = (sy as usize) * (sw as usize);
        let drow = (py as usize) * (bw as usize);
        for px in x0..x1 {
            let sx = sx0 + (((px - dx) as i64 * sub_w as i64) / dw as i64).clamp(0, sub_w as i64 - 1) as i32;
            let sc = src[srow + sx as usize];
            let sa = (sc >> 24) & 0xFF;
            if sa == 0 {
                continue;
            }
            let cell = &mut buf[drow + px as usize];
            if sa == 255 {
                *cell = sc & 0x00FF_FFFF;
            } else {
                let blended = crate::surface::blend_argb_over(0xFF00_0000 | *cell, sc);
                *cell = blended & 0x00FF_FFFF;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn blit_nine_slice_ui(
    buf: &mut [u32],
    bw: i32,
    bh: i32,
    dx: i32,
    dy: i32,
    dw: i32,
    dh: i32,
    src: &[u32],
    sw: i32,
    sh: i32,
    slice: [i32; 4],
) {
    if sw <= 0 || sh <= 0 || dw <= 0 || dh <= 0 {
        return;
    }
    let sl = slice[0].clamp(0, sw / 2);
    let sr = slice[2].clamp(0, sw - sl);
    let st = slice[1].clamp(0, sh / 2);
    let sb = slice[3].clamp(0, sh - st);

    let dl = sl.min(dw);
    let dr = sr.min((dw - dl).max(0));
    let dt = st.min(dh);
    let db = sb.min((dh - dt).max(0));

    let sx_bounds = [0, sl, sw - sr, sw];
    let sy_bounds = [0, st, sh - sb, sh];
    let dx_bounds = [dx, dx + dl, dx + dw - dr, dx + dw];
    let dy_bounds = [dy, dy + dt, dy + dh - db, dy + dh];

    for ry in 0..3 {
        let sy0 = sy_bounds[ry];
        let sub_h = sy_bounds[ry + 1] - sy0;
        let dest_y = dy_bounds[ry];
        let dest_h = dy_bounds[ry + 1] - dest_y;
        for rx in 0..3 {
            let sx0 = sx_bounds[rx];
            let sub_w = sx_bounds[rx + 1] - sx0;
            let dest_x = dx_bounds[rx];
            let dest_w = dx_bounds[rx + 1] - dest_x;
            blit_scaled_ui(buf, bw, bh, dest_x, dest_y, dest_w, dest_h, src, sw, sh, sx0, sy0, sub_w, sub_h);
        }
    }
}

#[cfg(test)]
pub(crate) fn test_peek_pixel(x: i32, y: i32) -> Option<u32> {
    UI_SESSION.with(|slot| {
        let b = slot.borrow();
        let s = b.as_ref()?;
        if x < 0 || y < 0 || x >= s.width || y >= s.height {
            return None;
        }
        s.buffer.get((y as usize) * (s.width as usize) + (x as usize)).copied()
    })
}

fn fill_circle(buf: &mut [u32], bw: i32, bh: i32, cx: i32, cy: i32, r: i32, color: u32) {
    if bw <= 0 || bh <= 0 || r <= 0 {
        return;
    }
    let r = r.min(4096);
    let r2 = (r as i64) * (r as i64);
    for dy in -r..=r {
        let py = cy.saturating_add(dy);
        if py < 0 || py >= bh {
            continue;
        }
        for dx in -r..=r {
            let px = cx.saturating_add(dx);
            if px < 0 || px >= bw || (dx as i64) * (dx as i64) + (dy as i64) * (dy as i64) > r2 {
                continue;
            }
            let idx = (py as usize)
                .saturating_mul(bw as usize)
                .saturating_add(px as usize);
            if let Some(cell) = buf.get_mut(idx) {
                *cell = color;
            }
        }
    }
}

#[derive(Clone)]
struct CallbackInvocation {
    callback: Value,
    args: Vec<Value>,
}

fn sync_widget_state(sess: &mut UiSession, node: &WidgetNode) {
    let entry = sess.widgets.entry(node.id.clone()).or_default();
    match node.kind {
        WidgetKind::TextInput => {
            if entry.text_val.is_none() || entry.last_text_prop.as_ref() != Some(&node.text) {
                entry.text_val = Some(node.text.clone());
                entry.last_text_prop = Some(node.text.clone());
            }
        }
        WidgetKind::Checkbox => {
            if entry.checked_val.is_none() || entry.last_checked_prop != Some(node.checked) {
                entry.checked_val = Some(node.checked);
                entry.last_checked_prop = Some(node.checked);
            }
        }
        WidgetKind::Slider => {
            let prop_changed = entry
                .last_slider_prop
                .map(|p| (p - node.value_num).abs() > 1e-9)
                .unwrap_or(true);
            if entry.slider_val.is_none() || prop_changed {
                let lo = node.min_num.min(node.max_num);
                let hi = node.min_num.max(node.max_num);
                entry.slider_val = Some(node.value_num.clamp(lo, hi));
                entry.last_slider_prop = Some(node.value_num);
            }
        }
        WidgetKind::Select => {
            if entry.select_idx.is_none() || entry.last_select_prop != Some(node.selected_idx) {
                entry.select_idx = Some(node.selected_idx);
                entry.last_select_prop = Some(node.selected_idx);
            }
        }
        WidgetKind::Column | WidgetKind::Row => {
            for ch in &node.children {
                sync_widget_state(sess, ch);
            }
        }
        _ => {}
    }
}

fn point_in_rect(px: i32, py: i32, r: &LayoutRect) -> bool {
    px >= r.x && px < r.x.saturating_add(r.w) && py >= r.y && py < r.y.saturating_add(r.h)
}

fn emit_event(events: &mut Vec<Value>, kind: &str, id: &str, val: Option<Value>) {
    let mut map = crate::value::new_dict_with_capacity(3);
    map.insert("type".to_string(), Value::from_str(kind));
    map.insert("id".to_string(), Value::from_str(id));
    if let Some(v) = val {
        map.insert("value".to_string(), v);
    }
    events.push(Value::dict(map));
}

fn snap_slider(raw: f64, min: f64, max: f64, step: f64) -> f64 {
    let lo = min.min(max);
    let hi = min.max(max);
    let clamped = raw.clamp(lo, hi);
    if step > 0.0 && ((hi - lo) / step).is_finite() {
        let steps = ((clamped - lo) / step).round();
        let snapped = lo + steps * step;
        if snapped.is_finite() {
            snapped.clamp(lo, hi)
        } else {
            clamped
        }
    } else {
        clamped
    }
}

fn blit_ui_media(
    buf: &mut [u32],
    bw: i32,
    bh: i32,
    dx: i32,
    dy: i32,
    dw: i32,
    dh: i32,
    media: &Value,
    slice: Option<[i32; 4]>,
) {
    match media {
        Value::Image(im) => {
            let mut target = crate::media::compose::BlitTarget {
                pixels: buf,
                width: bw,
                height: bh,
                clip: None,
                is_offscreen: false,
            };
            if let Some(sl) = slice {
                crate::media::compose::blit_nine_slice(&mut target, im, dx, dy, dw, dh, sl);
            } else {
                let opts = crate::media::compose::BlitOpts {
                    dst_x: dx as f64,
                    dst_y: dy as f64,
                    dst_w: Some(dw as f64),
                    dst_h: Some(dh as f64),
                    ..crate::media::compose::BlitOpts::default()
                };
                crate::media::compose::blit_image(&mut target, im, &opts);
            }
        }
        Value::Texture(tex) => {
            let mut target = crate::media::compose::BlitTarget {
                pixels: buf,
                width: bw,
                height: bh,
                clip: None,
                is_offscreen: false,
            };
            if let Some(sl) = slice {
                crate::media::compose::blit_nine_slice(&mut target, &tex.image, dx, dy, dw, dh, sl);
            } else {
                let opts = crate::media::compose::BlitOpts {
                    dst_x: dx as f64,
                    dst_y: dy as f64,
                    dst_w: Some(dw as f64),
                    dst_h: Some(dh as f64),
                    filter: tex.filter,
                    ..crate::media::compose::BlitOpts::default()
                };
                crate::media::compose::blit_image(&mut target, &tex.image, &opts);
            }
        }
        Value::Int(id) => {
            if let Ok((sw, sh, src)) = crate::modules::gfx::copy_handle_rgba(*id) {
                if let Some(sl) = slice {
                    blit_nine_slice_ui(buf, bw, bh, dx, dy, dw, dh, &src, sw, sh, sl);
                } else {
                    blit_scaled_ui(buf, bw, bh, dx, dy, dw, dh, &src, sw, sh, 0, 0, sw, sh);
                }
            }
        }
        _ => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn render_and_interact_node(
    node: &WidgetNode,
    rect_idx: &mut usize,
    rects: &[LayoutRect],
    sess: &mut UiSession,
    clicked_points: &[(i32, i32)],
    callbacks: &mut Vec<CallbackInvocation>,
) {
    let rect = rects[*rect_idx].clone();
    *rect_idx += 1;

    let hovered = point_in_rect(sess.mouse_x, sess.mouse_y, &rect);
    let was_clicked = !node.disabled
        && clicked_points
            .iter()
            .any(|&(cx, cy)| point_in_rect(cx, cy, &rect));

    let theme = sess.theme.clone();
    let bw = sess.width.max(1);
    let bh = sess.height.max(1);

    match node.kind {
        WidgetKind::Column | WidgetKind::Row => {
            if let Some(bg) = node.bg_color {
                fill_rect(&mut sess.buffer, bw, bh, rect.x, rect.y, rect.w, rect.h, bg);
            }
            if let Some(border) = node.border_color {
                stroke_rect(
                    &mut sess.buffer,
                    bw,
                    bh,
                    rect.x,
                    rect.y,
                    rect.w,
                    rect.h,
                    border,
                );
            }
            for ch in &node.children {
                render_and_interact_node(ch, rect_idx, rects, sess, clicked_points, callbacks);
            }
        }
        WidgetKind::Spacer => {}
        WidgetKind::Rect => {
            let bg = node.bg_color.unwrap_or(theme.surface);
            fill_rect(&mut sess.buffer, bw, bh, rect.x, rect.y, rect.w, rect.h, bg);
            if let Some(bc) = node.border_color {
                stroke_rect(&mut sess.buffer, bw, bh, rect.x, rect.y, rect.w, rect.h, bc);
            }
        }
        WidgetKind::Image => {
            if let Some(ref media) = node.image_val {
                blit_ui_media(&mut sess.buffer, bw, bh, rect.x, rect.y, rect.w, rect.h, media, None);
            }
            if let Some(bc) = node.border_color {
                stroke_rect(&mut sess.buffer, bw, bh, rect.x, rect.y, rect.w, rect.h, bc);
            }
        }
        WidgetKind::NineSlice => {
            if let Some(ref media) = node.image_val {
                blit_ui_media(&mut sess.buffer, bw, bh, rect.x, rect.y, rect.w, rect.h, media, Some(node.slice));
            }
            if let Some(bc) = node.border_color {
                stroke_rect(&mut sess.buffer, bw, bh, rect.x, rect.y, rect.w, rect.h, bc);
            }
        }
        WidgetKind::IconButton => {
            let bg = if node.disabled {
                theme.surface
            } else if hovered && sess.mouse_down {
                theme.surface_active
            } else if hovered {
                node.bg_color.unwrap_or(theme.accent_hover)
            } else {
                node.bg_color.unwrap_or(theme.accent)
            };
            let fg = if node.disabled {
                theme.muted
            } else {
                node.text_color.unwrap_or(theme.accent_text)
            };
            fill_rect(&mut sess.buffer, bw, bh, rect.x, rect.y, rect.w, rect.h, bg);
            stroke_rect(
                &mut sess.buffer,
                bw,
                bh,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                node.border_color.unwrap_or(theme.border),
            );
            let icon_side = (rect.h - 8).clamp(8, rect.w - 8).max(8);
            let icon_x = rect.x + 6;
            let icon_y = rect.y + (rect.h - icon_side) / 2;
            if let Some(ref media) = node.image_val {
                blit_ui_media(&mut sess.buffer, bw, bh, icon_x, icon_y, icon_side, icon_side, media, None);
            }
            if !node.text.is_empty() {
                sess.texts.push(TextDrawCmd {
                    x: icon_x + icon_side + 8,
                    y: rect.y + (rect.h - theme.font_size).max(4) / 2,
                    color: fg,
                    text: node.text.clone(),
                });
            }
            if was_clicked {
                emit_event(&mut sess.events, "click", &node.id, None);
                if let Some(cb) = &node.on_click {
                    callbacks.push(CallbackInvocation {
                        callback: cb.clone(),
                        args: vec![Value::from_str(node.id.clone())],
                    });
                }
            }
        }
        WidgetKind::Label => {
            let color = node.text_color.unwrap_or(theme.text);
            sess.texts.push(TextDrawCmd {
                x: rect.x + 2,
                y: rect.y + 4,
                color,
                text: node.text.clone(),
            });
        }
        WidgetKind::Button => {
            let bg = if node.disabled {
                theme.surface
            } else if hovered && sess.mouse_down {
                theme.surface_active
            } else if hovered {
                node.bg_color.unwrap_or(theme.accent_hover)
            } else {
                node.bg_color.unwrap_or(theme.accent)
            };
            let fg = if node.disabled {
                theme.muted
            } else {
                node.text_color.unwrap_or(theme.accent_text)
            };
            fill_rect(&mut sess.buffer, bw, bh, rect.x, rect.y, rect.w, rect.h, bg);
            stroke_rect(
                &mut sess.buffer,
                bw,
                bh,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                node.border_color.unwrap_or(theme.border),
            );
            sess.texts.push(TextDrawCmd {
                x: rect.x + 12,
                y: rect.y + (rect.h - theme.font_size).max(4) / 2,
                color: fg,
                text: node.text.clone(),
            });
            if was_clicked {
                emit_event(&mut sess.events, "click", &node.id, None);
                if let Some(cb) = &node.on_click {
                    callbacks.push(CallbackInvocation {
                        callback: cb.clone(),
                        args: vec![Value::from_str(node.id.clone())],
                    });
                }
            }
        }
        WidgetKind::TextInput => {
            if was_clicked {
                sess.focused_id = Some(node.id.clone());
            }
            let focused = sess.focused_id.as_deref() == Some(node.id.as_str());
            let mut text_now = sess
                .widgets
                .get(&node.id)
                .and_then(|w| w.text_val.clone())
                .unwrap_or_else(|| node.text.clone());

            if focused && !node.disabled {
                let mut changed = false;
                let mut submitted = false;
                let chars = std::mem::take(&mut sess.typed_chars);
                let vks = std::mem::take(&mut sess.pressed_vks);
                for ch in chars {
                    if ch == '\x08' {
                        if text_now.pop().is_some() {
                            changed = true;
                        }
                    } else if ch == '\r' || ch == '\n' {
                        submitted = true;
                    } else if !ch.is_control() {
                        text_now.push(ch);
                        changed = true;
                    }
                }
                for vk in vks {
                    if vk == 0x08 && text_now.pop().is_some() {
                        changed = true;
                    } else if vk == 0x0D {
                        submitted = true;
                    }
                }
                if changed {
                    if let Some(st) = sess.widgets.get_mut(&node.id) {
                        st.text_val = Some(text_now.clone());
                    }
                    emit_event(
                        &mut sess.events,
                        "change",
                        &node.id,
                        Some(Value::from_str(text_now.clone())),
                    );
                    if let Some(cb) = &node.on_change {
                        callbacks.push(CallbackInvocation {
                            callback: cb.clone(),
                            args: vec![Value::from_str(text_now.clone())],
                        });
                    }
                }
                if submitted {
                    emit_event(
                        &mut sess.events,
                        "submit",
                        &node.id,
                        Some(Value::from_str(text_now.clone())),
                    );
                    if let Some(cb) = &node.on_submit {
                        callbacks.push(CallbackInvocation {
                            callback: cb.clone(),
                            args: vec![Value::from_str(text_now.clone())],
                        });
                    }
                }
            }

            let bg = node.bg_color.unwrap_or(theme.surface);
            let border = if focused {
                theme.border_focus
            } else {
                node.border_color.unwrap_or(theme.border)
            };
            fill_rect(&mut sess.buffer, bw, bh, rect.x, rect.y, rect.w, rect.h, bg);
            stroke_rect(
                &mut sess.buffer,
                bw,
                bh,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                border,
            );

            let (display_txt, txt_color) = if text_now.is_empty() && !node.placeholder.is_empty() {
                (node.placeholder.clone(), theme.muted)
            } else if node.password {
                ("*".repeat(text_now.chars().count()), theme.text)
            } else {
                (text_now, node.text_color.unwrap_or(theme.text))
            };
            sess.texts.push(TextDrawCmd {
                x: rect.x + 8,
                y: rect.y + (rect.h - theme.font_size).max(4) / 2,
                color: txt_color,
                text: display_txt,
            });
        }
        WidgetKind::Checkbox => {
            let mut is_checked = sess
                .widgets
                .get(&node.id)
                .and_then(|w| w.checked_val)
                .unwrap_or(node.checked);
            if was_clicked {
                is_checked = !is_checked;
                if let Some(st) = sess.widgets.get_mut(&node.id) {
                    st.checked_val = Some(is_checked);
                }
                emit_event(
                    &mut sess.events,
                    "change",
                    &node.id,
                    Some(Value::Bool(is_checked)),
                );
                if let Some(cb) = &node.on_change {
                    callbacks.push(CallbackInvocation {
                        callback: cb.clone(),
                        args: vec![Value::Bool(is_checked)],
                    });
                }
            }
            let box_sz = 18;
            let bx = rect.x + 2;
            let by = rect.y + (rect.h - box_sz) / 2;
            let box_bg = if is_checked {
                theme.accent
            } else {
                theme.surface
            };
            fill_rect(&mut sess.buffer, bw, bh, bx, by, box_sz, box_sz, box_bg);
            stroke_rect(
                &mut sess.buffer,
                bw,
                bh,
                bx,
                by,
                box_sz,
                box_sz,
                theme.border,
            );
            if is_checked {
                fill_rect(
                    &mut sess.buffer,
                    bw,
                    bh,
                    bx + 4,
                    by + 4,
                    box_sz - 8,
                    box_sz - 8,
                    theme.accent_text,
                );
            }
            sess.texts.push(TextDrawCmd {
                x: bx + box_sz + 8,
                y: rect.y + (rect.h - theme.font_size).max(4) / 2,
                color: node.text_color.unwrap_or(theme.text),
                text: node.text.clone(),
            });
        }
        WidgetKind::Slider => {
            let lo = node.min_num.min(node.max_num);
            let hi = node.min_num.max(node.max_num);
            let mut val = sess
                .widgets
                .get(&node.id)
                .and_then(|w| w.slider_val)
                .unwrap_or(node.value_num)
                .clamp(lo, hi);

            let drag_x = if was_clicked {
                Some(clicked_points.last().map(|p| p.0).unwrap_or(sess.mouse_x))
            } else if sess.mouse_down && sess.active_slider.as_deref() == Some(node.id.as_str()) {
                Some(sess.mouse_x)
            } else if sess.mouse_down && !sess.prev_mouse_down && hovered {
                sess.active_slider = Some(node.id.clone());
                Some(sess.mouse_x)
            } else {
                None
            };
            if let Some(mx) = drag_x {
                let track_w = (rect.w - 16).max(1) as f64;
                let ratio = ((mx - (rect.x + 8)) as f64 / track_w).clamp(0.0, 1.0);
                let next = snap_slider(lo + ratio * (hi - lo), lo, hi, node.step_num);
                if (next - val).abs() > 1e-9 {
                    val = next;
                    if let Some(st) = sess.widgets.get_mut(&node.id) {
                        st.slider_val = Some(val);
                    }
                    emit_event(
                        &mut sess.events,
                        "change",
                        &node.id,
                        Some(Value::Float(val)),
                    );
                    if let Some(cb) = &node.on_change {
                        callbacks.push(CallbackInvocation {
                            callback: cb.clone(),
                            args: vec![Value::Float(val)],
                        });
                    }
                }
            }

            let track_x = rect.x + 8;
            let track_w = (rect.w - 16).max(16);
            let track_y = rect.y + rect.h / 2 - 3;
            let span = (node.max_num - node.min_num).abs().max(1e-9);
            let t = ((val - node.min_num) / span).clamp(0.0, 1.0);
            let fill_w = (t * track_w as f64) as i32;
            fill_rect(
                &mut sess.buffer,
                bw,
                bh,
                track_x,
                track_y,
                track_w,
                6,
                theme.surface_active,
            );
            fill_rect(
                &mut sess.buffer,
                bw,
                bh,
                track_x,
                track_y,
                fill_w,
                6,
                theme.accent,
            );
            fill_circle(
                &mut sess.buffer,
                bw,
                bh,
                track_x + fill_w,
                track_y + 3,
                7,
                theme.accent_hover,
            );
        }
        WidgetKind::Select => {
            let mut idx = sess
                .widgets
                .get(&node.id)
                .and_then(|w| w.select_idx)
                .unwrap_or(node.selected_idx);
            if was_clicked && !node.options.is_empty() {
                idx = (idx + 1) % node.options.len();
                if let Some(st) = sess.widgets.get_mut(&node.id) {
                    st.select_idx = Some(idx);
                }
                let chosen = node.options[idx].clone();
                emit_event(
                    &mut sess.events,
                    "change",
                    &node.id,
                    Some(Value::from_str(chosen.clone())),
                );
                if let Some(cb) = &node.on_change {
                    callbacks.push(CallbackInvocation {
                        callback: cb.clone(),
                        args: vec![Value::from_str(chosen)],
                    });
                }
            }
            let bg = if hovered {
                theme.surface_hover
            } else {
                node.bg_color.unwrap_or(theme.surface)
            };
            fill_rect(&mut sess.buffer, bw, bh, rect.x, rect.y, rect.w, rect.h, bg);
            stroke_rect(
                &mut sess.buffer,
                bw,
                bh,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                theme.border,
            );
            let label = node
                .options
                .get(idx)
                .map(|s| format!("{}  v", s))
                .unwrap_or_else(|| "(empty)".to_string());
            sess.texts.push(TextDrawCmd {
                x: rect.x + 10,
                y: rect.y + (rect.h - theme.font_size).max(4) / 2,
                color: node.text_color.unwrap_or(theme.text),
                text: label,
            });
        }
    }
}

fn mod_window(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 3 || args.len() > 4 {
        return Err("ui.window() expects (title, width, height, [opts])".to_string());
    }
    let title = as_str(&args[0], "ui.window title")?.to_string();
    let width = as_i32(&args[1], "ui.window width")?.clamp(120, 4096);
    let height = as_i32(&args[2], "ui.window height")?.clamp(120, 4096);
    vm.charge_alloc(
        (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(std::mem::size_of::<u32>()),
    )?;

    let mut headless = false;
    let mut theme = Theme::default();
    let mut min_w = 240;
    let mut min_h = 160;
    if let Some(opt_val) = args.get(3) {
        let Value::Dict(d) = opt_val else {
            return Err("ui.window() 4th argument must be an options dict".to_string());
        };
        let map = d.borrow();
        if let Some(Value::Bool(h)) = map.get("headless") {
            headless = *h;
        }
        if let Some(tv) = map.get("theme") {
            theme = parse_theme(Some(tv), &theme)?;
        }
        if let Some(mw) = map.get("min_width") {
            min_w = as_i32(mw, "ui.window min_width")?.clamp(80, width);
        }
        if let Some(mh) = map.get("min_height") {
            min_h = as_i32(mh, "ui.window min_height")?.clamp(80, height);
        }
    }

    reset_state();
    let mut sess = UiSession::new(width, height, theme);
    sess.min_width = min_w;
    sess.min_height = min_h;
    #[cfg(windows)]
    if !headless {
        let w32 = win32_ui::Win32Window::create(&title, width, height)?;
        sess.dpi_scale = w32.dpi_scale;
        sess.win32 = Some(w32);
    }

    UI_SESSION.with(|slot| {
        *slot.borrow_mut() = Some(sess);
    });
    Ok(Value::Bool(true))
}

fn mod_is_open(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("ui.is_open() expects 0 arguments".to_string());
    }
    UI_SESSION.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let Some(sess) = borrow.as_mut() else {
            return Ok(Value::Bool(false));
        };
        #[cfg(windows)]
        if let Some(w) = sess.win32.take() {
            w.pump_events(sess);
            sess.win32 = Some(w);
        }
        Ok(Value::Bool(sess.open))
    })
}

fn mod_close(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("ui.close() expects 0 arguments".to_string());
    }
    reset_state();
    Ok(Value::Unit)
}

fn mod_width(_vm: &mut VM, _args: &[Value]) -> Result<Value, String> {
    Ok(Value::Int(UI_SESSION.with(|s| {
        s.borrow().as_ref().map(|x| x.width as i64).unwrap_or(0)
    })))
}

fn mod_height(_vm: &mut VM, _args: &[Value]) -> Result<Value, String> {
    Ok(Value::Int(UI_SESSION.with(|s| {
        s.borrow().as_ref().map(|x| x.height as i64).unwrap_or(0)
    })))
}

fn mod_dpi_scale(_vm: &mut VM, _args: &[Value]) -> Result<Value, String> {
    Ok(Value::Float(UI_SESSION.with(|s| {
        s.borrow().as_ref().map(|x| x.dpi_scale).unwrap_or(1.0)
    })))
}

fn mod_set_theme(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("ui.set_theme() expects 1 argument".to_string());
    }
    UI_SESSION.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let sess = borrow
            .as_mut()
            .ok_or_else(|| "No active ui.window".to_string())?;
        sess.theme = parse_theme(Some(&args[0]), &sess.theme)?;
        Ok(Value::Unit)
    })
}

fn mod_render(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("ui.render() expects 1 argument (root widget)".to_string());
    }
    let callbacks = UI_SESSION.with(|slot| -> Result<Vec<CallbackInvocation>, String> {
        let mut borrow = slot.borrow_mut();
        let sess = borrow
            .as_mut()
            .ok_or_else(|| "No active ui.window; call ui.window() first".to_string())?;

        #[cfg(windows)]
        if let Some(w) = sess.win32.take() {
            w.pump_events(sess);
            sess.win32 = Some(w);
        }

        let mut auto_id = 0usize;
        let mut node_count = 0usize;
        let tree = parse_widget_node(&args[0], &sess.theme, &mut auto_id, &mut node_count, 0)?;
        sess.next_auto_id = auto_id;
        sync_widget_state(sess, &tree);

        let w = sess.width.clamp(1, 4096);
        let h = sess.height.clamp(1, 4096);
        sess.width = w;
        sess.height = h;
        let target_len = (w as usize).saturating_mul(h as usize);
        if target_len > sess.buffer.len() {
            vm.charge_alloc(
                (target_len - sess.buffer.len()).saturating_mul(std::mem::size_of::<u32>()),
            )?;
        }
        if sess.buffer.len() != target_len {
            sess.buffer.resize(target_len, sess.theme.bg);
        }
        sess.buffer.fill(sess.theme.bg);
        sess.texts.clear();

        let pad = sess.theme.padding;
        let mut rects = Vec::new();
        layout_node(
            &tree,
            pad,
            pad,
            (w - pad * 2).max(16),
            (h - pad * 2).max(16),
            &sess.theme,
            &mut rects,
        );

        let mut clicked_points = std::mem::take(&mut sess.pending_clicks)
            .into_iter()
            .map(|c| (c.x, c.y))
            .collect::<Vec<_>>();
        if !sess.mouse_down && sess.prev_mouse_down {
            clicked_points.push((sess.mouse_x, sess.mouse_y));
        }
        if !sess.mouse_down {
            sess.active_slider = None;
        }

        let mut rect_idx = 0usize;
        let mut callbacks = Vec::new();
        render_and_interact_node(
            &tree,
            &mut rect_idx,
            &rects,
            sess,
            &clicked_points,
            &mut callbacks,
        );

        sess.prev_mouse_down = sess.mouse_down;

        #[cfg(windows)]
        if let Some(w32) = sess.win32.as_ref() {
            w32.present(sess)?;
        }

        Ok(callbacks)
    })?;

    for cb in callbacks {
        vm.call_value(cb.callback, &cb.args)?;
    }
    Ok(Value::Unit)
}

fn mod_events(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if !args.is_empty() {
        return Err("ui.events() expects 0 arguments".to_string());
    }
    UI_SESSION.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let evs = borrow
            .as_mut()
            .map(|s| std::mem::take(&mut s.events))
            .unwrap_or_default();
        Ok(Value::List(Rc::new(RefCell::new(evs))))
    })
}

fn mod_layout(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() < 3 || args.len() > 4 {
        return Err("ui.layout() expects (root, width, height, [theme])".to_string());
    }
    let width = as_i32(&args[1], "ui.layout width")?.clamp(16, 4096);
    let height = as_i32(&args[2], "ui.layout height")?.clamp(16, 4096);
    let theme = parse_theme(args.get(3), &Theme::default())?;
    let mut auto_id = 0usize;
    let mut node_count = 0usize;
    let tree = parse_widget_node(&args[0], &theme, &mut auto_id, &mut node_count, 0)?;
    vm.charge_alloc(node_count.saturating_mul(128))?;

    let mut rects = Vec::new();
    let pad = theme.padding;
    layout_node(
        &tree,
        pad,
        pad,
        (width - pad * 2).max(16),
        (height - pad * 2).max(16),
        &theme,
        &mut rects,
    );

    let mut result = crate::value::new_dict_with_capacity(rects.len() + 1);
    let mut list_vals = Vec::with_capacity(rects.len());
    for r in rects {
        let mut item = crate::value::new_dict_with_capacity(5);
        item.insert("kind".to_string(), Value::from_str(r.kind));
        item.insert("x".to_string(), Value::Int(r.x as i64));
        item.insert("y".to_string(), Value::Int(r.y as i64));
        item.insert("w".to_string(), Value::Int(r.w as i64));
        item.insert("h".to_string(), Value::Int(r.h as i64));
        let dict_val = Value::dict(item);
        if let Some(id) = r.id {
            result.insert(id, dict_val.clone());
        }
        list_vals.push(dict_val);
    }
    result.insert(
        "_nodes".to_string(),
        Value::List(Rc::new(RefCell::new(list_vals))),
    );
    Ok(Value::dict(result))
}

fn mod_state(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("ui.state() expects 1 argument (widget id)".to_string());
    }
    let id = as_str(&args[0], "ui.state")?;
    UI_SESSION.with(|slot| {
        let borrow = slot.borrow();
        let Some(sess) = borrow.as_ref() else {
            return Ok(Value::Unit);
        };
        let Some(st) = sess.widgets.get(id) else {
            return Ok(Value::Unit);
        };
        if let Some(t) = &st.text_val {
            return Ok(Value::from_str(t.clone()));
        }
        if let Some(b) = st.checked_val {
            return Ok(Value::Bool(b));
        }
        if let Some(f) = st.slider_val {
            return Ok(Value::Float(f));
        }
        if let Some(idx) = st.select_idx {
            return Ok(Value::Int(idx as i64));
        }
        Ok(Value::Unit)
    })
}

fn mod_set_state(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("ui.set_state() expects 2 arguments (id, value)".to_string());
    }
    let id = as_str(&args[0], "ui.set_state")?.to_string();
    UI_SESSION.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let sess = borrow
            .as_mut()
            .ok_or_else(|| "No active ui.window".to_string())?;
        let st = sess.widgets.entry(id).or_default();
        match &args[1] {
            Value::Str(s) => st.text_val = Some(s.as_ref().clone()),
            Value::Bool(b) => st.checked_val = Some(*b),
            Value::Int(i) => {
                st.slider_val = Some(*i as f64);
                st.select_idx = Some((*i).max(0) as usize);
            }
            Value::Float(f) if f.is_finite() => st.slider_val = Some(*f),
            Value::Float(_) => {
                return Err("ui.set_state() numeric value must be finite".to_string());
            }
            _ => {}
        }
        Ok(Value::Unit)
    })
}

fn mod_inject_click(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("ui.inject_click() expects 2 arguments (x, y)".to_string());
    }
    let x = as_i32(&args[0], "ui.inject_click x")?;
    let y = as_i32(&args[1], "ui.inject_click y")?;
    UI_SESSION.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let sess = borrow
            .as_mut()
            .ok_or_else(|| "No active ui.window".to_string())?;
        sess.mouse_x = x;
        sess.mouse_y = y;
        sess.pending_clicks.push(PendingClick { x, y });
        Ok(Value::Unit)
    })
}

fn mod_inject_key(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("ui.inject_key() expects 1 argument (text)".to_string());
    }
    let s = as_str(&args[0], "ui.inject_key")?;
    vm.charge_alloc(s.len())?;
    UI_SESSION.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let sess = borrow
            .as_mut()
            .ok_or_else(|| "No active ui.window".to_string())?;
        match s {
            "backspace" | "Backspace" => {
                if sess.pressed_vks.len() < 65_536 {
                    sess.pressed_vks.push(0x08);
                }
            }
            "enter" | "Enter" => {
                if sess.pressed_vks.len() < 65_536 {
                    sess.pressed_vks.push(0x0D);
                }
            }
            other => {
                let avail = 65_536usize.saturating_sub(sess.typed_chars.len());
                sess.typed_chars.extend(other.chars().take(avail));
            }
        }
        Ok(Value::Unit)
    })
}

fn mod_inject_resize(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("ui.inject_resize() expects 2 arguments (width, height)".to_string());
    }
    let raw_w = as_i32(&args[0], "ui.inject_resize width")?;
    let raw_h = as_i32(&args[1], "ui.inject_resize height")?;
    if raw_w > 4096 || raw_h > 4096 {
        return Err("ui.inject_resize() width and height must not exceed 4096".to_string());
    }
    let w = raw_w.max(16);
    let h = raw_h.max(16);
    UI_SESSION.with(|slot| {
        let mut borrow = slot.borrow_mut();
        let sess = borrow
            .as_mut()
            .ok_or_else(|| "No active ui.window".to_string())?;
        let new_w = w.clamp(sess.min_width.clamp(1, 4096), 4096);
        let new_h = h.clamp(sess.min_height.clamp(1, 4096), 4096);
        let new_bytes = (new_w as usize)
            .saturating_mul(new_h as usize)
            .saturating_mul(std::mem::size_of::<u32>());
        let old_bytes = sess.buffer.len().saturating_mul(std::mem::size_of::<u32>());
        if new_bytes > old_bytes {
            vm.charge_alloc(new_bytes - old_bytes)?;
        }
        sess.width = new_w;
        sess.height = new_h;
        emit_event(&mut sess.events, "resize", "window", None);
        Ok(Value::Unit)
    })
}

fn mod_wait(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("ui.wait() expects 1 argument (ms)".to_string());
    }
    let ms = as_i32(&args[0], "ui.wait")?.max(0) as u64;
    vm.sleep_checked(ms)?;
    Ok(Value::Unit)
}

#[cfg(windows)]
mod win32_ui {
    use super::{UiSession, emit_event};
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};
    use std::sync::Once;

    type Hwnd = *mut c_void;
    type HInstance = *mut c_void;
    type Hdc = *mut c_void;
    type HFont = *mut c_void;
    type HObj = *mut c_void;

    const CS_HREDRAW: u32 = 0x0002;
    const CS_VREDRAW: u32 = 0x0001;
    const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const CW_USEDEFAULT: i32 = -2147483648;
    const WM_DESTROY: u32 = 0x0002;
    const WM_SIZE: u32 = 0x0005;
    const WM_CLOSE: u32 = 0x0010;
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_CHAR: u32 = 0x0102;
    const WM_MOUSEMOVE: u32 = 0x0200;
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_LBUTTONUP: u32 = 0x0202;
    const PM_REMOVE: u32 = 0x0001;
    const BI_RGB: u32 = 0;
    const DIB_RGB_COLORS: u32 = 0;
    const SRCCOPY: u32 = 0x00CC_0020;
    const TRANSPARENT: i32 = 1;
    const IDC_ARROW: *const u16 = 32512 as *const u16;

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
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct Msg {
        hwnd: Hwnd,
        message: u32,
        w_param: usize,
        l_param: isize,
        time: u32,
        pt: Point,
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

    #[allow(clashing_extern_declarations)]
    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterClassExW(lpwcx: *const WndClassExW) -> u16;
        fn CreateWindowExW(
            dw_ex_style: u32,
            lp_class_name: *const u16,
            lp_window_name: *const u16,
            dw_style: u32,
            x: i32,
            y: i32,
            n_width: i32,
            n_height: i32,
            h_wnd_parent: Hwnd,
            h_menu: *mut c_void,
            h_instance: HInstance,
            lp_param: *mut c_void,
        ) -> Hwnd;
        fn DestroyWindow(h_wnd: Hwnd) -> i32;
        fn DefWindowProcW(h_wnd: Hwnd, msg: u32, w_param: usize, l_param: isize) -> isize;
        fn PeekMessageW(lp_msg: *mut Msg, h_wnd: Hwnd, min: u32, max: u32, remove: u32) -> i32;
        fn TranslateMessage(lp_msg: *const Msg) -> i32;
        fn DispatchMessageW(lp_msg: *const Msg) -> isize;
        fn AdjustWindowRect(lp_rect: *mut Rect, dw_style: u32, b_menu: i32) -> i32;
        fn GetDC(h_wnd: Hwnd) -> Hdc;
        fn ReleaseDC(h_wnd: Hwnd, h_dc: Hdc) -> i32;
        fn LoadCursorW(h_instance: HInstance, lp_cursor_name: *const u16) -> *mut c_void;
        fn LoadIconW(h_instance: HInstance, lp_icon_name: *const u16) -> *mut c_void;
        fn SetProcessDPIAware() -> i32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetModuleHandleW(lp_module_name: *const u16) -> HInstance;
    }

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn StretchDIBits(
            hdc: Hdc,
            x_dest: i32,
            y_dest: i32,
            dest_w: i32,
            dest_h: i32,
            x_src: i32,
            y_src: i32,
            src_w: i32,
            src_h: i32,
            lp_bits: *const c_void,
            lpbmi: *const BitmapInfo,
            i_usage: u32,
            rop: u32,
        ) -> i32;
        fn GetDeviceCaps(hdc: Hdc, index: i32) -> i32;
        fn SetBkMode(hdc: Hdc, mode: i32) -> i32;
        fn SetTextColor(hdc: Hdc, color: u32) -> u32;
        fn CreateFontW(
            c_height: i32,
            c_width: i32,
            c_esc: i32,
            c_ori: i32,
            c_weight: i32,
            b_italic: u32,
            b_under: u32,
            b_strike: u32,
            i_charset: u32,
            i_out: u32,
            i_clip: u32,
            i_qual: u32,
            i_pitch: u32,
            psz_face: *const u16,
        ) -> HFont;
        fn SelectObject(hdc: Hdc, h: HObj) -> HObj;
        fn DeleteObject(ho: HObj) -> i32;
        fn TextOutW(hdc: Hdc, x: i32, y: i32, lp_string: *const u16, c: i32) -> i32;
    }

    static REGISTER_ONCE: Once = Once::new();

    #[derive(Default)]
    struct QueuedWin32Events {
        closed: bool,
        resized: Option<(i32, i32)>,
        mouse_pos: Option<(i32, i32)>,
        mouse_down: Option<bool>,
        chars: Vec<char>,
        vks: Vec<i32>,
    }

    thread_local! {
        static QUEUED: RefCell<QueuedWin32Events> = RefCell::new(QueuedWin32Events::default());
    }

    fn class_name_wide() -> Vec<u16> {
        "AskaneUiWindowClass\0".encode_utf16().collect()
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: Hwnd,
        msg: u32,
        w_param: usize,
        l_param: isize,
    ) -> isize {
        match msg {
            WM_CLOSE | WM_DESTROY => {
                QUEUED.with(|q| q.borrow_mut().closed = true);
                0
            }
            WM_SIZE => {
                let w = (l_param as usize & 0xFFFF) as i32;
                let h = ((l_param as usize >> 16) & 0xFFFF) as i32;
                if w > 0 && h > 0 {
                    QUEUED.with(|q| q.borrow_mut().resized = Some((w, h)));
                }
                0
            }
            WM_MOUSEMOVE => {
                let x = (l_param as i16) as i32;
                let y = ((l_param >> 16) as i16) as i32;
                QUEUED.with(|q| q.borrow_mut().mouse_pos = Some((x, y)));
                0
            }
            WM_LBUTTONDOWN => {
                let x = (l_param as i16) as i32;
                let y = ((l_param >> 16) as i16) as i32;
                QUEUED.with(|q| {
                    let mut b = q.borrow_mut();
                    b.mouse_pos = Some((x, y));
                    b.mouse_down = Some(true);
                });
                0
            }
            WM_LBUTTONUP => {
                let x = (l_param as i16) as i32;
                let y = ((l_param >> 16) as i16) as i32;
                QUEUED.with(|q| {
                    let mut b = q.borrow_mut();
                    b.mouse_pos = Some((x, y));
                    b.mouse_down = Some(false);
                });
                0
            }
            WM_CHAR => {
                if let Some(ch) = char::from_u32(w_param as u32) {
                    QUEUED.with(|q| q.borrow_mut().chars.push(ch));
                }
                0
            }
            WM_KEYDOWN => {
                QUEUED.with(|q| q.borrow_mut().vks.push(w_param as i32));
                0
            }
            _ => unsafe { DefWindowProcW(hwnd, msg, w_param, l_param) },
        }
    }

    pub struct Win32Window {
        pub hwnd: Hwnd,
        pub dpi_scale: f64,
    }

    impl Win32Window {
        pub fn create(title: &str, width: i32, height: i32) -> Result<Self, String> {
            unsafe {
                SetProcessDPIAware();
            }
            let h_instance = unsafe { GetModuleHandleW(null()) };
            let cname = class_name_wide();
            REGISTER_ONCE.call_once(|| unsafe {
                let app_icon = LoadIconW(h_instance, 1usize as *const u16);
                let wc = WndClassExW {
                    cb_size: std::mem::size_of::<WndClassExW>() as u32,
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfn_wnd_proc: wnd_proc,
                    cb_cls_extra: 0,
                    cb_wnd_extra: 0,
                    h_instance,
                    h_icon: app_icon,
                    h_cursor: LoadCursorW(null_mut(), IDC_ARROW),
                    hbr_background: null_mut(),
                    lpsz_menu_name: null(),
                    lpsz_class_name: cname.as_ptr(),
                    h_icon_sm: app_icon,
                };
                RegisterClassExW(&wc);
            });

            let style = WS_OVERLAPPEDWINDOW | WS_VISIBLE;
            let mut rc = Rect {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            unsafe {
                AdjustWindowRect(&mut rc, style, 0);
            }
            let title_w: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    cname.as_ptr(),
                    title_w.as_ptr(),
                    style,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    rc.right - rc.left,
                    rc.bottom - rc.top,
                    null_mut(),
                    null_mut(),
                    h_instance,
                    null_mut(),
                )
            };
            if hwnd.is_null() {
                return Err("Failed to create Win32 UI window".to_string());
            }
            let dpi_scale = unsafe {
                let hdc = GetDC(hwnd);
                let dpi = if hdc.is_null() {
                    96
                } else {
                    let v = GetDeviceCaps(hdc, 88); // LOGPIXELSX
                    ReleaseDC(hwnd, hdc);
                    if v > 0 { v } else { 96 }
                };
                (dpi as f64) / 96.0
            };
            Ok(Self { hwnd, dpi_scale })
        }

        pub fn pump_events(&self, sess: &mut UiSession) {
            unsafe {
                let mut msg: Msg = std::mem::zeroed();
                while PeekMessageW(&mut msg, self.hwnd, 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            QUEUED.with(|q| {
                let mut b = q.borrow_mut();
                if b.closed {
                    sess.open = false;
                    emit_event(&mut sess.events, "close", "window", None);
                    b.closed = false;
                }
                if let Some((w, h)) = b.resized.take() {
                    sess.width = w.clamp(sess.min_width.clamp(1, 4096), 4096);
                    sess.height = h.clamp(sess.min_height.clamp(1, 4096), 4096);
                    emit_event(&mut sess.events, "resize", "window", None);
                }
                if let Some((mx, my)) = b.mouse_pos.take() {
                    sess.mouse_x = mx;
                    sess.mouse_y = my;
                }
                if let Some(md) = b.mouse_down.take() {
                    sess.mouse_down = md;
                }
                sess.typed_chars.append(&mut b.chars);
                sess.pressed_vks.append(&mut b.vks);
            });
        }

        pub fn present(&self, sess: &UiSession) -> Result<(), String> {
            let needed = (sess.width.max(0) as usize).saturating_mul(sess.height.max(0) as usize);
            if sess.width <= 0 || sess.height <= 0 || sess.buffer.len() < needed {
                return Ok(());
            }
            unsafe {
                let hdc = GetDC(self.hwnd);
                if hdc.is_null() {
                    return Ok(());
                }
                let bmi = BitmapInfo {
                    bmi_header: BitmapInfoHeader {
                        bi_size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                        bi_width: sess.width,
                        bi_height: -sess.height,
                        bi_planes: 1,
                        bi_bit_count: 32,
                        bi_compression: BI_RGB,
                        bi_size_image: 0,
                        bi_x_pels_per_meter: 0,
                        bi_y_pels_per_meter: 0,
                        bi_clr_used: 0,
                        bi_clr_important: 0,
                    },
                    bmi_colors: [0],
                };
                StretchDIBits(
                    hdc,
                    0,
                    0,
                    sess.width,
                    sess.height,
                    0,
                    0,
                    sess.width,
                    sess.height,
                    sess.buffer.as_ptr() as *const c_void,
                    &bmi,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
                SetBkMode(hdc, TRANSPARENT);
                let face: Vec<u16> = "Segoe UI\0".encode_utf16().collect();
                let font = CreateFontW(
                    -(sess.theme.font_size),
                    0,
                    0,
                    0,
                    400,
                    0,
                    0,
                    0,
                    1,
                    0,
                    0,
                    5,
                    0,
                    face.as_ptr(),
                );
                let old_font = if !font.is_null() {
                    SelectObject(hdc, font)
                } else {
                    null_mut()
                };
                for t in &sess.texts {
                    let r = (t.color >> 16) & 0xFF;
                    let g = (t.color >> 8) & 0xFF;
                    let b = t.color & 0xFF;
                    SetTextColor(hdc, r | (g << 8) | (b << 16));
                    let wide: Vec<u16> = t.text.encode_utf16().collect();
                    TextOutW(hdc, t.x, t.y, wide.as_ptr(), wide.len() as i32);
                }
                if !old_font.is_null() {
                    SelectObject(hdc, old_font);
                }
                if !font.is_null() {
                    DeleteObject(font);
                }
                ReleaseDC(self.hwnd, hdc);
            }
            Ok(())
        }

        pub fn destroy(self) {
            unsafe {
                DestroyWindow(self.hwnd);
            }
        }
    }
}
