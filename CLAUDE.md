# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

servo-scraper is a headless web scraper built on the Servo browser engine. It provides persistent page sessions with JavaScript evaluation, screenshots, HTML capture, input events, and wait mechanisms — a lightweight PhantomJS replacement. Available as a CLI tool and a Rust library with C FFI bindings consumed by Python, JavaScript (Node.js), and Go.

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
./target/release/servo-scraper --eval "document.title" https://example.com
./target/release/servo-scraper --wait-for "h1" --screenshot page.png https://example.com
```

## Architecture

The library is organized into four modules under `src/`:

```
src/
  lib.rs      Module declarations + re-exports
  types.rs    Shared public types (ScraperOptions, ConsoleMessage, NetworkRequest, ScraperError)
  engine.rs   PageEngine + all internal utilities (event loop, delegate, capture helpers)
  page.rs     Page (thread-safe wrapper) + Command enum
  ffi.rs      All extern "C" functions + error codes
```

Three architectural layers (dependency graph: `types ← engine ← page ← ffi`):

1. **PageEngine** (Layer 1, `engine.rs`) — Single-threaded, zero-overhead core. Not `Send`/`Sync`. Owns a persistent WebView for interactive use. Directly owns the Servo instance, event loop, and rendering context. The CLI (`src/main.rs`) uses this directly.

2. **Page** (Layer 2, `page.rs`) — Thread-safe wrapper (`Send + Sync`). Spawns a background thread running `PageEngine` and communicates via `mpsc` channels using a `Command` enum. Used by FFI consumers.

3. **C FFI** (Layer 3, `ffi.rs`) — `extern "C"` functions wrapping Layer 2. All functions prefixed with `page_`. Returns integer error codes (0 = OK, 1-9 = various errors).

### Public API (PageEngine / Page)

| Method | Description |
|---|---|
| `new(options)` | Initialize engine/page |
| `open(url)` | Navigate to URL (creates or reuses WebView) |
| `evaluate(script)` | Run JS, return result as JSON string |
| `screenshot()` | Viewport screenshot (PNG bytes) |
| `screenshot_fullpage()` | Full scrollable page screenshot |
| `html()` | Get page HTML |
| `url()` / `title()` | Get current URL / page title |
| `console_messages()` | Drain captured console messages |
| `network_requests()` | Drain captured network requests |
| `wait_for_selector(css, timeout)` | Wait for CSS selector to match |
| `wait_for_condition(js, timeout)` | Wait for JS expression to be truthy |
| `wait(seconds)` | Fixed wait with event loop alive |
| `wait_for_navigation(timeout)` | Wait for next page load |
| `click(x, y)` | Click at device coordinates |
| `click_selector(css)` | Click element by CSS selector |
| `type_text(text)` | Type text via key events |
| `key_press(name)` | Press a named key (Enter, Tab, etc.) |
| `mouse_move(x, y)` | Move mouse to coordinates |
| `close()` | Drop the WebView |

### Key Implementation Details

- **Persistent WebView** — WebView is created on first `open()` and reused for subsequent navigations via `WebView::load()`.
- **PageDelegate** captures console messages (`show_console_message`), network requests (`load_web_resource`), and auto-dismisses dialogs (`show_embedder_control`).
- **Servo runs headless** using `SoftwareRenderingContext` — no GPU or display server needed.
- **Resources are embedded** via `include_bytes!()` from `servo/resources/` — the binary is self-contained.
- **Stderr is suppressed** during Servo rendering via fd-level `dup2` to `/dev/null` (to hide macOS OpenGL noise).
- **Event loop** uses a condvar-based sleep/wake pattern with 5ms poll intervals.
- **Full-page screenshots** work by evaluating JS to get `scrollHeight`, then resizing the rendering context and viewport.
- **HTML capture** uses JS evaluation of `document.documentElement.outerHTML`.
- **Input events** use `WebView::notify_input_event()` with MouseButton/Keyboard/MouseMove events.
- **Event-driven frame waiting** — `PageDelegate` tracks a `frame_count: Cell<u64>` incremented by `notify_new_frame_ready`. Two helpers drive all waiting: `wait_for_frame(timeout)` blocks until at least one new frame is painted, and `wait_for_idle(idle_duration, max_timeout)` blocks until no new frames arrive for `idle_duration`. This replaces all arbitrary `spin_for`/`spin_briefly` delays (except the explicit `wait(seconds)` API). Input events, full-page screenshots, selector/condition polling, and post-load settling all use these frame-driven primitives.
- CLI argument parsing uses **bpaf** (derive mode).

### FFI Memory Contract

- `page_screenshot` / `page_screenshot_fullpage` return a heap-allocated `Box<[u8]>` — caller frees with `page_buffer_free(data, len)`.
- All string-returning functions (`page_html`, `page_evaluate`, `page_url`, `page_title`, `page_console_messages`, `page_network_requests`) return a `CString` — caller frees with `page_string_free(ptr)`.
- All FFI functions are NULL-safe and return `SCRAPER_ERR_NULL_PTR` (7) for null arguments.

### Error Codes

| Code | Name | Meaning |
|---|---|---|
| 0 | `SCRAPER_OK` | Success |
| 1 | `SCRAPER_ERR_INIT` | Initialization failed |
| 2 | `SCRAPER_ERR_LOAD` | Page load failed |
| 3 | `SCRAPER_ERR_TIMEOUT` | Operation timed out |
| 4 | `SCRAPER_ERR_JS` | JavaScript error |
| 5 | `SCRAPER_ERR_SCREENSHOT` | Screenshot failed |
| 6 | `SCRAPER_ERR_CHANNEL` | Internal channel closed |
| 7 | `SCRAPER_ERR_NULL_PTR` | NULL pointer argument |
| 8 | `SCRAPER_ERR_NO_PAGE` | No page open |
| 9 | `SCRAPER_ERR_SELECTOR` | CSS selector not found |

## Dependencies

- **Servo** is included as a git submodule at `./servo` and consumed via `libservo` (path dependency).
- **serde** + **serde_json** for JSON serialization (console messages, network requests, JS results).
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
