use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "html",
        &[
            ("escape", html_escape),
            ("unescape", html_unescape),
            ("parse", html_parse),
            ("select", html_select),
            ("text", html_text),
        ],
        &[],
    )
}

pub(crate) fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

pub(crate) fn unescape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '&' {
            if let Some(semi) = chars[i + 1..chars.len().min(i + 12)]
                .iter()
                .position(|&c| c == ';')
            {
                let ent: String = chars[i + 1..i + 1 + semi].iter().collect();
                let decoded = match ent.as_str() {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" | "#39" => Some('\''),
                    "nbsp" => Some(' '),
                    _ if ent.starts_with('#') => {
                        let code = if let Some(h) = ent[1..].strip_prefix(['x', 'X']) {
                            u32::from_str_radix(h, 16).ok()
                        } else {
                            ent[1..].parse::<u32>().ok()
                        };
                        code.and_then(char::from_u32)
                    }
                    _ => None,
                };
                if let Some(ch) = decoded {
                    out.push(ch);
                    i = i + 2 + semi;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn html_escape(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("escape() expects 1 string argument".to_string());
    }
    let escaped = escape_str(&args[0].to_string());
    vm.charge_alloc(escaped.len())?;
    Ok(Value::Str(escaped))
}

fn html_unescape(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("unescape() expects 1 string argument".to_string());
    }
    let decoded = unescape_str(&args[0].to_string());
    vm.charge_alloc(decoded.len())?;
    Ok(Value::Str(decoded))
}

#[derive(Clone)]
struct DomNode {
    tag: String,
    attrs: HashMap<String, String>,
    text: String,
    children: Vec<DomNode>,
}

impl DomNode {
    fn to_value(&self) -> Value {
        let mut attrs_map = HashMap::with_capacity(self.attrs.len());
        for (k, v) in &self.attrs {
            attrs_map.insert(k.clone(), Value::Str(v.clone()));
        }
        let child_vals: Vec<Value> = self.children.iter().map(DomNode::to_value).collect();
        let mut map = HashMap::new();
        map.insert("tag".to_string(), Value::Str(self.tag.clone()));
        map.insert(
            "attrs".to_string(),
            Value::Dict(Rc::new(RefCell::new(attrs_map))),
        );
        map.insert("text".to_string(), Value::Str(self.text.clone()));
        map.insert(
            "children".to_string(),
            Value::List(Rc::new(RefCell::new(child_vals))),
        );
        Value::Dict(Rc::new(RefCell::new(map)))
    }
}

fn is_void_tag(tag: &str) -> bool {
    matches!(
        tag.to_ascii_lowercase().as_str(),
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn parse_tag_attrs(raw: &str) -> (String, HashMap<String, String>, bool) {
    let trimmed = raw.trim();
    let self_close = trimmed.ends_with('/');
    let clean = trimmed.trim_end_matches('/').trim();
    let bytes = clean.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    let tag = clean[..i].to_string();
    let mut attrs = HashMap::new();
    let chars: Vec<char> = clean[i..].chars().collect();
    let mut p = 0usize;
    while p < chars.len() {
        while p < chars.len() && chars[p].is_whitespace() {
            p += 1;
        }
        if p >= chars.len() {
            break;
        }
        let k_start = p;
        while p < chars.len() && !chars[p].is_whitespace() && chars[p] != '=' {
            p += 1;
        }
        let key: String = chars[k_start..p].iter().collect();
        if key.is_empty() {
            p += 1;
            continue;
        }
        while p < chars.len() && chars[p].is_whitespace() {
            p += 1;
        }
        let mut val = String::new();
        if p < chars.len() && chars[p] == '=' {
            p += 1;
            while p < chars.len() && chars[p].is_whitespace() {
                p += 1;
            }
            if p < chars.len() && (chars[p] == '"' || chars[p] == '\'') {
                let q = chars[p];
                p += 1;
                let v_start = p;
                while p < chars.len() && chars[p] != q {
                    p += 1;
                }
                val = unescape_str(&chars[v_start..p].iter().collect::<String>());
                if p < chars.len() {
                    p += 1;
                }
            } else {
                let v_start = p;
                while p < chars.len() && !chars[p].is_whitespace() {
                    p += 1;
                }
                val = unescape_str(&chars[v_start..p].iter().collect::<String>());
            }
        }
        attrs.insert(key, val);
    }
    (tag, attrs, self_close)
}

fn parse_dom(src: &str) -> DomNode {
    let mut stack = vec![DomNode {
        tag: "#document".to_string(),
        attrs: HashMap::new(),
        text: String::new(),
        children: Vec::new(),
    }];
    let bytes = src.as_bytes();
    let mut i = 0usize;
    let mut nodes_created = 0usize;

    while i < bytes.len() && nodes_created < 200_000 {
        if bytes[i] == b'<' {
            if src[i..].starts_with("<!--") {
                if let Some(end) = src[i + 4..].find("-->") {
                    i += 4 + end + 3;
                    continue;
                }
                break;
            }
            if src[i..].starts_with("<!") || src[i..].starts_with("<?") {
                if let Some(end) = src[i + 2..].find('>') {
                    i += 2 + end + 1;
                    continue;
                }
                break;
            }
            if let Some(gt) = src[i + 1..].find('>') {
                let inner = &src[i + 1..i + 1 + gt];
                i += gt + 2;
                let trimmed = inner.trim();
                if let Some(close_name) = trimmed.strip_prefix('/') {
                    let cname = close_name.trim();
                    if let Some(pos) = stack
                        .iter()
                        .rposition(|n| n.tag.eq_ignore_ascii_case(cname))
                    {
                        if pos > 0 {
                            while stack.len() > pos {
                                let finished = stack.pop().unwrap();
                                stack.last_mut().unwrap().children.push(finished);
                            }
                        }
                    }
                } else {
                    let (tag, attrs, self_close) = parse_tag_attrs(trimmed);
                    if !tag.is_empty() {
                        nodes_created += 1;
                        let node = DomNode {
                            tag: tag.clone(),
                            attrs,
                            text: String::new(),
                            children: Vec::new(),
                        };
                        if self_close || is_void_tag(&tag) || stack.len() > 256 {
                            stack.last_mut().unwrap().children.push(node);
                        } else {
                            stack.push(node);
                        }
                    }
                }
                continue;
            }
        }
        let next_lt = src[i..].find('<').map(|p| i + p).unwrap_or(src.len());
        let txt = unescape_str(&src[i..next_lt]);
        if !txt.trim().is_empty() {
            let cur = stack.last_mut().unwrap();
            if !cur.text.is_empty() && !cur.text.ends_with(' ') && !txt.starts_with(' ') {
                cur.text.push(' ');
            }
            cur.text.push_str(txt.trim());
        }
        i = next_lt;
    }

    while stack.len() > 1 {
        let finished = stack.pop().unwrap();
        stack.last_mut().unwrap().children.push(finished);
    }
    stack.pop().unwrap()
}

pub(crate) fn html_parse(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("parse() expects 1 string argument".to_string());
    }
    let Value::Str(src) = &args[0] else {
        return Err("parse() expects a string".to_string());
    };
    if src.len() > 16 * 1024 * 1024 {
        return Err("parse(): document exceeds 16 MiB limit".to_string());
    }
    let dom = parse_dom(src);
    vm.charge_alloc(src.len().saturating_mul(2))?;
    Ok(dom.to_value())
}

fn node_matches_simple(node: &HashMap<String, Value>, part: &str) -> bool {
    let mut tag_req: Option<&str> = None;
    let mut id_req: Option<&str> = None;
    let mut class_reqs: Vec<&str> = Vec::new();

    let mut rest = part;
    if !rest.starts_with('.') && !rest.starts_with('#') {
        let end = rest.find(['.', '#']).unwrap_or(rest.len());
        tag_req = Some(&rest[..end]);
        rest = &rest[end..];
    }
    while !rest.is_empty() {
        let sigil = rest.as_bytes()[0];
        let body = &rest[1..];
        let end = body.find(['.', '#']).unwrap_or(body.len());
        let token = &body[..end];
        if sigil == b'#' {
            id_req = Some(token);
        } else if sigil == b'.' {
            class_reqs.push(token);
        }
        rest = &body[end..];
    }

    if let Some(t) = tag_req {
        if t != "*" {
            let Some(Value::Str(node_tag)) = node.get("tag") else {
                return false;
            };
            if !node_tag.eq_ignore_ascii_case(t) {
                return false;
            }
        }
    }
    let attrs_opt = node.get("attrs").and_then(|v| match v {
        Value::Dict(d) => Some(d),
        _ => None,
    });
    if let Some(id) = id_req {
        let ok =
            attrs_opt.is_some_and(|d| d.borrow().get("id").is_some_and(|v| v.to_string() == id));
        if !ok {
            return false;
        }
    }
    for cls in class_reqs {
        let ok = attrs_opt.is_some_and(|d| {
            d.borrow()
                .get("class")
                .is_some_and(|v| v.to_string().split_whitespace().any(|c| c == cls))
        });
        if !ok {
            return false;
        }
    }
    true
}

fn collect_matching(
    node_val: &Value,
    parts: &[&str],
    stage: usize,
    out: &mut Vec<Value>,
    depth: usize,
) {
    if depth > 256 || out.len() > 20_000 {
        return;
    }
    let Value::Dict(d_rc) = node_val else {
        return;
    };
    let d = d_rc.borrow();
    let mut next_stage = stage;
    if node_matches_simple(&d, parts[stage]) {
        if stage + 1 == parts.len() {
            out.push(node_val.clone());
        } else {
            next_stage += 1;
        }
    }
    if let Some(Value::List(ch_rc)) = d.get("children") {
        let children = ch_rc.borrow().clone();
        drop(d);
        for child in children {
            collect_matching(&child, parts, next_stage, out, depth + 1);
        }
    }
}

pub(crate) fn html_select(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("select() expects (node_or_str, selector)".to_string());
    }
    let root = match &args[0] {
        Value::Str(_) => html_parse(vm, &args[..1])?,
        Value::Dict(_) => args[0].clone(),
        other => {
            return Err(format!(
                "select() expects parsed node or string, got '{}'",
                other.type_name()
            ));
        }
    };
    let Value::Str(sel) = &args[1] else {
        return Err("select() selector must be string".to_string());
    };
    let parts: Vec<&str> = sel.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(Value::List(Rc::new(RefCell::new(Vec::new()))));
    }
    let mut out = Vec::new();
    collect_matching(&root, &parts, 0, &mut out, 0);
    vm.charge_alloc(out.len().saturating_mul(std::mem::size_of::<Value>()))?;
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn extract_node_text(v: &Value, out: &mut String, depth: usize) {
    if depth > 256 {
        return;
    }
    let Value::Dict(d_rc) = v else {
        return;
    };
    let d = d_rc.borrow();
    if let Some(Value::Str(t)) = d.get("text") {
        if !t.is_empty() {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            out.push_str(t);
        }
    }
    if let Some(Value::List(ch)) = d.get("children") {
        let children = ch.borrow().clone();
        drop(d);
        for child in children {
            extract_node_text(&child, out, depth + 1);
        }
    }
}

pub(crate) fn html_text(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("text() expects 1 node argument".to_string());
    }
    let mut out = String::new();
    extract_node_text(&args[0], &mut out, 0);
    vm.charge_alloc(out.len())?;
    Ok(Value::Str(out))
}
