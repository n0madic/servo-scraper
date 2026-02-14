# servo-scraper

A minimal headless web scraper built on the [Servo](https://servo.org/) browser engine. Captures screenshots and/or HTML content from web pages with full JavaScript execution.

Available as a **CLI tool** and a **library** with FFI bindings for C, Python, JavaScript, and Go.

## Features

- Full-page or viewport-only screenshots (PNG, JPG, BMP)
- HTML capture via JS evaluation (`document.documentElement.outerHTML`)
- Configurable viewport size, load timeout, and post-load JS settle time
- Software rendering — no GPU or display server required
- C FFI with shared (`.dylib`/`.so`) and static (`.a`) libraries
- Thread-safe — Servo runs on a dedicated background thread

## Prerequisites

- Rust 1.86+
- System dependencies required by Servo (see [Servo build instructions](https://book.servo.org/building/building.html))

## Build

```bash
git clone --recurse-submodules https://github.com/n0madic/servo-scraper.git
cd servo-scraper
make
```

### Build Targets

| Target | Command | Output |
|---|---|---|
| All (CLI + lib) | `make build` | binary + `.dylib` + `.a` |
| CLI only | `make build-cli` | `target/release/servo-scraper` |
| Library only | `make build-lib` | `.dylib` + `.a` |
| C example | `make test-c` | `target/release/test_scraper` |
| Python smoke test | `make test-python` | verifies FFI symbols |
| JS smoke test | `make test-js` | verifies koffi binding |
| Go example | `make test-go` | `target/release/go_scraper` |

### Build Artifacts

```
target/release/
  servo-scraper              # CLI binary
  libservo_scraper.dylib     # shared library (macOS) / .so (Linux)
  libservo_scraper.a         # static library
```

## CLI Usage

```bash
# Screenshot
servo-scraper --screenshot page.png https://example.com

# Full-page screenshot
servo-scraper --fullpage --screenshot page.png https://example.com

# HTML capture
servo-scraper --html page.html https://example.com

# Both + custom viewport
servo-scraper --screenshot page.png --html page.html --width 1920 --height 1080 https://example.com
```

### Options

| Option | Description | Default |
|---|---|---|
| `--screenshot <PATH>` | Save screenshot (png, jpg, bmp) | — |
| `--html <PATH>` | Save page HTML | — |
| `--fullpage` | Capture full scrollable page | off |
| `--width <PX>` | Viewport width | 1280 |
| `--height <PX>` | Viewport height | 720 |
| `--timeout <SEC>` | Max page load wait | 30 |
| `--wait <SEC>` | Post-load JS settle time | 2.0 |

## FFI Examples

Working examples for each language are in the `examples/` directory:

| Language | Directory | Description |
|---|---|---|
| **C** | [`examples/c/`](examples/c/) | Static linking with `libservo_scraper.a` |
| **Python** | [`examples/python/`](examples/python/) | ctypes + shared library |
| **JavaScript** | [`examples/js/`](examples/js/) | Node.js + koffi + shared library |
| **Go** | [`examples/go/`](examples/go/) | CGo + shared library |

Each directory has its own README with setup instructions and API reference.

## Architecture

```
src/
  lib.rs    — ScraperEngine (core) + Scraper (thread-safe) + C FFI
  main.rs   — Thin CLI: bpaf args → ScraperEngine → file output
examples/
  c/        — C header + test utility (static linking)
  python/   — ctypes example
  js/       — Node.js koffi example
  go/       — CGo example
```

The library has three layers:

1. **ScraperEngine** — single-threaded, zero-overhead core. CLI uses this directly.
2. **Scraper** — thread-safe Rust wrapper (`Send + Sync`). Spawns a background Servo thread.
3. **C FFI** — `extern "C"` functions wrapping Layer 2. Used by Python, JS, C, etc.

## Updating Servo

```bash
make update-servo
git commit -m "Update servo submodule"
```

## License

MPL-2.0
