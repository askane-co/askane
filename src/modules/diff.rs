use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use crate::modules::build_module;
use crate::value::{Value, new_dict_with_capacity};
use crate::vm::VM;

pub fn module() -> Value {
    build_module(
        "diff",
        &[
            ("lines", diff_lines),
            ("format", diff_format),
            ("values", diff_values),
        ],
        &[],
    )
}

pub(crate) fn compute_line_diff_raw<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(char, &'a str)> {
    let n = a.len().min(2000);
    let m = b.len().min(2000);
    let a = &a[..n];
    let b = &b[..m];
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut out = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push(('-', a[i]));
            i += 1;
        } else {
            out.push(('+', b[j]));
            j += 1;
        }
    }
    while i < n {
        out.push(('-', a[i]));
        i += 1;
    }
    while j < m {
        out.push(('+', b[j]));
        j += 1;
    }
    out
}

pub(crate) fn lcs_line_diff<'a>(
    vm: &mut VM,
    a: &[&'a str],
    b: &[&'a str],
) -> Result<Vec<(char, &'a str)>, String> {
    let n = a.len().min(2000);
    let m = b.len().min(2000);
    let dp_bytes = (n + 1).saturating_mul(m + 1).saturating_mul(4);
    vm.charge_alloc(dp_bytes)?;
    for _ in 0..n {
        vm.charge_work(((m / 32) as u64).max(1))?;
    }
    Ok(compute_line_diff_raw(a, b))
}

fn diff_lines(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("diff.lines() expects 2 string arguments".to_string());
    }
    let a = args[0].to_string();
    let b = args[1].to_string();
    let al: Vec<&str> = a.lines().collect();
    let bl: Vec<&str> = b.lines().collect();
    let ops = lcs_line_diff(vm, &al, &bl)?;
    let total_bytes: usize = ops
        .iter()
        .map(|(_, line)| {
            line.len()
                .saturating_add(2)
                .saturating_add(std::mem::size_of::<Value>())
        })
        .sum();
    vm.charge_alloc(total_bytes)?;
    let out: Vec<Value> = ops
        .into_iter()
        .map(|(tag, line)| Value::from_str(format!("{} {}", tag, line)))
        .collect();
    Ok(Value::List(Rc::new(RefCell::new(out))))
}

fn diff_format(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("diff.format() expects 2 arguments".to_string());
    }
    let a = args[0].to_string();
    let b = args[1].to_string();
    let al: Vec<&str> = a.lines().collect();
    let bl: Vec<&str> = b.lines().collect();
    let ops = lcs_line_diff(vm, &al, &bl)?;
    let total_bytes: usize = ops
        .iter()
        .map(|(_, line)| line.len().saturating_add(3))
        .sum();
    if total_bytes > 32 * 1024 * 1024 {
        return Err("diff.format() output exceeds 32 MiB limit".to_string());
    }
    vm.charge_alloc(total_bytes)?;
    let mut out = String::with_capacity(total_bytes);
    for (i, (tag, line)) in ops.into_iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push(tag);
        out.push(' ');
        out.push_str(line);
    }
    Ok(Value::from_str(out))
}

fn record_entry(out: &mut Vec<Value>, path: &str, kind: &str, left: Value, right: Value) {
    let mut map = new_dict_with_capacity(4);
    map.insert("path".to_string(), Value::from_str(path.to_string()));
    map.insert("kind".to_string(), Value::from_str(kind.to_string()));
    map.insert("left".to_string(), left);
    map.insert("right".to_string(), right);
    out.push(Value::dict(map));
}

fn collect_val_diffs(
    vm: &mut VM,
    path: &str,
    left: &Value,
    right: &Value,
    out: &mut Vec<Value>,
    depth: usize,
    seen: &mut HashSet<(usize, usize)>,
    steps: &mut usize,
) -> Result<(), String> {
    if depth > 64 || out.len() >= 2000 || *steps >= 200_000 {
        return Ok(());
    }
    *steps += 1;
    if (*steps & 1023) == 0 {
        vm.check_timeout()?;
    }
    match (left, right) {
        (Value::List(a), Value::List(b)) => {
            if Rc::ptr_eq(a, b) {
                return Ok(());
            }
            let key = (Rc::as_ptr(a) as usize, Rc::as_ptr(b) as usize);
            if !seen.insert(key) {
                return Ok(());
            }
            let av = a.borrow();
            let bv = b.borrow();
            let max_len = av.len().max(bv.len());
            for idx in 0..max_len {
                if out.len() >= 2000 || *steps >= 200_000 {
                    break;
                }
                *steps += 1;
                if (*steps & 1023) == 0 {
                    vm.check_timeout()?;
                }
                let sub = format!("{}[{}]", path, idx);
                match (av.get(idx), bv.get(idx)) {
                    (Some(x), Some(y)) => {
                        collect_val_diffs(vm, &sub, x, y, out, depth + 1, seen, steps)?;
                    }
                    (Some(x), None) => record_entry(out, &sub, "removed", x.clone(), Value::Unit),
                    (None, Some(y)) => record_entry(out, &sub, "added", Value::Unit, y.clone()),
                    (None, None) => {}
                }
            }
        }
        (Value::Dict(a), Value::Dict(b)) => {
            if Rc::ptr_eq(a, b) {
                return Ok(());
            }
            let key = (Rc::as_ptr(a) as usize, Rc::as_ptr(b) as usize);
            if !seen.insert(key) {
                return Ok(());
            }
            let am = a.borrow();
            let bm = b.borrow();
            let mut keys: Vec<&String> = am.keys().chain(bm.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                if out.len() >= 2000 || *steps >= 200_000 {
                    break;
                }
                *steps += 1;
                if (*steps & 1023) == 0 {
                    vm.check_timeout()?;
                }
                let sub = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{}.{}", path, k)
                };
                match (am.get(k), bm.get(k)) {
                    (Some(x), Some(y)) => {
                        collect_val_diffs(vm, &sub, x, y, out, depth + 1, seen, steps)?;
                    }
                    (Some(x), None) => record_entry(out, &sub, "removed", x.clone(), Value::Unit),
                    (None, Some(y)) => record_entry(out, &sub, "added", Value::Unit, y.clone()),
                    (None, None) => {}
                }
            }
        }
        _ => {
            if !left.equals_vm(right, vm)? {
                let p = if path.is_empty() { "$" } else { path };
                record_entry(out, p, "changed", left.clone(), right.clone());
            }
        }
    }
    Ok(())
}

fn diff_values(vm: &mut VM, args: &[Value]) -> Result<Value, String> {
    if args.len() != 2 {
        return Err("diff.values() expects 2 arguments".to_string());
    }
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut steps = 0usize;
    collect_val_diffs(
        vm, "", &args[0], &args[1], &mut out, 0, &mut seen, &mut steps,
    )?;
    vm.charge_alloc(out.len().saturating_mul(160))?;
    Ok(Value::List(Rc::new(RefCell::new(out))))
}
