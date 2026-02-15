# servo-scraper

A headless web scraper built on the [Servo](https://servo.org/) browser engine. Provides persistent page sessions with JavaScript evaluation, screenshots, HTML capture, input events, and wait mechanisms — a lightweight PhantomJS replacement.

Available as a **CLI tool** and a **library** with FFI bindings for C, Python, JavaScript, and Go.

## Features

- **Persistent page sessions** — open a page, interact with it, capture results
- **JavaScript evaluation** — run JS and get results as JSON
- **Screenshots** — full-page or viewport-only (PNG, JPG, BMP)
- **HTML capture** — via JS evaluation (`document.documentElement.outerHTML`)
- **Wait mechanisms** — wait for CSS selectors, JS conditions, navigation, or fixed time
- **Input events** — click (coordinates or CSS selector), type text, press keys, mouse move
- **Cookies** — get, set, and clear cookies via `document.cookie`
- **Request interception** — block URLs matching patterns (images, trackers, etc.)
- **Navigation** — reload, go back, go forward in history
- **Element info** — get bounding rect, text content, attributes, and HTML of elements
- **Custom User-Agent** — set via `PageOptions` or `--user-agent` CLI flag
- **Console capture** — collect `console.log/warn/error` messages
- **Network monitoring** — observe HTTP requests made during page load
- **Dialog auto-dismiss** — alert/confirm/prompt dialogs are automatically handled
- Configurable viewport size, load timeout, and post-load JS settle time
- Software rendering — no GPU or display server required
- C FFI with shared (`.dylib`/`.so`) and static (`.a`) libraries
- Thread-safe — Servo runs on a dedicated background thread

## Prerequisites

- Rust 1.86+
- System dependencies required by Servo (see [Servo build instructions](https://book.servo.org/building/building.html))

## Build

```bash
git clone --recurse-submodules https://github.com/user/servo-scraper.git
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

# Evaluate JavaScript (result printed to stdout as JSON)
servo-scraper --eval "document.title" https://example.com

# Evaluate JavaScript from a file
servo-scraper --eval-file script.js https://example.com

# Wait for a CSS selector before capturing
servo-scraper --wait-for "h1" --screenshot page.png https://example.com

# Custom User-Agent
servo-scraper --user-agent "MyBot/1.0" --eval "navigator.userAgent" https://example.com

# Block images and tracking pixels
servo-scraper --block-urls ".png,.jpg,.gif,.svg,.tracker" --screenshot page.png https://example.com

# Combined
servo-scraper --eval "document.title" --screenshot page.png --html page.html --width 1920 --height 1080 https://example.com
```

### Options

| Option | Description | Default |
|---|---|---|
| `--screenshot <PATH>` | Save screenshot (png, jpg, bmp) | — |
| `--html <PATH>` | Save page HTML | — |
| `--eval <JS>` | Evaluate JS, print JSON result to stdout | — |
| `--eval-file <PATH>` | Evaluate JS from a file, print JSON result to stdout | — |
| `--wait-for <SELECTOR>` | Wait for CSS selector before capturing | — |
| `--fullpage` | Capture full scrollable page | off |
| `--user-agent <STRING>` | Custom User-Agent string | Servo default |
| `--block-urls <PATTERNS>` | Comma-separated URL patterns to block | — |
| `--width <PX>` | Viewport width | 1280 |
| `--height <PX>` | Viewport height | 720 |
| `--timeout <SEC>` | Max page load wait | 30 |
| `--wait <SEC>` | Post-load JS settle time | 2.0 |

## Rust API

```rust
use servo_scraper::{PageEngine, PageOptions};

// Layer 1: Single-threaded (for CLI / direct use)
let options = PageOptions {
    user_agent: Some("MyBot/1.0".into()),
    ..PageOptions::default()
};
let mut engine = PageEngine::new(options).unwrap();

// Block tracking/ad resources
engine.block_urls(vec![".tracker".into(), "ads.".into()]);

engine.open("https://example.com").unwrap();
let title = engine.evaluate("document.title").unwrap();  // JSON string
let html = engine.html().unwrap();
let png = engine.screenshot().unwrap();

// Cookies
let cookies = engine.get_cookies().unwrap();
engine.set_cookie("name=value; path=/").unwrap();

// Navigation
engine.reload().unwrap();
engine.go_back();   // Ok(false) if no history
engine.go_forward(); // Ok(false) if no forward history

// Element info
let rect = engine.element_rect("h1").unwrap();
let text = engine.element_text("h1").unwrap();
let href = engine.element_attribute("a", "href").unwrap();
let el_html = engine.element_html("h1").unwrap();

// Wait for element, then click it
engine.wait_for_selector("button#submit", 10).unwrap();
engine.click_selector("button#submit").unwrap();

// Type into a field
engine.click_selector("input[name=search]").unwrap();
engine.type_text("hello world").unwrap();
engine.key_press("Enter").unwrap();
```

```rust
use servo_scraper::{Page, PageOptions};

// Layer 2: Thread-safe (for FFI / multi-threaded use)
let page = Page::new(PageOptions::default()).unwrap();
page.open("https://example.com").unwrap();
let png = page.screenshot().unwrap();
```

## C FFI API

All functions are prefixed with `page_`. See [`examples/c/servo_scraper.h`](examples/c/servo_scraper.h) for the full header.

```c
// Lifecycle
ServoPage *page_new(width, height, timeout, wait, fullpage, user_agent);
void       page_free(ServoPage *page);

// Navigation
int page_open(page, url);
int page_reload(page);
int page_go_back(page);
int page_go_forward(page);

// Capture
int page_evaluate(page, script, &out_json, &out_len);
int page_screenshot(page, &out_data, &out_len);
int page_screenshot_fullpage(page, &out_data, &out_len);
int page_html(page, &out_html, &out_len);

// Page info
int page_url(page, &out_url, &out_len);
int page_title(page, &out_title, &out_len);

// Cookies
int page_get_cookies(page, &out_cookies, &out_len);
int page_set_cookie(page, cookie);
int page_clear_cookies(page);

// Request interception
int page_block_urls(page, patterns);  // comma-separated, NULL = clear

// Element info
int page_element_rect(page, selector, &out_json, &out_len);
int page_element_text(page, selector, &out_text, &out_len);
int page_element_attribute(page, selector, attribute, &out_value, &out_len);
int page_element_html(page, selector, &out_html, &out_len);

// Events (JSON arrays)
int page_console_messages(page, &out_json, &out_len);
int page_network_requests(page, &out_json, &out_len);

// Wait
int page_wait_for_selector(page, selector, timeout_secs);
int page_wait_for_condition(page, js_expr, timeout_secs);
int page_wait(page, seconds);
int page_wait_for_navigation(page, timeout_secs);

// Input
int page_click(page, x, y);
int page_click_selector(page, selector);
int page_type_text(page, text);
int page_key_press(page, key_name);
int page_mouse_move(page, x, y);

// Memory
void page_buffer_free(data, len);
void page_string_free(s);
```

## FFI Examples

Working examples for each language are in the `examples/` directory:

| Language | Directory | Description |
|---|---|---|
| **C** | [`examples/c/`](examples/c/) | Dynamic linking with `libservo_scraper.dylib` |
| **Python** | [`examples/python/`](examples/python/) | ctypes + shared library |
| **JavaScript** | [`examples/js/`](examples/js/) | Node.js + koffi + shared library |
| **Go** | [`examples/go/`](examples/go/) | CGo + shared library |

## Architecture

```
src/
  lib.rs    — PageEngine (core) + Page (thread-safe) + C FFI
  main.rs   — CLI: bpaf args → PageEngine → file/stdout output
examples/
  c/        — C header + test utility
  python/   — ctypes example
  js/       — Node.js koffi example
  go/       — CGo example
```

The library has three layers:

1. **PageEngine** — single-threaded, zero-overhead core. CLI uses this directly. Owns a persistent WebView for interactive use.
2. **Page** — thread-safe Rust wrapper (`Send + Sync`). Spawns a background Servo thread, communicates via channels.
3. **C FFI** — `extern "C"` functions wrapping Layer 2. Used by Python, JS, C, Go, etc.

## Updating Servo

```bash
make update-servo
git commit -m "Update servo submodule"
```

## License

MPL-2.0
