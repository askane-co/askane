use std::cell::RefCell;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::Value;
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "regex",
        &[
            ("is_match", rx_is_match),
            ("find", rx_find),
            ("find_all", rx_find_all),
            ("replace", rx_replace),
        ],
        &[],
    )
}

#[derive(Clone)]
enum Atom {
    Any,
    Literal(char),
    Digit,
    Word,
    Space,
    Class {
        negated: bool,
        chars: Vec<(char, char)>,
    },
}

#[derive(Clone)]
enum Quant {
    One,
    ZeroOrOne,
    ZeroOrMore,
    OneOrMore,
}

#[derive(Clone)]
struct Token {
    atom: Atom,
    quant: Quant,
}

struct Compiled {
    anchored_start: bool,
    anchored_end: bool,
    tokens: Vec<Token>,
}

fn compile_pattern(pattern: &str) -> Result<Compiled, String> {
    if pattern.len() > 512 {
        return Err("regex pattern exceeds 512 byte limit".to_string());
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    let anchored_start = chars.first() == Some(&'^');
    if anchored_start {
        i += 1;
    }
    let mut anchored_end = false;
    let mut tokens = Vec::new();

    while i < chars.len() {
        if chars[i] == '$' && i + 1 == chars.len() {
            anchored_end = true;
            break;
        }
        let atom = match chars[i] {
            '.' => {
                i += 1;
                Atom::Any
            }
            '\\' => {
                i += 1;
                let esc = *chars
                    .get(i)
                    .ok_or_else(|| "regex: trailing backslash".to_string())?;
                i += 1;
                match esc {
                    'd' => Atom::Digit,
                    'w' => Atom::Word,
                    's' => Atom::Space,
                    other => Atom::Literal(other),
                }
            }
            '[' => {
                i += 1;
                let negated = chars.get(i) == Some(&'^');
                if negated {
                    i += 1;
                }
                let mut ranges = Vec::new();
                while i < chars.len() && chars[i] != ']' {
                    let start = chars[i];
                    i += 1;
                    if i + 1 < chars.len() && chars[i] == '-' && chars[i + 1] != ']' {
                        let end = chars[i + 1];
                        i += 2;
                        ranges.push((start, end));
                    } else {
                        ranges.push((start, start));
                    }
                }
                if i >= chars.len() || chars[i] != ']' {
                    return Err("regex: unclosed character class".to_string());
                }
                i += 1;
                Atom::Class {
                    negated,
                    chars: ranges,
                }
            }
            c => {
                i += 1;
                Atom::Literal(c)
            }
        };

        let quant = match chars.get(i) {
            Some('*') => {
                i += 1;
                Quant::ZeroOrMore
            }
            Some('+') => {
                i += 1;
                Quant::OneOrMore
            }
            Some('?') => {
                i += 1;
                Quant::ZeroOrOne
            }
            _ => Quant::One,
        };
        tokens.push(Token { atom, quant });
    }

    Ok(Compiled {
        anchored_start,
        anchored_end,
        tokens,
    })
}

fn atom_matches(atom: &Atom, ch: char) -> bool {
    match atom {
        Atom::Any => ch != '\n',
        Atom::Literal(c) => *c == ch,
        Atom::Digit => ch.is_ascii_digit(),
        Atom::Word => ch.is_alphanumeric() || ch == '_',
        Atom::Space => ch.is_whitespace(),
        Atom::Class { negated, chars } => {
            let hit = chars.iter().any(|&(a, b)| ch >= a && ch <= b);
            if *negated { !hit } else { hit }
        }
    }
}

fn match_at(
    comp: &Compiled,
    ti: usize,
    text: &[char],
    si: usize,
    steps: &mut usize,
) -> Result<Option<usize>, String> {
    *steps += 1;
    if *steps > 100_000 {
        return Err("regex step limit exceeded (possible ReDoS pattern)".to_string());
    }
    if ti == comp.tokens.len() {
        if comp.anchored_end && si != text.len() {
            return Ok(None);
        }
        return Ok(Some(si));
    }

    let tok = &comp.tokens[ti];
    match tok.quant {
        Quant::One => {
            if si < text.len() && atom_matches(&tok.atom, text[si]) {
                match_at(comp, ti + 1, text, si + 1, steps)
            } else {
                Ok(None)
            }
        }
        Quant::ZeroOrOne => {
            if si < text.len() && atom_matches(&tok.atom, text[si]) {
                if let Some(end) = match_at(comp, ti + 1, text, si + 1, steps)? {
                    return Ok(Some(end));
                }
            }
            match_at(comp, ti + 1, text, si, steps)
        }
        Quant::ZeroOrMore | Quant::OneOrMore => {
            let min_rep = if matches!(tok.quant, Quant::OneOrMore) {
                1
            } else {
                0
            };
            let mut max_si = si;
            while max_si < text.len() && atom_matches(&tok.atom, text[max_si]) {
                max_si += 1;
            }
            if max_si - si < min_rep {
                return Ok(None);
            }
            for try_si in (si + min_rep..=max_si).rev() {
                if let Some(end) = match_at(comp, ti + 1, text, try_si, steps)? {
                    return Ok(Some(end));
                }
            }
            Ok(None)
        }
    }
}

fn find_first_char_range(
    comp: &Compiled,
    chars: &[char],
    start_from: usize,
) -> Result<Option<(usize, usize)>, String> {
    if chars.len() > 1_048_576 {
        return Err("regex input exceeds 1M characters limit".to_string());
    }
    let mut steps = 0usize;
    let limit = if comp.anchored_start {
        if start_from == 0 { 1 } else { 0 }
    } else {
        chars.len() + 1
    };
    for s in start_from..limit {
        if let Some(end) = match_at(comp, 0, chars, s, &mut steps)? {
            return Ok(Some((s, end)));
        }
    }
    Ok(None)
}

fn expect_str<'a>(v: &'a Value, fn_name: &str) -> Result<&'a str, String> {
    match v {
        Value::Str(s) => Ok(s.as_str()),
        other => Err(format!(
            "regex.{}() expects a string, got '{}'",
            fn_name,
            other.type_name()
        )),
    }
}

