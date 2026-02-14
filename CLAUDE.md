# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

servo-scraper is a headless web scraper built on the Servo browser engine. It captures screenshots (PNG) and HTML content from web pages with full JavaScript execution. Available as a CLI tool and a Rust library with C FFI bindings consumed by Python, JavaScript (Node.js), and Go.

## Build Commands

```bash
make build          # Build everything (CLI binary + shared/static libraries)
make build-cli      # Build only the CLI binary
make build-lib      # Build only the library (rlib + cdylib + staticlib)
make clean          # Clean build artifacts
make update-servo   # Update Servo submodule to latest main
```

All builds use `cargo build --release`. There is no debug build target in the Makefile.

### Testing / Smoke Tests

```bash
make test-c         # Build C example against shared library
make test-python    # Verify Python ctypes can load FFI symbols
make test-js        # Verify Node.js koffi binding loads
make test-go        # Build Go example via CGo
```

There are no unit tests or integration test suites — the "tests" are FFI smoke tests that verify the shared library loads and exports the expected symbols.

### Running the CLI

```bash
./target/release/servo-scraper --screenshot page.png https://example.com
./target/release/servo-scraper --html page.html --width 1920 --height 1080 https://example.com
./target/release/servo-scraper --fullpage --screenshot full.png --html page.html https://example.com
```

## Architecture

The entire library is in `src/lib.rs` with three layers:

1. **ScraperEngine** (Layer 1) — Single-threaded, zero-overhead core. Not `Send`/`Sync`. Directly owns the Servo instance, event loop, and rendering context. The CLI (`src/main.rs`) uses this directly.

2. **Scraper** (Layer 2) — Thread-safe wrapper (`Send + Sync`). Spawns a background thread running `ScraperEngine` and communicates via `mpsc` channels using a `Command` enum. Used by FFI consumers.

3. **C FFI** (Layer 3) — `extern "C"` functions wrapping Layer 2. Exposes: `scraper_new`, `scraper_free`, `scraper_screenshot`, `scraper_html`, `scraper_buffer_free`, `scraper_string_free`. Returns integer error codes (0 = OK, 1-7 = various errors).

### Key Implementation Details

- **Servo runs headless** using `SoftwareRenderingContext` — no GPU or display server needed.
- **Resources are embedded** via `include_bytes!()` from `servo/resources/` — the binary is self-contained.
- **Stderr is suppressed** during Servo rendering via fd-level `dup2` to `/dev/null` (to hide macOS OpenGL noise).
- **Event loop** uses a condvar-based sleep/wake pattern with 5ms poll intervals.
- **Full-page screenshots** work by evaluating JS to get `scrollHeight`, then resizing the rendering context and viewport.
- **HTML capture** uses JS evaluation of `document.documentElement.outerHTML`.
- CLI argument parsing uses **bpaf** (derive mode).

### FFI Memory Contract

- `scraper_screenshot` returns a heap-allocated `Box<[u8]>` — caller frees with `scraper_buffer_free(data, len)`.
- `scraper_html` returns a `CString` — caller frees with `scraper_string_free(ptr)`.
- All FFI functions are NULL-safe and return `SCRAPER_ERR_NULL_PTR` (7) for null arguments.

## Dependencies

- **Servo** is included as a git submodule at `./servo` and consumed via `libservo` (path dependency).
- Requires Rust 1.86+ (edition 2024).
- Release profile: LTO enabled, single codegen unit, `opt-level = "z"`, stripped, `panic = "abort"`.

## FFI Examples

- `examples/c/` — C header (`servo_scraper.h`) + test binary. Links against `libservo_scraper.dylib`.
- `examples/python/` — ctypes wrapper loading the `.dylib`/`.so`.
- `examples/js/` — Node.js using `koffi` for FFI. Requires `npm install` in `examples/js/`.
- `examples/go/` — CGo with `#cgo LDFLAGS` pointing to `target/release`.

## Platform Notes

- macOS: shared library is `.dylib`, runtime needs `DYLD_LIBRARY_PATH=target/release` for FFI examples.
- Linux: shared library is `.so`, runtime needs `LD_LIBRARY_PATH=target/release`.
- The `test-python` and `test-js` Makefile targets hardcode `.dylib` (macOS-only).
