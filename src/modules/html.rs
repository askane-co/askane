use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::{DictMap, Value, new_dict_with_capacity};
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
    Ok(Value::from_str(escaped))
}

fn html_unescape(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("unescape() expects 1 string argument".to_string());
    }
    let decoded = unescape_str(&args[0].to_string());
    vm.charge_alloc(decoded.len())?;
    Ok(Value::from_str(decoded))
}

#[derive(Clone)]
pub(crate) struct DomNode {
    pub tag: String,
    pub attrs: HashMap<String, String>,
    pub text: String,
    pub children: Vec<DomNode>,
}

impl DomNode {
    fn to_value(&self) -> Value {
        let mut attrs_map = new_dict_with_capacity(self.attrs.len());
        for (k, v) in &self.attrs {
            attrs_map.insert(k.clone(), Value::from_str(v.clone()));
        }
        let child_vals: Vec<Value> = self.children.iter().map(DomNode::to_value).collect();
        let mut map = new_dict_with_capacity(4);
        map.insert("tag".to_string(), Value::from_str(self.tag.clone()));
        map.insert("attrs".to_string(), Value::dict(attrs_map));
        map.insert("text".to_string(), Value::from_str(self.text.clone()));
        map.insert(
            "children".to_string(),
            Value::List(Rc::new(RefCell::new(child_vals))),
        );
        Value::dict(map)
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

fn parse_dom(vm: &mut VM, src: &str) -> Result<DomNode, String> {
    parse_dom_mode(vm, src, false, 200_000)
}

pub(crate) fn parse_xml(vm: &mut VM, src: &str) -> Result<DomNode, String> {
    parse_dom_mode(vm, src, true, 100_000)
}

fn parse_dom_mode(
    vm: &mut VM,
    src: &str,
    strict_xml: bool,
    max_nodes: usize,
) -> Result<DomNode, String> {
    let mut stack = vec![DomNode {
        tag: "#document".to_string(),
        attrs: HashMap::new(),
        text: String::new(),
        children: Vec::new(),
    }];
    let bytes = src.as_bytes();
    let mut i = 0usize;
    let mut nodes_created = 0usize;
    let mut est_alloc = 0usize;
    let mut steps = 0usize;

    while i < bytes.len() {
        steps += 1;
        if (steps & 1023) == 0 {
            vm.check_timeout()?;
        }
        if bytes[i] == b'<' {
            if src[i..].starts_with("<!--") {
                if let Some(end) = src[i + 4..].find("-->") {
                    i += 4 + end + 3;
                    continue;
                }
                break;
            }
            if src[i..].starts_with("<![CDATA[") {
                let cstart = i + 9;
                if let Some(cend) = src[cstart..].find("]]>") {
                    if cend > 16 * 1024 * 1024 {
                        return Err("E_ASSET_XML: CDATA section exceeds 16 MiB limit".to_string());
                    }
                    let cur = stack.last_mut().unwrap();
                    cur.text.push_str(&src[cstart..cstart + cend]);
                    i = cstart + cend + 3;
                    continue;
                }
                return Err("E_ASSET_XML: unterminated CDATA section".to_string());
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
                        if nodes_created > max_nodes {
                            if strict_xml {
                                return Err(format!(
                                    "E_ASSET_XML: XML node limit ({}) exceeded",
                                    max_nodes
                                ));
                            }
                            break;
                        }
                        if strict_xml && stack.len() > 128 {
                            return Err("E_ASSET_XML: XML nesting depth (128) exceeded".to_string());
                        }
                        let attr_bytes: usize = attrs
                            .iter()
                            .map(|(k, v)| k.len().saturating_add(v.len()).saturating_add(48))
                            .sum();
                        let delta = 256usize
                            .saturating_add(tag.len())
                            .saturating_add(attr_bytes);
                        est_alloc = est_alloc.saturating_add(delta);
                        if est_alloc >= 4096 {
                            vm.charge_alloc(est_alloc)?;
                            est_alloc = 0;
                        }
                        let node = DomNode {
                            tag: tag.clone(),
                            attrs,
                            text: String::new(),
                            children: Vec::new(),
                        };
                        let void_html = !strict_xml && is_void_tag(&tag);
                        if self_close || void_html || stack.len() > 256 {
                            stack.last_mut().unwrap().children.push(node);
                        } else {
                            stack.push(node);
                        }
                    }
                }
                continue;
            }
            let tail_txt = unescape_str(&src[i..]);
            if !tail_txt.trim().is_empty() {
                let cur = stack.last_mut().unwrap();
                if !cur.text.is_empty() && !cur.text.ends_with(' ') && !tail_txt.starts_with(' ') {
                    cur.text.push(' ');
                }
                cur.text.push_str(tail_txt.trim());
            }
            break;
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
    if est_alloc > 0 {
        vm.charge_alloc(est_alloc)?;
    }

    while stack.len() > 1 {
        let finished = stack.pop().unwrap();
        stack.last_mut().unwrap().children.push(finished);
    }
    Ok(stack.pop().unwrap())
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
    vm.charge_alloc(src.len())?;
    let dom = parse_dom(vm, src)?;
    Ok(dom.to_value())
}

fn node_matches_simple(node: &DictMap, part: &str) -> bool {
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
    vm: &mut VM,
    node_val: &Value,
    parts: &[&str],
    stage: usize,
    out: &mut Vec<Value>,
    depth: usize,
    visited: &mut std::collections::HashSet<(usize, usize)>,
    steps: &mut usize,
) -> Result<(), String> {
    if depth > 256 || out.len() >= 20_000 || *steps >= 500_000 {
        return Ok(());
    }
    *steps += 1;
    if (*steps & 1023) == 0 {
        vm.check_timeout()?;
    }
    let Value::Dict(d_rc) = node_val else {
        return Ok(());
    };
    let ptr = Rc::as_ptr(d_rc) as usize;
    if !visited.insert((ptr, stage)) {
        return Ok(());
    }
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
            if out.len() >= 20_000 || *steps >= 500_000 {
                break;
            }
            collect_matching(
                vm,
                &child,
                parts,
                next_stage,
                out,
                depth + 1,
                visited,
                steps,
            )?;
        }
    }
    Ok(())
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
    let mut visited = std::collections::HashSet::new();
    let mut steps = 0usize;
    collect_matching(vm, &root, &parts, 0, &mut out, 0, &mut visited, &mut steps)?;
    vm.charge_alloc(out.len().saturating_mul(std::mem::size_of::<Value>()))?;
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn extract_node_text(
    vm: &mut VM,
    v: &Value,
    out: &mut String,
    depth: usize,
    steps: &mut usize,
) -> Result<(), String> {
    const MAX_TEXT_BYTES: usize = 16 * 1024 * 1024;
    if depth > 256 || out.len() >= MAX_TEXT_BYTES || *steps >= 500_000 {
        return Ok(());
    }
    *steps += 1;
    if (*steps & 1023) == 0 {
        vm.check_timeout()?;
    }
    let Value::Dict(d_rc) = v else {
        return Ok(());
    };
    let d = d_rc.borrow();
    if let Some(Value::Str(t)) = d.get("text") {
        if !t.is_empty() {
            let need = t.len().saturating_add(1);
            if out.len().saturating_add(need) > MAX_TEXT_BYTES {
                return Err("text(): extracted text exceeds 16 MiB limit".to_string());
            }
            vm.charge_alloc(need)?;
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
            if out.len() >= MAX_TEXT_BYTES || *steps >= 500_000 {
                break;
            }
            extract_node_text(vm, &child, out, depth + 1, steps)?;
        }
    }
    Ok(())
}

pub(crate) fn html_text(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("text() expects 1 node argument".to_string());
    }
    let mut out = String::new();
    let mut steps = 0usize;
    extract_node_text(vm, &args[0], &mut out, 0, &mut steps)?;
    Ok(Value::from_str(out))
}
