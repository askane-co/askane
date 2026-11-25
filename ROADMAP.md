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
| `0.14.3` | **Done** | Security, compiler, VM, and stdlib bugfix release (timeout enforcement, module recursion, glob boundaries, DAG limits, GUI/process/fs/fmt fixes) |
| `0.15.0` | **Done** | First-class `bytes`, unified `ResourceBudget`, working `askane.toml`, multi-file `.ask` projects, AST `fmt`, `struct` & `set` |
| `0.16.0 - 0.16.2` | **Done** | 2D Desktop UI (`ui`), signature pipelines (`|?>`, `|*>`, `recover`), `needs`/`sandbox`, `with`/`defer`, security fixes, embedded icon & `askane setup` |
| `0.17.0 - 0.17.1` | **Done** | Askane Studio v2 + Setup UI, `crypto`, `kv`, `net`, `ws`, cooperative `task`/`channel`, `--deterministic`, snapshots, reactive `signal` & `on change` |
| `0.18.0` | **Done** | Diagnostics (`E...`/`W...`, `--explain`, `--diagnostic-format=json`, `did you mean`, recovery, lints), semantics/helpers, `gfx` alpha/`key_pressed`, CLI `info`/`doctor`/`modules`/`clean` |
| `0.19.0` | **Planned** | Bytecode cache (`.askc`), streaming `fs` (`read_lines`/`walk`/`temp_file`/`copy_dir`), `fmt --diff`/`--stdin`, `test --filter`, `gfx` layers & sprite animation |
| `0.20.0 - 0.22.0` | **Exploratory** | Linux/macOS backends (`http`, `gfx`/`ui`, `clipboard`), `askane lsp` & VS Code, `askane bundle`, fuzzing corpus, AppContainer/DNS pinning, experimental 3D & baseline JIT |

---

## Zero-Crate Philosophy and Trade-offs

Askane targets a compact single-binary runtime (~4 MB on Windows) without pulling dozens of third-party crates into `Cargo.lock`. In practice, this follows two rules:

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

