use crate::modules::{build_module, html};
use crate::value::Value;

pub fn module() -> Value {
    build_module(
        "xml",
        &[
            ("parse", html::html_parse),
            ("select", html::html_select),
            ("text", html::html_text),
            ("escape", xml_escape),
            ("unescape", xml_unescape),
        ],
        &[],
    )
}

fn xml_escape(vm: &mut crate::vm::VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("xml.escape() expects 1 argument".to_string());
    }
    let s = html::escape_str(&args[0].to_string());
    vm.charge_alloc(s.len())?;
    Ok(Value::from_str(s))
}

fn xml_unescape(vm: &mut crate::vm::VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 1 {
        return Err("xml.unescape() expects 1 argument".to_string());
    }
    let s = html::unescape_str(&args[0].to_string());
    vm.charge_alloc(s.len())?;
    Ok(Value::from_str(s))
}
