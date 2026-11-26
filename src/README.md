# Askane Source Code Tour

Askane compiles `.ask` scripts through a clean 4-stage pipeline inside a single zero-crate Rust binary. Click any file below to jump straight to its implementation.

## Core Compiler, VM & Tooling

| File | Role |
| --- | --- |
| [main.rs](main.rs) | CLI frontend (`run`, `check`, `test`, `fmt`, `init`, `setup`, `info`, `doctor`, `modules`, `clean`, `permissions`, REPL) |
| [lexer.rs](lexer.rs) | Tokenizer with line/column spans, strings, raw/bytes literals, comments |
| [ast.rs](ast.rs) | Abstract Syntax Tree nodes (expressions, statements, patterns, `spec` blocks) |
| [parser.rs](parser.rs) | Recursive-descent parser, precedence, error recovery, pipelines (`\|>`, `\|?>`, `\|*>`, `\|>=`) |
| [compiler.rs](compiler.rs) | AST-to-bytecode compiler, local slots, upvalues, loops, `match` jumps |
| [bytecode.rs](bytecode.rs) | Opcode definitions, `FunctionProto`, `.askc` SHA-256 disk cache, and `verify_proto` |
| [value.rs](value.rs) | Runtime value representation, collections, structs, bound methods |
| [vm.rs](vm.rs) | Stack/register fast VM (`execute_fast_inner`), `ResourceBudget`, capability checks |
| [diag.rs](diag.rs) | Structured diagnostics (`E...` / `W...`), `did you mean`, JSON output, `--explain` |
| [gui.rs](gui.rs) | Askane Studio v2 (Windows Win32 editor, project tree, tabs, Job Object runner) |
| [setup.rs](setup.rs) | Windows User `PATH` & `.ask` Explorer association installer (`askane setup`) |

## Standard Library (`49` Built-in Modules)

- Registry & exports: [modules/mod.rs](modules/mod.rs)
- Runtime & concurrency: [sys.rs](modules/sys.rs), [os.rs](modules/os.rs), [task.rs](modules/task.rs) (`task` + `channel`), [signal.rs](modules/signal.rs), [time.rs](modules/time.rs), [datetime.rs](modules/datetime.rs), [cli.rs](modules/cli.rs), [process.rs](modules/process.rs), [mem.rs](modules/mem.rs)
- Filesystem & archives: [fs.rs](modules/fs.rs), [path.rs](modules/path.rs), [glob.rs](modules/glob.rs), [kv.rs](modules/kv.rs), [compress.rs](modules/compress.rs), [archive.rs](modules/archive.rs)
- Data, text & binary: [strings.rs](modules/strings.rs), [regex.rs](modules/regex.rs), [bytes.rs](modules/bytes.rs), [binary.rs](modules/binary.rs), [base64.rs](modules/base64.rs), [hex.rs](modules/hex.rs), [json.rs](modules/json.rs), [csv.rs](modules/csv.rs), [ini.rs](modules/ini.rs), [toml.rs](modules/toml.rs), [html.rs](modules/html.rs), [xml.rs](modules/xml.rs), [url.rs](modules/url.rs), [uuid.rs](modules/uuid.rs)
- Math, collections & utilities: [math.rs](modules/math.rs), [collections.rs](modules/collections.rs), [random.rs](modules/random.rs), [hash.rs](modules/hash.rs), [crypto.rs](modules/crypto.rs), [diff.rs](modules/diff.rs), [pprint.rs](modules/pprint.rs), [log.rs](modules/log.rs), [test.rs](modules/test.rs), [term.rs](modules/term.rs)
- Networking, UI, graphics & audio: [http.rs](modules/http.rs), [net.rs](modules/net.rs), [ws.rs](modules/ws.rs), [gfx.rs](modules/gfx.rs), [ui.rs](modules/ui.rs), [input.rs](modules/input.rs), [audio.rs](modules/audio.rs), [clipboard.rs](modules/clipboard.rs), [dialog.rs](modules/dialog.rs)
