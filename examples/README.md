# Askane Examples

Runnable scripts and mini-projects demonstrating Askane syntax, capability flags, 2D graphics, networking, and procedural media generation.

## Quick Links (Open in 1 Click)

| Example | Topic | Permissions Needed | How to Run |
| --- | --- | --- | --- |
| [hello.ask](hello.ask) | Expressions, `match`, functions, pipelines | Default sandbox | `askane examples/hello.ask` |
| [pipeline.ask](pipeline.ask) | Pipelines `\|>`, `map`, `filter`, `reduce` | Default sandbox | `askane examples/pipeline.ask` |
| [closures.ask](closures.ask) | Lexical closures and state | Default sandbox | `askane examples/closures.ask` |
| [calculator.ask](calculator.ask) | Interactive CLI calculator (`try`/`catch`, `match`) | Default sandbox | `askane examples/calculator.ask` |
| [test_script.ask](test_script.ask) | `?.`, `??`, guards, lambdas, `json`, `math` | Default sandbox | `askane examples/test_script.ask` |
| [pong.ask](pong.ask) | Classic 2D Pong in `gfx` (Windows) | `--max-steps=500000000` (or `--unsafe`) | `askane examples/pong.ask --max-steps=500000000` |
| [neon_dodge/](neon_dodge/main.ask) | Multi-file 2D game project (`askane.toml`, `gfx`, stats save) | Sandbox limits defined in [askane.toml](neon_dodge/askane.toml) | `askane run examples/neon_dodge` |
| [logo_gen.ask](logo_gen.ask) | Procedural PNG & SVG generator (`bytes`, `compress`, `fs`) | Default sandbox (writes in script dir) | `askane examples/logo_gen.ask` |
| [modules_demo.ask](modules_demo.ask) | Stdlib + relative module imports + `mem` demo | `--unsafe` + `ASKANE_ENABLE_UNSAFE_MEM=1` | `ASKANE_ENABLE_UNSAFE_MEM=1 askane --unsafe examples/modules_demo.ask` |
| [math_helper.ask](math_helper.ask) | Helper module imported by [modules_demo.ask](modules_demo.ask) | Imported automatically | *(library module)* |
| [api_demo.ask](api_demo.ask) | HTTPS GET + `json.parse` (Windows WinHTTP) | `--allow-net=httpbin.org` or `--unsafe` | `askane --allow-net=httpbin.org examples/api_demo.ask` |

You can also open any example visually in **Askane Studio v2** on Windows:

```bash
askane --gui examples/neon_dodge/main.ask
```
