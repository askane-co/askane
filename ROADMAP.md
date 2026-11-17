# Askane Roadmap

![Askane Roadmap](docs/roadmap.jpg)

Release roadmap for Askane, separated into completed milestones, immediate fixes, planned releases, and long-term exploratory ideas.

## Version Status

| Version | Status | Primary Goal |
| --- | --- | --- |
| `0.11.0` | **Done** | Granular capability flags, `try`/`catch`/`throw`, lazy `range` |
| `0.12.0` | **Done** | Askane Studio GUI, multiline REPL, `check`/`test`, core stdlib (8 modules) |
| `0.13.0` | **Done** | Destructuring, lazy sequences, GUI Job Object, `verify_proto`, `--timeout`, `hash`/`ini`/`datetime`/`clipboard` |
| `0.14.0 - 0.14.1` | **Done** | Extended syntax (`+=`, `in`, `loop`, slices, list comprehensions, `match` ranges), 9 modules, BMP sprites, `fmt`/`init`, interactive stdin |
| `0.14.2-fix` | **Done** | Compiler stack-slot offset fix (`compiler.rs`), Job Object isolation for CLI `process.run()`, and sandbox security hardening |
| `0.15.0` | **Next** | First-class `bytes`, unified `ResourceBudget`, working `askane.toml`, multi-file `.ask` projects, AST `fmt`, `struct` & `set` |
| `0.16.0` | **Exploratory** | 2D Desktop UI (`ui`) module before 3D, signature pipelines (`|?>`, `|*>`, `recover`), `needs`/`sandbox`, `with`/`defer` |
| `0.17 - 0.22` | **Exploratory** | Storage/networking (`kv`, `ws`, `crypto`), diagnostics/LSP & Studio v2, cross-platform backends, fuzzing, experimental 3D (`0.22`) |

---

## Zero-Crate Philosophy and Trade-offs

Askane targets a single compact binary (~1.6 MB) without pulling dozens of third-party crates into `Cargo.lock`. In practice, this follows two rules:

1. **Simple parsers and core algorithms are written in-house.** The lexer, parser, compiler, bytecode VM, JSON, CSV, INI, URL, BMP decoder, CRC32/SHA-256, and a bounded linear-time `regex` subset live in the codebase with strict input-size limits.
2. **OS cryptography, process isolation, and UI are delegated to the platform.** CSPRNG (`BCryptGenRandom`), dialogs/windows, audio (`winmm.dll`), clipboard, and Job Objects use native Windows system libraries instead of reimplementing OS primitives from scratch. Where cross-platform portability or complex standards are needed (TLS/HTTPS, compression, embedded DB engines), the trade-off between system APIs, a minimal audited crate, or a bounded built-in subset is evaluated per feature.

---

## Completed Releases (`Done`)

### `0.11.0 - 0.12.0` - Language Core, Sandbox, and Studio [Done]
- **Capability model and isolation:** granular flags (`--allow-net`, `--allow-read`, `--allow-write`, `--allow-env`, `--allow-args`, `--allow-gfx`), step budgets (`--max-steps`), and capability inheritance across imports without privilege escalation.
- **Language and runtime:** `try / catch / throw`, lazy `range`, deterministic collection helpers (`keys`, `values`, `entries`, `get`, `remove`, `sort`, `reverse`).
- **Tooling:** multiline REPL (`:reset`, `:modules`, `:bc`), source-snippet diagnostics, `askane check [--json]`, and `askane test`.
- **Askane Studio GUI:** built-in Win32 editor and runner executing scripts in an isolated child process.
- **Stdlib:** `strings`, `path`, `time`, `random` (CSPRNG rejection sampling), `base64`, `csv`, `log`, and `test`.

### `0.13.0` - Tooling, Safety Hardening, and Sequences [Done]
- **Syntax and sequences:** destructuring (`let [a, b] = ...`, `let #{ name, hp } = ...`), `enumerate`, `zip`, `take`, `skip`, `any`, `all`, and `find`.
- **CLI and GUI:** `askane -e`, `askane permissions`, `askane check <dir>`, `:gui` from REPL, Windows Job Object for `Stop`, and editor shortcuts (`F5`, `Ctrl+S`, `Ctrl+O`, `Ctrl+A`).
- **VM hardening:** pre-execution bytecode verifier (`verify_proto`), wall-clock `--timeout`, secret redaction in `log`, and opaque `mem` handles with PID checks and automatic close on drop.
- **Stdlib:** `hash` (`sha256`, `crc32`, `fnv1a`), `ini`, `datetime`, and `clipboard` (`--allow-clipboard`).

### `0.14.0 - 0.14.1` - Expressive Syntax, Assets, and CLI [Done]
- **Language:** compound assignment (`+=`, `-=`, `*=`, `/=`), `in` operator, `loop`, raw strings `r"..."`, multiline strings `"""..."""`, `for k, v in dict` unpacking, slices (`[a..b]`, `[a..=b]`, `[..b]`, `[a..]`), list comprehensions `[x for x in seq if cond]`, integer ranges in `match` (`0..50`, `80..=100`), and `unique`, `flatten`, `chunk`, `sum`.
- **Files and graphics:** `fs.read_bytes`, `fs.write_bytes`, `fs.copy`, `fs.move`, `fs.size`, and `fs.glob` with atomic temp-file writes and symlink overwrite refusal; BMP sprites in `gfx` (`load_image`, `draw_image`, `draw_image_keyed`, `rect_outline`, `circle_outline`, `set_title`, `delta`) with header validation.
- **Stdlib:** `audio`, `url`, `hex`, `uuid` (v4 via `BCryptGenRandom`), `term`, `glob`, `regex`, `dialog`, `process` (`--allow-exec`), and `math` extensions (`clamp`, `lerp`, `dist`, `atan2`).
- **Tooling and Studio (`0.14.1`):** `askane init`, `askane fmt [--check]`, GUI `Check` button, `Dark` theme toggle, and interactive `stdin` input + `Send` button.