### `0.14.3` - Audit Bugfix & Stability Release [Done]
- **VM, Sandbox & CLI (`vm.rs`, `value.rs`, `main.rs`, `bytecode.rs`):** separate `step_counter` for `--timeout` even under `--unsafe` and preserve explicit CLI limits regardless of flag order; resolve relative CLI `--allow-read`/`--allow-write` against startup CWD (including non-existent `--allow-write` ancestors) and normalize `\\?\` / Windows case; bound DAG equality and `Display` formatting; add periodic `check_timeout()` in heavy natives; verify `Op::MakeModule` and disallow `Constant::Function` in `Op::Constant`; fix `askane permissions` depth tracking, `collect_ask_files` symlink safety, and `fmt`/REPL multiline & raw strings.
- **Compiler & Parser (`compiler.rs`, `parser.rs`, `ast.rs`):** enable recursion for local and file-module `fn` declarations (`scope_depth > 0`), guard `match` range arms for non-numeric subjects, and evaluate compound-assignment targets (`arr[idx()] += ...`, `obj().x += ...`) once.
- **Modules & Studio (`process.rs`, `fs.rs`, `glob.rs`, `regex.rs`, `math.rs`, `datetime.rs`, `gfx.rs`, `gui.rs`):** eliminate pipe-reader hangs past `--timeout` on detached child processes; enforce `**/` segment boundaries in iterative DP `glob.match`; fix `Op::Slice`, `math.abs(i64::MIN)`, and `datetime.parse_iso` overflow panics; fix `regex` per-start step counting, `[...]` escapes, and `replace` match-limit errors; bound `fs.copy()`, preserve symlinks on `fs.remove()`/`fs.move()`, fix `fs.glob()` in module subdirs; clear `gfx` state on close/VM drop; fix GUI Job Object `KILL_ON_JOB_CLOSE`, handle-close races, and temp cleanup.

---

### `0.15.0` - `bytes`, `ResourceBudget`, Modules, `struct` & `set`, VM Optimization [Done]

This release focuses on strengthening the existing foundation before adding heavy features like `async`, FFI, or JIT:

1. **First-class `bytes` type and `ResourceBudget`:**
   - native `bytes` type (`b"..."`, `br"..."`/`rb"..."`, indexing, slicing, concatenation, membership) backed by `Rc<Vec<u8>>`, wired across `bytes`, `fs`, `hex`, `base64`, `hash`, `random`, and `strings` modules;
   - unified `ResourceBudget` tracking memory allocations (`--max-mem=MB`), I/O bytes, and native work cost before large allocations, plus `sys.budget()` inspection and unrecoverable budget errors that `try`/`catch` cannot swallow;
   - fast `ForPrep`/`ForIter` loops, depth-thresholded tail call optimization (`Op::TailCall`), epoch-guarded global and struct-field inline caches, in-place integer fast paths, and `--profile`.
2. **Real project support (`askane.toml`) and multi-file `.ask` modules:**
   - CLI commands (`askane run`, `askane test`, `askane check`) automatically read declarative `askane.toml` manifests (`entry`, `tests`, `[limits]`), while keeping manifests non-authoritative for sandbox capabilities;
   - each `.ask` closure remembers its source `base_dir` for nested relative imports within `sandbox_root`; added `pub` export visibility, `from ... import ... [as ...]`, directory `mod.ask` packages, and `is_main()`.
3. **Tooling, structs, sets, and language ergonomics:**
   - token-aware, syntax-verified `askane fmt [--check] <file_or_dir>` safe for comments, raw/multiline strings, and golden corpus tests;
   - `struct` declarations (`pub struct`), field defaults evaluated per instance, methods (`self`), copy-update (`Player { hp: 80, ..p1 }`), value equality (`==`), `dict()`/`to_dict()`/JSON serialization, and protocol hooks (`fmt`, `contains`, `iter`, `close`);
   - built-in `set(...)` with $O(1)$ membership (`x in s`) and `union`/`intersect`/`diff`, default function parameters evaluated per call, named arguments, `*args`, string/list/set/bytes/dict methods, `not in`, and short-circuiting chained comparisons (`0 <= x <= 10`).

---

### `0.16.0 - 0.16.2` - Pipelines, Contracts, Declarative UI & Setup [Done]
- **Signature syntax & error recovery:** pipeline placeholder `_`, short-circuit `|?>`, element-wise `|*>`, in-place `|>=`, inline `recover`, typed templates (`sql"..."`, `log"..."`), and `${x=}` debug interpolation.
- **Security contracts & RAII:** `needs` capability declarations, `sandbox { ... }` privilege attenuation, `with` resource scoping, `defer { ... }` cleanup unwinds, time literals (`16ms`, `2s`), and `every` / `after` reactive blocks.
- **Desktop `ui`, stdlib & Windows setup (`0.16.1 - 0.16.2`):** declarative `ui` module (`window`, `column`, `row`, `label`, `button`, `text_input`, `checkbox`, `slider`, `select`, themes), PNG sprite loading in `gfx`, global `input` (`--allow-input`), `diff`, `pprint`, `html`, `xml`, `toml`, `binary`, `collections`, `compress`, `archive`, embedded app icon, and `askane setup`.

### `0.17.0 - 0.17.1` - Storage, Crypto, Networking, Concurrency & Studio v2 [Done]
- **Crypto, storage & realtime networking:** `crypto` (`hmac_sha256`, `constant_time_eq`, `random_key`, ChaCha20-Poly1305 `seal`/`open`), append-only CRC32 `kv` store with `compact()`, sandboxed TCP/UDP `net`, and RFC 6455 WebSocket `ws` (including Close handshake frames).
- **Concurrency, reactivity & testing:** cooperative `task` (`spawn`, `yield_now`, `sleep`, `join`, `select`, `timeout`) and bounded `channel`, reactive `signal` + `on change`, `spec "..." { ... }` test blocks, `--deterministic` simulation, `assert_snapshot`, and `TailCall` / upvalue / `try` fixes across scheduler switches.
- **Askane Studio v2 & setup UI:** dark Win32 IDE with project explorer, tabs, line numbers, find/replace (`Ctrl+F`/`Ctrl+H`), permission toggles, Setup UI dialog, interactive stdin, Job Object isolation, and REPL `:modules` (49 built-in modules).

### `0.18.0` - Diagnostics, Semantics & Developer Experience [Done]
- **Diagnostics, recovery & lints (`diag.rs`, `ast.rs`, `parser.rs`, `compiler.rs`, `vm.rs`):** structured codes (`E0101`..`E0405`, `W0101`, `W0102`), Levenshtein `did you mean` hints, exact imported file/line + `(imported from '...')` chains, accurate instruction IP / stack traces (`execute_fast_inner`), ANSI/NO_COLOR output, `--explain`, unified JSON for `askane check --diagnostic-format=json` (files, directories, read errors), multi-error parse recovery without depth leaks, unused-local (`W0101`) with identifier spans, and duplicate-`match` (`W0102`, including floats & ranges) warnings.
- **Language semantics, types & stdlib (`value.rs`, `vm.rs`, `strings.rs`, `collections.rs`):** checked/wrapping integer arithmetic (`checked_add`/`sub`/`mul`, `wrapping_add`/`sub`/`mul`), `is_same` (refs, `BoundMethod`, `NaN`), `repr`, `try_int`/`try_float`, type predicates (`is_unit`..`is_fn`), exact `cmp_int_float` above `2^53`, deterministic dict `Display` key ordering, collection helpers (`first`, `last`, `is_empty`, `find_index`, `count`, `partition`, `flat_map`, `take_while`, `drop_while`, `zip_longest`, `min_by`, `max_by`), and `strings` extensions (`byte_len`, `eq_ignore_case`/`contains_ignore_case` with `ß`/`ẞ` folding, `strip_prefix`/`strip_suffix`, `split_once`/`rsplit_once`, `truncate_utf8`).
- **`gfx` 2D helpers (`gfx.rs`):** `rect_alpha`, `circle_alpha`, `width()`, `height()`, and per-frame `key_pressed()`, with active-window guards and `#[cfg(not(windows))]` CI compatibility.
- **CLI tooling (`main.rs`, `modules/mod.rs`, `sys.rs`, `os.rs`):** `askane info`, `askane doctor`, `askane modules [--json]`, `askane clean [--dry-run] [--snapshots]`, `askane init [--name]`, strict flag validation, `--` script argument passthrough to `sys.args()`/`os.args()`, and alias/`from import`/named-call `[dynamic]` + `sandbox` notices in `askane permissions`.

