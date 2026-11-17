# Askane Language Specification (v0.14.1)

## 1. Types
- `int`: 64-bit signed integer (`42`, `-7`)
- `float`: 64-bit IEEE-754 floating point (`3.14`)
- `bool`: `true`, `false`
- `str`: UTF-8 string (`"hello"`, raw `r"C:\path"`, multiline `"""..."""`)
- `list`: mutable dynamic array (`[1, 2, 3]`)
- `dict`: mutable string-keyed map (`#{ name: "Askane", hp: 100 }`)
- `range`: lazy integer sequence (`range(10)`, `range(1, 10)`)
- `fn`: closure or native function
- `module`: built-in or imported `.ask` namespace
- `unit`: empty / null-like value (`unit`, `()`)

Truthiness: `false`, `unit`, `0`, `0.0`, `NaN`, `""`, `[]`, `#{}` are falsy; all other values are truthy.

## 2. Variables & Destructuring
```askane
let x = 10;
let [a, b] = [1, 2];
let #{ name, hp } = #{ name: "Hero", hp: 100 };
```
Compound assignments: `+=`, `-=`, `*=`, `/=` (supported on variables, list indices, and dict attributes).

## 3. Strings & Interpolation
- Standard strings support escapes (`\n`, `\r`, `\t`, `\"`, `\\`) and `${expr}` interpolation.
- Raw strings `r"..."` disable escape processing and interpolation.
- Multiline strings `"""..."""` preserve newlines across lines.
- Indexing (`s[i]`), slicing (`s[0..3]`, `s[1..=3]`), and `len(s)` operate on Unicode scalar values.

## 4. Operators & Precedence (lowest to highest)
1. Assignment: `=`, `+=`, `-=`, `*=`, `/=`
2. Null-coalescing: `??`
3. Logical OR: `||`
4. Logical AND: `&&`
5. Equality: `==`, `!=`
6. Comparison & membership: `<`, `<=`, `>`, `>=`, `in`
7. Pipeline: `|>` (`x |> f(a)` desugars to `f(x, a)`)
8. Addition / Subtraction: `+`, `-`
9. Multiplication / Division / Modulo: `*`, `/`, `%`
10. Unary: `-`, `!`, `throw`
11. Postfix: `call()`, `index[i]`, `slice[a..b]`, `obj.attr`, `obj?.attr`

## 5. Control Flow & Expressions
Blocks `{ ... }`, `if / else`, `match`, and `try / catch` are expressions that evaluate to their tail expression (or `unit` if ending with `;`).

```askane
let label = match score {
    0..50 => "fail",
    50..80 => "ok",
    n if n >= 80 => "great: ${n}",
    _ => "other",
};

let safe = try {
    10 / 0
} catch err {
    0
};
```

Loops: `while cond { ... }`, `loop { ... }`, `for x in seq { ... }`, `for k, v in dict_or_pairs { ... }` with `break` and `continue`.
List comprehensions: `[x * x for x in range(1, 10) if x % 2 == 0]`.

## 6. Functions & Closures
```askane
fn add(a, b) { a + b }
let sq = |n| n * n;
let forty_two = || 42;
```
Functions capture outer lexical variables by upvalue (closures).

## 7. Modules & Imports
- Built-in modules (`import math;`, `import fs as f;`): `math`, `fs`, `sys`, `json`, `strings`, `path`, `time`, `random`, `base64`, `csv`, `log`, `test`, `hash`, `ini`, `datetime`, `clipboard`, `audio`, `url`, `hex`, `uuid`, `term`, `glob`, `regex`, `dialog`, `process`, `http`, `gfx`, `mem`.
- File imports: `import "./helper.ask" as helper;`. Imported modules inherit the caller VM's permissions without privilege escalation.

## 8. Security & Resource Policy
By default scripts run in sandbox mode:
- `fs` is restricted to the script directory (device names, ADS, and symlink overwrites are rejected).
- `http`, `sys.env`, `sys.args`, `sys.exit`, `clipboard`, `process`, and `mem` are denied unless explicitly enabled (`--allow-net`, `--allow-read`, `--allow-write`, `--allow-env`, `--allow-args`, `--allow-clipboard`, `--allow-exec`, or `--unsafe`).
- Default limits: 10,000,000 VM instructions (`--max-steps=N`) and 30,000 ms wall-clock deadline (`--timeout=MS`).
