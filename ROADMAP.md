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
| `0.19.0` | **Done** | Bytecode cache (`.askc` / `--no-cache`), streaming & directory `fs` (`read_lines`/`open_lines`/`write_stream`/`walk`/`copy_dir`/`temp_file`), `fmt --diff`/`--stdin` + `[fmt] indent`, `test --filter`, `gfx` sprite sheets, `mouse_pressed`, normalized `delta` & cross-platform `headless` |
| `0.20.0` | **Done** | Expanded media & world engine (`media::image` PNG/BMP/TGA/QOI/GIF/baseline JPEG, multi-channel `audio` `Mixer`/`Voice`/`Music`/`Bus` + `WAV`/`PCM`/`ADPCM`/`render_offline`, software `Surface` + DepthBuffer + fonts, Tiled maps, `.r16` terrain, `.obj`/`.mtl`/`.gltf`/`.glb` meshes, lighting/postfx, `TypedBuffer`, VM speedup `65-75+`, cross-platform HAL, `askane bundle`, `askane lsp`) |
| `0.21.0 - 0.22.0` | **Exploratory** | Software triangle rasterizer, fuzzed TTF fonts, opt-in compressed audio (`FLAC`/`MP3`/`OGG`), fuzzing corpus, AppContainer/DNS hardening, dedicated `0.22` `gfx3d` 3D module (camera, PBR, IBL, lighting, skinning) over frozen `0.20` contracts & baseline JIT exploration |

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

### `0.19.0` - Bytecode Cache, Streaming I/O, Formatter Tooling & Headless 2D Gfx [Done]
- **Bytecode cache (`bytecode.rs`, `vm.rs`, `main.rs`, `.gitignore`):** content-hashed (`SHA-256`) and schema/version-checked `.askc` cache under `__askcache__/`, mandatory `verify_proto` check on load, sandbox write/symlink guards, `--no-cache` CLI flag, and `askane clean` cleanup.
- **Streaming & directory `fs` (`fs.rs`, `mod.rs`):** line/byte-bounded `fs.read_lines` & `fs.write_lines`, RAII handles `fs.open_lines` (`next`/`close`) and `fs.open_writer`/`fs.write_stream` (`write`/`write_line`/`flush`/`close`) integrated with `with` and VM teardown, `fs.temp_file`, bounded symlink-safe `fs.walk`, `fs.copy_dir`, and `fs.remove_dir`.
- **Formatter, config & test ergonomics (`main.rs`, `diff.rs`):** `askane fmt --diff` unified diff preview, `askane fmt --stdin` pipe formatting, project `[fmt] indent = 1..=8` in `askane.toml`, and `askane test --filter=<pattern>` filtering test files and individual `spec "..."` blocks.
- **`gfx` 2D helpers & cross-platform headless mode (`gfx.rs`, `mod.rs`):** `gfx.draw_sprite_frame` for scaled sprite sheets, per-frame edge-triggered `gfx.mouse_pressed`, normalized/clamped `gfx.delta()`, and cross-platform framebuffer `gfx.headless(enabled, [w], [h])` for automated rendering tests on Windows and Linux CI.

---

## `0.20.0` - Media Engine, 3D Assets, HAL, Packaging & LSP [Done]

Designed around a clean `Core` + `Extension` architecture (zero external Rust crates, estimated `+410..685 KB` release binary growth to ~`4.8..5.1 MB`, `+5%` CI size gate) with 5 frozen runtime/asset contracts preparing Askane for native 3D in `0.22.0`:

- **Stage 1 - VM speed (`41.35 -> 65-75+`), `TypedBuffer`, `.askc v2` & `BudgetCore`:**
  - pack `enum Value` down to 16 bytes (`size_of::<Value>() == 16`) in 100% safe Rust, add stack `Vec2(f32, f32)`, `FxHash` map hashing, `SetKey::from_value` via `Rc::clone`, and zero-escape `&[u8]` scanning in `json.parse`;
  - add the `buffer` module (`TypedBuffer`: `U8`, `I32`, `U32`, `F32` with copy-on-write mutation at `Rc::strong_count == 1` and zero `unsafe`) as the binary backbone for pixels, audio PCM, vertices, indices, and heightmaps;
  - drop `*last_ip` writes on non-throwing hot opcodes, introduce register `steps_until_sync`, lightweight `CallFrame`, `Op::CallSelf` / `IndexLocal` / `SetIndexLocal` / `PushLocal`, zero-alloc `ForRange`, `Op::step_cost()`, atomic `BudgetCore` with RAII `TempCharge`, and `BYTECODE_CACHE_FORMAT_VERSION = 2` with opcode roundtrip verification.
- **Stage 2 - Software `Surface`, layers, `Camera2D`, `geom` & raster fonts:**
  - unified `0xAARRGGBB` software `Surface` across Windows (`StretchDIBits`), Linux (raw X11 `PutImage`), and headless CI, plus up to `32` layers and NDC `[0, 1]` `DepthBuffer` (`frag_z < buf_z`, clear `1.0`);
  - freeze coordinate canon (2D Y-down, 3D RH Y-up `-Z` forward) and add `geom` / `scene::math` (`Vec2..4`, `Mat4`, `Quat`, `Transform`, `Aabb`, `Ray`, `Plane`, `Frustum`);
  - built-in Latin + Cyrillic raster font, BMFont `.fnt` loader, and `ui.image` / `ui.rect` / `nine_slice` / `icon_button` widgets (TTF deferred to `0.21` with fuzz corpus).