---

## Next Milestone: `0.19.0` (`Planned`)

1. **Bytecode cache (`.askc`)**:
   - content-hashed and schema-versioned `.askc` module cache under project/temp boundaries with mandatory `verify_proto` validation on load;
   - seamless invalidation when source files or compiler version change, controllable via `askane clean`.
2. **Streaming & directory filesystem helpers (`fs`)**:
   - size- and line-bounded `fs.read_lines`, streaming text/byte writers, and `fs.temp_file` with safe automatic cleanup;
   - `fs.walk` (explicit symlink policy, depth/entry limits), `fs.copy_dir`, and explicit `fs.remove_dir` separation.
3. **Formatter, test ergonomics & 2D game helpers**:
   - `askane fmt --diff` and `askane fmt --stdin` backed by project `[fmt]` settings in `askane.toml` and golden corpus tests;
   - `askane test --filter <pattern>` for focused test runs;
   - `gfx` 2D layers, sprite-sheet frame helpers, normalized `delta`, and headless frame-step testing for game loops.

---

## Long-Term Ideas (`0.20 - 0.22`, `Exploratory`)

These items are tracked across future milestones as exploratory directions rather than committed release scope, and promote to `Planned` once prerequisites land:

- **`0.20` - Cross-Platform Backends & IDE Integration:**
  - portable Linux and macOS backends for `http` (TLS), `gfx`/`ui`, and `clipboard` without changing `.ask` user code;
  - built-in `askane lsp` server over JSON diagnostics, VS Code syntax/extension, shell completions (PowerShell/Bash/Zsh), and single-binary `askane bundle`.
- **`0.21` - Fuzzing, Hardening & Release Discipline:**
  - continuous benchmark suite tracking startup, VM throughput, and release binary size regressions;
  - lexer/parser/compiler/verifier/VM fuzzing corpus guaranteeing zero Rust panics on malformed inputs;
  - OS isolation hardening (Windows AppContainer / low-integrity worker, `http` DNS rebinding pinning, handle-based directory I/O), `CHANGELOG.md`, and release checksums.
- **`0.22` - Experimental 3D & Native Acceleration:**
  - experimental 3D camera, transforms, basic meshes, materials, and lighting layered cleanly over the portable graphics backend;
  - baseline JIT / AOT exploration for hot numeric kernels while preserving Askane's zero-crate simplicity.
