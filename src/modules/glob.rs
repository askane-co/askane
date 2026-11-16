use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module("glob", &[("match", glob_match_fn)], &[])
}

pub fn glob_matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.replace('\\', "/").chars().collect();
    let t: Vec<char> = text.replace('\\', "/").chars().collect();
    match_impl(&p, &t, 0)
}

fn match_impl(p: &[char], t: &[char], depth: usize) -> bool {
    if depth > 64 {
        return false;
    }
    if p.is_empty() {
        return t.is_empty();
    }
    if p[0] == '*' {
        let is_double = p.len() > 1 && p[1] == '*';
        let rest_p = if is_double {
            if p.len() > 2 && p[2] == '/' {
                &p[3..]
            } else {
                &p[2..]
            }
        } else {
            &p[1..]
        };
        if match_impl(rest_p, t, depth + 1) {
            return true;
        }
        if !t.is_empty() && (is_double || t[0] != '/') {
            return match_impl(p, &t[1..], depth + 1);
        }
        return false;
    }
    if t.is_empty() {
        return false;
    }
    if p[0] == '?' || p[0] == t[0] {
        return match_impl(&p[1..], &t[1..], depth + 1);
    }
    false
}

fn glob_match_fn(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("glob.match() expects 2 string arguments (pattern, text)".to_string());
    }
    let pat = match &args[0] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "glob.match() pattern must be a string, got '{}'",
                other.type_name()
            ));
        }
    };
    let text = match &args[1] {
        Value::Str(s) => s.as_str(),
        other => {
            return Err(format!(
                "glob.match() text must be a string, got '{}'",
                other.type_name()
            ));
        }
    };
    Ok(Value::Bool(glob_matches(pat, text)))
}