fn rx_is_match(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("regex.is_match() expects 2 strings (pattern, text)".to_string());
    }
    let comp = compile_pattern(expect_str(&args[0], "is_match")?)?;
    let chars: Vec<char> = expect_str(&args[1], "is_match")?.chars().collect();
    Ok(Value::Bool(
        find_first_char_range(&comp, &chars, 0)?.is_some(),
    ))
}

fn rx_find(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("regex.find() expects 2 strings (pattern, text)".to_string());
    }
    let comp = compile_pattern(expect_str(&args[0], "find")?)?;
    let chars: Vec<char> = expect_str(&args[1], "find")?.chars().collect();
    match find_first_char_range(&comp, &chars, 0)? {
        Some((s, e)) => Ok(Value::Str(chars[s..e].iter().collect())),
        None => Ok(Value::Unit),
    }
}

fn rx_find_all(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("regex.find_all() expects 2 strings (pattern, text)".to_string());
    }
    let comp = compile_pattern(expect_str(&args[0], "find_all")?)?;
    let chars: Vec<char> = expect_str(&args[1], "find_all")?.chars().collect();
    let mut out = Vec::new();
    let mut pos = 0;
    while pos <= chars.len() && out.len() < 10_000 {
        match find_first_char_range(&comp, &chars, pos)? {
            Some((s, e)) => {
                out.push(Value::Str(chars[s..e].iter().collect()));
                pos = if e > s { e } else { s + 1 };
            }
            None => break,
        }
    }
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn rx_replace(_vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 3 {
        return Err("regex.replace() expects 3 strings (pattern, text, replacement)".to_string());
    }
    let comp = compile_pattern(expect_str(&args[0], "replace")?)?;
    let chars: Vec<char> = expect_str(&args[1], "replace")?.chars().collect();
    let repl = expect_str(&args[2], "replace")?;
    let mut out = String::new();
    let mut pos = 0;
    let mut count = 0;
    while pos <= chars.len() && count < 10_000 {
        match find_first_char_range(&comp, &chars, pos)? {
            Some((s, e)) => {
                for &c in &chars[pos..s] {
                    out.push(c);
                }
                out.push_str(repl);
                if e == s {
                    if s < chars.len() {
                        out.push(chars[s]);
                    }
                    pos = s + 1;
                } else {
                    pos = e;
                }
                count += 1;
            }
            None => break,
        }
    }
    if pos < chars.len() {
        for &c in &chars[pos..] {
            out.push(c);
        }
    }
    Ok(Value::Str(out))
}