- **Stage 3 - Unified `media::image` pipeline, textures & sprite animations:**
  - single shared decoder across `image`, `gfx`, `ui`, and `assets`: `PNG`, `BMP`, `TGA`, `QOI` in Core plus animated `GIF` and baseline `JPEG` (`SOF0`/`SOF1`, rejecting `SOF2` progressive with `E_MEDIA_UNSUPPORTED`) in Extension;
  - sprite atlases, frame animations, `draw_sprite_ex` (flip, rotation, tint, scale, origin), `Rgba8`/`R8` mipmaps, and strict pre-alloc decompression-bomb limits (`MAX_DIM = 8192`, `MAX_PIXELS = 33,554,432`, `MAX_MEDIA_BYTES = 64 MiB`, `>= 150` fuzz/bomb tests).
- **Stage 4 - Multi-channel `audio`, music & deterministic `Mixer`:**
  - 100% safe-Rust `Mixer` (`S16Le`/`F32Le`, linear resampler, wavetable `sin` synth) with `Voice`, `Music`, and `Bus` handles (`MAX_VOICES = 64`, hard cap `256`, `MAX_MUSICS = 8`, `MAX_BUSES = 16` + master, `MAX_SOUND_BYTES = 64 MiB`);
  - Core codecs `WAV`/`PCM`, `IMA ADPCM`, `A-law`, `u-law` (`FLAC`/`MP3`/`OGG` deferred to `0.21` opt-in outside `--deterministic`), deterministic `render_offline` WAV export, universal `NullSink`, Win32 `waveOut` via `CALLBACK_EVENT`, Linux `dlopen("libasound.so.2")`, and `--allow-audio` (preserving backward compatibility for legacy `audio.play`/`stop` under `--allow-read`).
- **Stage 5 - Non-`.ask` maps, terrain, 3D file formats, lighting & post-FX (`0.22` bridge):**
  - unified asset pipeline (`AssetRef::parse`, `AssetStore`, binary asset cache `ASKAS`) supporting non-`.ask` files directly;
  - Tiled tilemaps (`.tmj`, `.tmx`, `.tsj`, `.tsx` orthogonal with `SolidGrid` collision), `terrain` heightmaps (`.r16`, 8/16-bit PNG, `height_at`, `normal_at`, Mode-7 projection), and `mesh` loaders for `.obj` + `.mtl` (PBR conversion in Core) and `.gltf` / `.glb` (Extension) producing shared `MeshData` + frozen PBR `Material`;
  - `lighting` (half-res `RGBA16` light buffer, 16x16 tile culling, tile-DDA `cast_shadows`), `postfx` (bloom, LUT, vignette), `particles`, `procgen`, `nav`, and `scene.assemble` / `scene.draw` (depth-sorted billboards in `0.20`).
- **Stage 6 - Cross-platform HAL, `askane bundle` & compile-time asset audit:**
  - isolated `src/platform/dl.rs` loader (`LoadLibraryW` / `dlopen`), Linux raw X11 over `UnixStream` + `libssl.so.3` TLS IP pinning, and 4-job CI (`windows-latest`, `ubuntu-latest bare`, `ubuntu-backends`, `macos-latest`);
  - single-binary `askane bundle` (`ASKANE-PAK` format with EOF SHA-256 trailer, in-RAM verification, and hermetic mode) plus compile-time literal asset-path checks in `askane check` / `askane permissions` (`E0501..E0506`, `W0501..W0505`).
- **Stage 7 - Documentation sync, game starter & `askane lsp`:**
  - centralized `REGISTRY` + per-module `FN_DOCS` with CI-enforced `docs_sync`, `askane init --template game` (<4 KiB const starter assets), `askane doctor` / `sys.backends()`, and `askane lsp` over stdio JSON-RPC (UTF-16 positions, diagnostics, completion, hover); dedicated `embed` syntax deferred past `0.20.0`.

---

## Long-Term Ideas (`0.21 - 0.22`, `Exploratory`)

These items are tracked across future milestones as exploratory directions rather than committed release scope, and promote to `Planned` once prerequisites land:

- **`0.21` - Rasterizer, Fuzzing, Hardening & Release Discipline:**
  - software triangle rasterizer layered cleanly over the `0.20` `Surface`, `DepthBuffer`, and `MeshData`;
  - fuzzed TTF font support (with hinting bytecode disabled) and opt-in compressed audio (`FLAC`/`MP3`/`OGG`) outside `--deterministic`;
  - continuous benchmark suite tracking startup, VM throughput, and release binary size regressions, plus lexer/parser/compiler/verifier/VM fuzzing guaranteeing zero Rust panics on malformed inputs;
  - OS isolation hardening (Windows AppContainer / low-integrity worker, `http` DNS rebinding pinning, handle-based directory I/O), `CHANGELOG.md`, and release checksums.
- **`0.22` - Dedicated 3D Module (`gfx3d`) & Native Acceleration:**
  - dedicated `gfx3d` module (perspective camera, meshes, PBR materials, IBL, lighting, shadows, and skeletal skinning) built directly on the frozen `0.20` contracts (`TypedBuffer`, `geom`, `MeshData`, `Material`, NDC `[0, 1]` depth);
  - optional `ffi` module (`.dll`/`.so` gated behind `--allow-ffi`) and baseline JIT / AOT exploration for hot numeric kernels while preserving Askane's zero-crate simplicity.
