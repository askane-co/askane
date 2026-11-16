# Askane (Open Beta)

Askane is an experimental, bytecode-compiled scripting language written in Rust with zero external crate dependencies.

> **Transparency note**: This language and its runtime were designed and built in collaboration with **Gemini** (AI-assisted development). I am publishing it as an open beta so anyone can test it, inspect the code, break things, and suggest improvements.

## How It Works Under the Hood

Askane runs as a classic 4-stage pipeline in a single standalone binary:

1. **Lexer** (`src/lexer.rs`): scans `.ask` source code into tokens with line/column spans.
2. **Parser** (`src/parser.rs`): builds an AST (`src/ast.rs`). Blocks, `if`/`else`, and `match` are expressions, and `|>` desugars `x |> f(a)` into `f(x, a)`.
3. **Bytecode Compiler** (`src/compiler.rs`, `src/bytecode.rs`): compiles the AST into stack-VM opcodes, resolving local variable slots, lexical closures (upvalues), loops, and `match` jumps.
4. **Stack VM & Modules** (`src/vm.rs`, `src/value.rs`, `src/modules/`): executes bytecode on a value stack with built-in functions and standard modules (`math`, `fs`, `sys`, `json`, plus Windows-native `http`, `gfx`, and `mem` using system Win32 DLLs).

## Build & Run

Requires Rust (`cargo`).

```bash
cargo build --release
```

Run a script:

```bash
cargo run -- examples/hello.ask
```

Or using the compiled binary:

```bash
./target/release/askane examples/pipeline.ask
```

Inspect compiled bytecode:

```bash
./target/release/askane -b examples/pipeline.ask
```

Start the interactive REPL:

```bash
./target/release/askane
```

## Quick Syntax Overview

```askane
// Everything is an expression
let x = {
    let a = 10;
    a * 2
};

// Pipelines with short lambdas
range(1, 7)
    |> filter(|n| n % 2 == 0)
    |> map(|n| n * n)
    |> println;

// Pattern matching with guards and string interpolation
let label = match x {
    0 => "zero",
    n if n > 10 => "big: ${n}",
    other => "value: ${other}",
};

// Dictionaries, dot-assignment, optional chaining (?.) and ??
let player = #{ name: "Askane", hp: 100 };
player.hp = 90;
let mana = player?.mana ?? 50;
println("${player.name}: hp=${player.hp}, mana=${mana}", keys(player));
```

## Built-in Functions & Modules

Global functions:
- I/O: `print(...)`, `println(...)`, `read_line()`
- Collections & Strings: `len(x)`, `push(list, val)`, `pop(list)`, `range(end)` / `range(start, end)`, `keys(dict)`, `has(dict, key)`
- Higher-order: `map(list, fn)`, `filter(list, fn)`, `reduce(list, init, fn)`
- Conversions & Utils: `str(x)`, `int(x)`, `float(x)`, `type_of(x)`, `clock()`

Built-in standard modules (`import <name>;`):
- `math`: `pi`, `e`, `sqrt`, `abs`, `pow`, `sin`, `cos`, `floor`, `ceil`, `round`, `min`, `max`
- `fs`: `read`, `write`, `append`, `exists`, `remove`, `list_dir`, `mkdir`, `is_dir`
- `sys`: `args`, `env` (requires `--unsafe`), `exit`, `os`, `arch`, `sleep`
- `json`: `parse`, `stringify`
- `http` (Windows): `get(url, [headers])`, `post(url, body, [content_type_or_headers])`
- `gfx` (Windows 2D): `window`, `is_open`, `clear`, `rect`, `circle`, `line`, `pixel`, `text`, `present`, `key_down`, `mouse_x`, `mouse_y`, `mouse_down`, `close`
- `mem` (Windows): `current_pid`, `find_pid` (returns `unit` if not found), `open`, `close`, `module_base` (returns `unit` if not found), `read_i32`, `read_i64`, `read_f32`, `read_f64`, `read_bytes`, `write_i32`, `write_i64`, `write_f32`, `write_f64`

## Security model

Scripts run in a filesystem sandbox rooted at the script's directory. The `fs` module
cannot escape this directory through absolute paths, `..`, or symlinks. HTTP requests
require HTTPS. The Windows `mem` module is disabled by default because it can inspect
and modify other processes; enable it only for trusted scripts with
`ASKANE_ENABLE_UNSAFE_MEM=1`. The `--unsafe` command-line option disables the filesystem
sandbox and should only be used with trusted code.

## License

MIT
