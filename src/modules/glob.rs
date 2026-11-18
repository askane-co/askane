use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module("glob", &[("match", glob_match_fn)], &[])
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GlobTok {
    Literal(char),
    AnyChar,
    Star,
    DoubleStar,
    DoubleStarDir,
}

pub fn glob_matches(pattern: &str, text: &str) -> bool {
    if pattern.len() > 1024 || text.len() > 65536 {
        return false;
    }
    let p: Vec<char> = pattern.replace('\\', "/").chars().collect();
    let t: Vec<char> = text.replace('\\', "/").chars().collect();

    let mut toks = Vec::with_capacity(p.len());
    let mut idx = 0;
    while idx < p.len() {
        if p[idx] == '*' {
            let is_double = idx + 1 < p.len() && p[idx + 1] == '*';
            if is_double {
                while idx < p.len() && p[idx] == '*' {
                    idx += 1;
                }
                if idx < p.len() && p[idx] == '/' {
                    idx += 1;
                    if !matches!(toks.last(), Some(GlobTok::DoubleStarDir)) {
                        toks.push(GlobTok::DoubleStarDir);
                    }
                } else if !matches!(toks.last(), Some(GlobTok::DoubleStar)) {
                    toks.push(GlobTok::DoubleStar);
                }
            } else {
                idx += 1;
                if !matches!(toks.last(), Some(GlobTok::Star | GlobTok::DoubleStar)) {
                    toks.push(GlobTok::Star);
                }
            }
        } else if p[idx] == '?' {
            toks.push(GlobTok::AnyChar);
            idx += 1;
        } else {
            toks.push(GlobTok::Literal(p[idx]));
            idx += 1;
        }
    }

    let n = toks.len();
    let mut prev = vec![false; n + 1];
    let mut cur = vec![false; n + 1];
    let mut mid_prev = vec![false; n + 1];
    let mut mid_cur = vec![false; n + 1];
    prev[0] = true;
    for j in 1..=n {
        prev[j] = prev[j - 1]
            && matches!(
                toks[j - 1],
                GlobTok::Star | GlobTok::DoubleStar | GlobTok::DoubleStarDir
            );
    }

    for &ch in &t {
        cur[0] = false;
        mid_cur[0] = false;
        for j in 1..=n {
            mid_cur[j] = false;
            cur[j] = match toks[j - 1] {
                GlobTok::Literal(c) => prev[j - 1] && ch == c,
                GlobTok::AnyChar => prev[j - 1],
                GlobTok::Star => cur[j - 1] || (ch != '/' && prev[j]),
                GlobTok::DoubleStar => cur[j - 1] || prev[j],
                GlobTok::DoubleStarDir => {
                    let entered = prev[j - 1] || prev[j] || mid_prev[j];
                    mid_cur[j] = entered;
                    cur[j - 1] || (ch == '/' && entered)
                }
            };
        }
        std::mem::swap(&mut prev, &mut cur);
        std::mem::swap(&mut mid_prev, &mut mid_cur);
    }

    prev[n]
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