---

### `0.14.2-fix` - Hotfix & Security Hardening [Done]
- **Compiler & Verifier (`compiler.rs`, `bytecode.rs`):** fix stack-slot calculation in `Expr::ListComp` (and `Block` / `Match` / `TryCatch`) when compiled inside compound expressions with temporary operands on the VM stack; count upvalue-only locals in `local_count`; validate `upvalue_lists` bounds in `verify_proto`.
- **`process.run()` in CLI (`process.rs`, `vm.rs`):** execute child processes inside a Windows Job Object (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) with enforced `--timeout` and output-size limits, and block path-separator bypasses on bare `--allow-exec=cmd` allowlists.
- **Sandbox & Runtime Hardening (`vm.rs`, `fs.rs`, `glob.rs`, `main.rs`):** unwind failed closures in `VM::call_value`, prevent `try`/`catch` and `test.assert_err` from swallowing sandbox limits/timeouts, block UNC/device paths and outside-sandbox existence probing, block writing executable extensions in sandbox mode, bound `glob.match` backtracking, and make `askane permissions` inspect the full AST and transitive file imports.

---

## Upcoming Release (`Planned`)

### `0.15.0` - `bytes`, `ResourceBudget`, Working `askane.toml`, and Modules [Planned]

This release focuses on strengthening the existing foundation before adding heavy features like `async`, FFI, or JIT:

1. **First-class `bytes` type and `ResourceBudget`:**
   - native `bytes` type (`b"..."`, indexing, slicing) backed by `Vec<u8>` wired across `fs`, `hex`, `base64`, `hash`, and networking instead of `Value::Int` lists;
   - unified `ResourceBudget` tracking memory allocations (`--max-mem=MB`), I/O sizes, and native-function costs before allocation;
   - specialized VM opcodes for faster loops, globals, and field access plus `--profile`.
2. **Real project support (`askane.toml`) and multi-file `.ask` modules:**
   - teach CLI commands (`askane run`, `askane test`, `askane check`) to read `askane.toml` generated by `askane init`;
   - resolve relative imports against the importing module's directory within `sandbox_root`, add `pub` visibility, selective imports, `mod.ask`, and `is_main()`.
3. **Tooling and data ergonomics:**
   - token/AST-based `askane fmt` safe for comments, raw/multiline strings, and corpus-tested;
   - basic `struct` with methods (`self`), value equality, and `set(...)` with $O(1)$ membership (`x in s`);
   - default function parameters, `not in`, chained comparisons (`0 <= x <= 10`), and string/list methods.

---

## Long-Term Ideas (`0.16 - 0.22`, `Exploratory`)

These items are tracked across future milestones as exploratory directions rather than committed release scope, and promote to `Planned` once prerequisites land:

- **`0.16` - 2D Desktop UI (`ui`) & Signature Askane Syntax:**
  - lightweight 2D UI module ahead of 3D so developers can build real desktop utilities (settings windows, launchers, calculators) with `label`, `button`, `text_input`, `checkbox`, `slider`, `select`, vertical/horizontal layouts, events, RGB/RGBA/HEX themes, and resize handling;
  - `_` pipeline placeholder, short-circuit `|?>` and element-wise `|*>` operators, inline `recover`, typed templates (`sql"..."`), `${x=}` debug formatting, `needs`/`sandbox` contracts, and `with`/`defer`.
- **`0.17` - Storage, Crypto, Concurrency & Networking:** embedded transactional `kv` store, CNG-backed `crypto`, TCP/UDP `net`, WebSockets `ws`, `yield`/`async`/`task`/`channel`, `spec "..."` blocks, and deterministic simulation testing.
- **`0.18 - 0.19` - Diagnostics, Tooling & Askane Studio v2:** error codes (`E...`), `--explain`, `--diagnostic-format=json`, transitive `askane permissions`, `askane doctor`, Askane Studio v2 (line numbers, syntax highlighting, tabs, find/replace, `Run tests`), streaming `fs.read_lines`, and 2D game layers/animations/input in `gfx`.
- **`0.20 - 0.21` - Cross-Platform, Ecosystem & Hardening:** portable Linux/macOS backends (`http`, `gfx`/`ui`, `clipboard`), `askane lsp` & VS Code extension, `askane bundle`, continuous performance benchmarks, fuzzing corpus for lexer/parser/bytecode/VM, OS isolation hardening (AppContainer, DNS rebinding protection, handle-based I/O), and `CHANGELOG`/release checksums.
- **`0.22` - Experimental 3D & Native Acceleration:** basic 3D camera, meshes, materials, and lighting layered on the portable graphics backend once 2D UI and 2D games are stable, plus baseline JIT/AOT research.
