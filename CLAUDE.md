# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

servo-scraper is a headless web scraper built on the Servo browser engine. It provides persistent page sessions with JavaScript evaluation, screenshots, HTML capture, input events, and wait mechanisms — a lightweight PhantomJS replacement. Available as a CLI tool and a Rust library with C FFI bindings consumed by Python, JavaScript (Node.js), and Go.

## Build Commands

```bash
make build          # Build everything (CLI binary + shared/static libraries)
make build-cli      # Build only the CLI binary
make build-lib      # Build only the library (rlib + cdylib + staticlib)
make clean          # Clean build artifacts + dist/
```

Servo is a regular crates.io dependency (`servo = "0.3.0"`). To update it, bump the version in `Cargo.toml` and run `cargo update -p servo`.

All builds use `cargo build --release`. There is no debug build target in the Makefile.

### Testing

```bash
cargo test          # Run integration tests (~60-90s)
make test           # Same thing via Makefile
```

The integration test suite (`tests/engine_integration.rs`) contains tests covering all public `PageEngine`/`Page` methods — both success and error paths. Most tests use a global `Page` singleton (Servo allows only one instance per process) with `data:text/html,...` URIs for fully self-contained, offline, deterministic operation. The native-cookies / custom-headers / resource-blocking / history-traversal tests spin up a local `tiny_http` server on an ephemeral `127.0.0.1` port (real round-trip through Servo's network stack).

Tests must run single-threaded — `.cargo/config.toml` sets `RUST_TEST_THREADS=1` automatically, so plain `cargo test` works.

### FFI Smoke Tests

```bash
make test-c         # Build C example against shared library
make test-python    # Verify Python ctypes can load FFI symbols
make test-js        # Verify Node.js koffi binding loads
make test-go        # Build Go example via CGo
```

FFI smoke tests verify the shared library loads and exports the expected symbols.

### Running the CLI

```bash
./target/release/servo-scraper --screenshot page.png https://example.com
./target/release/servo-scraper --html page.html --width 1920 --height 1080 https://example.com
./target/release/servo-scraper --fullpage --screenshot full.png --html page.html https://example.com
./target/release/servo-scraper --eval "document.title" https://example.com
./target/release/servo-scraper --eval-file script.js https://example.com
./target/release/servo-scraper --wait-for "h1" --screenshot page.png https://example.com
./target/release/servo-scraper --user-agent "MyBot/1.0" --eval "navigator.userAgent" https://example.com
./target/release/servo-scraper --wait-for-network-idle 500 --screenshot page.png https://example.com
./target/release/servo-scraper --block-urls ".png,.jpg,.gif,.svg" --screenshot page.png https://example.com
./target/release/servo-scraper --block-resource-type image --block-resource-type font --screenshot page.png https://example.com
./target/release/servo-scraper --header "Authorization: Bearer TOKEN" --eval "document.body.innerText" https://example.com
./target/release/servo-scraper --init-script inject.js --init-style theme.css --screenshot page.png https://example.com
./target/release/servo-scraper --temporary-storage --screenshot page.png https://example.com
```

## Architecture

The library is organized into four modules under `src/`:

```
src/
  lib.rs      Module declarations + re-exports
  types.rs    Shared public types (PageOptions, ConsoleMessage, NetworkRequest, PageError, ElementRect, InputFile)
  engine.rs   PageEngine + all internal utilities (event loop, delegate, capture helpers)
  page.rs     Page (thread-safe wrapper) + Command enum
  ffi.rs      All extern "C" functions + error codes
```

Three architectural layers (dependency graph: `types ← engine ← page ← ffi`):

1. **PageEngine** (Layer 1, `engine.rs`) — Single-threaded, zero-overhead core. Not `Send`/`Sync`. Manages multiple pages (WebViews) with an active-page model. Directly owns the Servo instance, event loop, and per-page rendering contexts. The CLI (`src/main.rs`) uses this directly.

2. **Page** (Layer 2, `page.rs`) — Thread-safe wrapper (`Send + Sync`). Spawns a background thread running `PageEngine` and communicates via `mpsc` channels using a `Command` enum. Used by FFI consumers.

3. **C FFI** (Layer 3, `ffi.rs`) — `extern "C"` functions wrapping Layer 2. All functions prefixed with `page_`. Returns integer error codes (0 = OK, 1-9 = various errors).

### Public API (PageEngine / Page)

| Method | Description |
|---|---|
| `new(options)` | Initialize engine/page (`PageOptions`: `user_agent`, `temporary_storage`, `headers`, `init_scripts`, `init_stylesheets`) |
| `open(url)` | Navigate to URL (creates or reuses WebView) |
| `evaluate(script)` | Run JS, return result as JSON string |
| `screenshot()` | Viewport screenshot (PNG bytes) |
| `screenshot_fullpage()` | Full scrollable page screenshot |
| `html()` | Get page HTML |
| `url()` / `title()` | Get current URL / page title |
| `console_messages()` | Drain captured console messages |
| `network_requests()` | Drain captured network requests |
| `get_cookies()` | Get cookies via Servo's native cookie store (includes `HttpOnly`) |
| `set_cookie(cookie)` | Set a cookie via `SiteDataManager::set_cookie_for_url` |
| `clear_cookies()` | Clear all cookies via `SiteDataManager::clear_cookies` (includes `HttpOnly`) |
| `block_urls(patterns)` | Block requests whose URL contains any pattern |
| `clear_blocked_urls()` | Clear all blocked URL patterns |
| `block_resource_types(types)` / `block_resource_type_names(names)` | Block requests by `Destination` (image/font/script/stylesheet/media/...) |
| `clear_blocked_resource_types()` | Clear all blocked resource-type destinations |
| `set_headers(headers)` | Set extra HTTP headers for subsequent navigations (`load_request`) |
| `add_init_script(js)` | Inject JS into every page (takes effect on next load) |
| `add_init_stylesheet(css)` | Inject a CSS user stylesheet into every page (next load) |
| `reload()` | Reload the current page |
| `go_back()` | Navigate back (returns `false` if no history) |
| `go_forward()` | Navigate forward (returns `false` if no forward history) |
| `element_rect(css)` | Get bounding rectangle of first matching element |
| `element_text(css)` | Get text content of first matching element |
| `element_attribute(css, attr)` | Get attribute value (`None` if attribute missing) |
| `element_html(css)` | Get outer HTML of first matching element |
| `wait_for_selector(css, timeout)` | Wait for CSS selector to match |
| `wait_for_condition(js, timeout)` | Wait for JS expression to be truthy |
| `wait(seconds)` | Fixed wait with event loop alive |
| `wait_for_navigation(timeout)` | Wait for next page load |
| `wait_for_network_idle(idle_ms, timeout)` | Wait until no new network requests for `idle_ms` ms |
| `click(x, y)` | Click at device coordinates |
| `click_selector(css)` | Click element by CSS selector |
| `type_text(text)` | Type text via key events |
| `key_press(name)` | Press a named key (Enter, Tab, etc.) |
| `mouse_move(x, y)` | Move mouse to coordinates |
| `scroll(delta_x, delta_y)` | Scroll viewport by pixel deltas (positive y = scroll down) |
| `scroll_to_selector(css)` | Scroll element into view via `scrollIntoView()` |
| `select_option(css, value)` | Select `<select>` option by value, fires change event |
| `set_input_files(css, files)` | Set files on `<input type="file">` via DataTransfer API |
| `close()` | Drop the active page's WebView |
| `reset()` | Drop all pages + clear blocked URLs, console messages, network requests |
| `new_page()` | Create a new page with default viewport, return its ID |
| `new_page_with_size(w, h)` | Create a new page with custom viewport size |
| `switch_to(page_id)` | Switch the active page |
| `close_page(page_id)` | Close a specific page by ID |
| `active_page_id()` | Get the active page's ID (`None` if no active page) |
| `page_ids()` | List all open page IDs (sorted) |
| `page_count()` | Number of open pages |
| `set_popup_handling(enabled)` | Enable/disable popup capture (`window.open`, `target="_blank"`) |
| `popup_pages()` | Drain pending popup pages, assign IDs, return them |
| `page_url(page_id)` | Get URL of a specific page by ID (without switching) |
| `page_title(page_id)` | Get title of a specific page by ID (without switching) |

### Key Implementation Details

- **Multi-page architecture** — `PageEngine` maintains a `HashMap<u32, PageState>` of pages, each with its own `WebView`, `SoftwareRenderingContext`, and `PageDelegate`. This provides per-page isolation of console messages, network requests, blocked URL patterns, viewport size, and screenshots. An active-page model means all existing methods (`evaluate`, `screenshot`, `html`, etc.) target the current active page. Auto-incrementing `u32` IDs identify pages (simple for FFI). `open()` with no pages auto-creates page 0 for backward compatibility.
- **Popup handling** — Opt-in via `set_popup_handling(true)`. When enabled, `WebViewDelegate::request_create_new` creates popup WebViews and buffers them. `popup_pages()` drains the buffer and assigns IDs. When disabled (default), popup requests are dropped (blocked).
- **Persistent WebView** — WebView is created on first `open()` and reused for subsequent navigations. When navigation headers are set the new/existing WebView is driven via `WebView::load_request(UrlRequest)`; otherwise `WebView::load(url)`.
- **PageDelegate** captures console messages (`show_console_message`), network requests (`load_web_resource`), blocks requests by URL substring (`blocked_url_patterns`) **and** by `Destination` (`blocked_destinations`) using `WebResourceLoad::intercept().cancel()`, auto-dismisses dialogs (`show_embedder_control`), and tracks history traversal completion (`notify_traversal_complete`). Blocked requests are cancelled and **omitted** from `network_requests()` (but still update `last_request_time` for idle detection).
- **User-Agent** is set via `ServoBuilder::preferences(Preferences { user_agent })` when `PageOptions.user_agent` is `Some`.
- **Cookies** use Servo's network-layer cookie store via `servo.site_data_manager()`: `get_cookies()` → `cookies_for_url(url, CookieSource::HTTP)` (includes `HttpOnly`), `set_cookie()` → `cookie::Cookie::parse` + `set_cookie_for_url(url, cookie, None)`, `clear_cookies()` → `clear_cookies(None)`. The `None`-callback paths are synchronous (resource-thread sync IPC, no event-loop spin). No more `document.cookie` JS hacks.
- **Navigation headers** — `PageOptions.headers` (and runtime `set_headers`) are stored on `PageEngine` and converted to an `http::HeaderMap` per navigation; applied via `UrlRequest::new(url).headers(map)` + `load_request`.
- **Resource-type blocking** — `block_resource_types`/`block_resource_type_names` set `Destination`s on the active `PageDelegate`. `parse_resource_types` maps user names (`image`→Image, `font`, `script`, `stylesheet`/`style`/`css`→Style, `media`→Audio+Video, `document`, `frame`/`iframe`, ...) from `content_security_policy::Destination`. `WebResourceRequest.destination` is matched in `load_web_resource`.
- **Init scripts / stylesheets** — a shared `Rc<UserContentManager>` (created in `PageEngine::new`, attached to every WebView in `open()` via `.user_content_manager(...)`) injects `PageOptions.init_scripts`/`init_stylesheets` and runtime `add_init_script`/`add_init_stylesheet`. Per Servo semantics, mutations take effect on the next load/reload. Stylesheet base URLs are synthesized (`https://servo-scraper.invalid/...`).
- **Temporary storage** — `PageOptions.temporary_storage` sets `ServoBuilder::opts(Opts { temporary_storage: true, .. })` for an in-memory, non-persistent session.
- **Element info** methods use JS `querySelector` + `getBoundingClientRect`/`textContent`/`getAttribute`/`outerHTML`.
- **Navigation** uses native `WebView::reload()`, `go_back(1)`, `go_forward(1)` with `can_go_back()`/`can_go_forward()` checks. `go_back`/`go_forward` wait on `notify_traversal_complete` **or** `LoadStatus::Complete` (the traversal signal fixes hangs on `data:` URIs, where Servo does not re-fire `Complete` for history navigation).
- **Servo runs headless** using `SoftwareRenderingContext` — no GPU or display server needed.
- **Resources are embedded** via Servo's `baked-in-resources` feature (the `servo-default-resources` crate) — the binary is self-contained, no external resource directory needed.
- **Stderr is suppressed** during Servo rendering via fd-level `dup2` to `/dev/null` (to hide macOS OpenGL noise).
- **Event loop** uses a condvar-based sleep/wake pattern with 5ms poll intervals.
- **Full-page screenshots** work by evaluating JS to get `scrollHeight`, then resizing the rendering context and viewport.
- **HTML capture** uses JS evaluation of `document.documentElement.outerHTML`.
- **Input events** use `WebView::notify_input_event()` with MouseButton/Keyboard/MouseMove/Wheel events.
- **Scroll** uses native `WheelEvent` with negated deltas (Servo's convention: positive = scroll up; our API: positive = scroll down). `scroll_to_selector` uses JS `scrollIntoView()`.
- **Select** uses JS to set `<select>.value` and dispatch `input`+`change` events.
- **File upload** uses JS DataTransfer API with base64-encoded file data to set `input.files` and dispatch `change` event. Depends on the `base64` crate.
- **Event-driven frame waiting** — `PageDelegate` tracks a `frame_count: Cell<u64>` incremented by `notify_new_frame_ready`. Two helpers drive all waiting: `wait_for_frame(timeout)` blocks until at least one new frame is painted, and `wait_for_idle(idle_duration, max_timeout)` blocks until no new frames arrive for `idle_duration`. This replaces all arbitrary `spin_for`/`spin_briefly` delays (except the explicit `wait(seconds)` API). Input events, full-page screenshots, selector/condition polling, and post-load settling all use these frame-driven primitives.
- **Network idle detection** — `PageDelegate` tracks `last_request_time: Cell<Option<Instant>>`, updated in `load_web_resource()` on every request start. `wait_for_network_idle(idle_ms, timeout)` polls this timestamp and returns when no new requests have started for `idle_ms` milliseconds. Since Servo's `WebViewDelegate` only fires at request **start** (no completion callback), this detects when the request cascade has settled — the same semantic used by Puppeteer/Playwright's "networkidle".
- CLI argument parsing uses **bpaf** (derive mode).

### FFI Memory Contract

- `page_screenshot` / `page_screenshot_fullpage` return a heap-allocated `Box<[u8]>` — caller frees with `page_buffer_free(data, len)`.
- All string-returning functions (`page_html`, `page_evaluate`, `page_url`, `page_title`, `page_console_messages`, `page_network_requests`, `page_get_cookies`, `page_element_rect`, `page_element_text`, `page_element_attribute`, `page_element_html`, `page_page_ids`, `page_popup_pages`, `page_page_url`, `page_page_title`) return a `CString` — caller frees with `page_string_free(ptr)`.
- `page_new` takes 7 parameters: `width, height, timeout, wait, fullpage, user_agent` (`*const c_char`, NULL = default), `temporary_storage` (`int`, non-zero = in-memory session).
- Runtime configuration FFI (apply before `page_open`): `page_set_headers(page, "Name: Value\nName2: Value2")` (NULL clears), `page_block_resource_types(page, "image,font")` (NULL clears), `page_add_init_script(page, js)`, `page_add_init_stylesheet(page, css)`.
- All FFI functions are NULL-safe and return `PAGE_ERR_NULL_PTR` (7) for null arguments.

### Error Codes

| Code | Name | Meaning |
|---|---|---|
| 0 | `PAGE_OK` | Success |
| 1 | `PAGE_ERR_INIT` | Initialization failed |
| 2 | `PAGE_ERR_LOAD` | Page load failed |
| 3 | `PAGE_ERR_TIMEOUT` | Operation timed out |
| 4 | `PAGE_ERR_JS` | JavaScript error |
| 5 | `PAGE_ERR_SCREENSHOT` | Screenshot failed |
| 6 | `PAGE_ERR_CHANNEL` | Internal channel closed |
| 7 | `PAGE_ERR_NULL_PTR` | NULL pointer argument |
| 8 | `PAGE_ERR_NO_PAGE` | No page open |
| 9 | `PAGE_ERR_SELECTOR` | CSS selector not found |

## Dependencies

- **Servo** is a crates.io dependency (`servo = "0.3.0"`, features `baked-in-resources` + `js_jit`, `default-features = false`). Servo publishes monthly releases plus an LTS line; bump the version in `Cargo.toml` to update.
- **serde** + **serde_json** for JSON serialization (console messages, network requests, JS results).
- **base64** for encoding file data in `set_input_files()`.
- **cookie** (`0.18`) for parsing/constructing `Cookie<'static>` for `set_cookie_for_url` (not re-exported from servo).
- **http** (`1.4`) for `http::HeaderMap` used by `UrlRequest::headers()` (version must match Servo's).
- **content-security-policy** (`0.8.0`, `serde` feature) for the `Destination` enum used in resource-type blocking.
- **tiny_http** (dev-dependency) for the local HTTP server in cookie/header/blocking/traversal integration tests.
- Requires Rust 1.88+ (edition 2024).
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
- Windows: stderr suppression uses MSVC CRT functions (`_dup`, `_dup2`, `_open("NUL", ...)`) via `#[cfg(windows)]` — separate from the Unix `libc::dup`/`/dev/null` path. Servo itself may have additional platform limitations.

## Cross-Compilation & Releases

Local cross-compilation script (host: macOS ARM64). No CI/CD — builds run locally and publish to GitHub Releases via `gh`.

### Target Platforms

| Platform | Method | Shared lib ext |
|---|---|---|
| macOS ARM64 | Native `cargo build --release` | `.dylib` |
| macOS x86_64 | `cargo build --release --target x86_64-apple-darwin` | `.dylib` |
| Linux x86_64 | `cross build --release --target x86_64-unknown-linux-gnu` | `.so` |

### Release Commands

```bash
make release-macos-arm64     # Build + package macOS ARM64
make release-macos-x86_64    # Build + package macOS x86_64
make release-linux-x86_64    # Build + package Linux x86_64 (requires Docker)
make release-all             # Build + package all platforms
make release VERSION=0.1.0   # Build all + git tag + push + gh release create
```

VERSION defaults to the version in `Cargo.toml` if not specified.

Each `release-*` target builds the binary + libraries, then packages into `dist/servo-scraper-{VERSION}-{PLATFORM}.tar.gz` containing: CLI binary, shared library, static library, C header (`servo_scraper.h`), and `README.md`.

### Prerequisites (one-time setup)

```bash
rustup target add x86_64-apple-darwin
cargo install cross --git https://github.com/cross-rs/cross
brew install gh && gh auth login
# Docker must be running for Linux builds
```

### Configuration Files

- **`Cross.toml`** — Configuration for `cross` (Linux cross-compilation via Docker). Points to a custom Dockerfile.
- **`cross/Dockerfile.x86_64-unknown-linux-gnu`** — Custom Docker image extending `cross`'s base with Servo build dependencies (`python3`, `cmake`, `clang`, `libclang-dev`, `pkg-config`, font/glib/ssl/dbus dev libraries).
- Release archives are output to `dist/` (gitignored).
