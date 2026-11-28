<!-- askane:diagnostics:begin -->
# Askane Diagnostic Code Reference

Total documented codes: `45` (auto-synced with `src/diag.rs`). Run `askane --explain <CODE>` for full details.

| Code | Kind | Summary |
| --- | --- | --- |
| `E0101` | error | Lexer syntax error |
| `E0201` | error | Parser syntax error |
| `E0202` | error | Parser depth or expression limit exceeded |
| `E0301` | error | Compiler semantic or bytecode limit error |
| `E0302` | error | Parameter, field, or argument count mismatch |
| `E0401` | error | Undefined variable, field, or module export |
| `E0402` | error | Module import or capability declaration error |
| `E0403` | error | Type or operator mismatch at runtime |
| `E0404` | error | General runtime error |
| `E0405` | error | Sandbox policy or resource budget violation |
| `E0501` | error | Media allocation or dimension limit exceeded |
| `E0502` | error | Unsupported media format |
| `E0503` | error | Corrupt media payload |
| `E0504` | error | Invalid media option or path |
| `E0505` | error | Non-deterministic operation in deterministic mode |
| `E0506` | error | Audio command/event queue backpressure |
| `E0507` | error | Unsupported audio stream format |
| `E0508` | error | Real-time audio sink disallowed |
| `E0509` | error | Audio device permission denied |
| `E0510` | error | Asset JSON limit or syntax error |
| `E0511` | error | Asset XML limit or syntax error |
| `E0512` | error | Mesh geometry limit exceeded |
| `E0513` | error | Corrupt 3D model data |
| `E0514` | error | 3D scene node cycle |
| `E0515` | error | Unsupported 3D feature |
| `E0520` | error | Unsupported Tiled map feature |
| `E0521` | error | Corrupt Tiled map payload |
| `E0530` | error | Terrain limit exceeded |
| `E0531` | error | Corrupt terrain payload |
| `E0532` | error | Invalid terrain option |
| `E0533` | error | Invalid splatmap |
| `E0540` | error | Invalid cubemap |
| `E0550` | error | Invalid 3D LUT file |
| `E0551` | error | Invalid navigation grid or coordinates |
| `E0552` | error | Invalid scene assembly spec |
| `E0560` | error | OS backend unavailable |
| `E0561` | error | Corrupt ASKANE-PAK bundle (unrecoverable) |
| `E0562` | error | Bundle read-only violation |
| `E0563` | error | Asset missing in hermetic bundle |
| `E0564` | error | Unsafe ASKANE-PAK entry path |
| `E0565` | error | Static asset path escapes sandbox |
| `W0101` | warning | Unused local variable |
| `W0102` | warning | Unreachable match arm |
| `W0501` | warning | Dynamic asset path cannot be verified statically |
| `W0502` | warning | Literal asset path missing on disk |

<!-- askane:diagnostics:end -->
