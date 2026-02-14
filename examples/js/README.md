# JavaScript Example — servo-scraper FFI

Uses [koffi](https://koffi.dev/) to call the shared library (`libservo_scraper.dylib` / `.so`) from Node.js.

## Prerequisites

- Node.js 18+
- Build the shared library: `make build-lib`

## Setup

```bash
cd examples/js
npm install
```

## Usage

```bash
node examples/js/scraper.mjs <URL> <screenshot.png> <output.html>

# Example
node examples/js/scraper.mjs https://example.com /tmp/shot.png /tmp/page.html
```

Or from the `examples/js/` directory:

```bash
npm run scrape -- https://example.com /tmp/shot.png /tmp/page.html
```

## How It Works

1. Loads `target/release/libservo_scraper.dylib` via koffi
2. Defines function signatures matching the C header
3. Calls `scraper_new()` to create a thread-safe scraper handle
4. Calls `scraper_screenshot()` / `scraper_html()` to capture data
5. Decodes returned buffers with `koffi.decode()`
6. Frees memory with `scraper_buffer_free()` / `scraper_string_free()`
7. Destroys the scraper with `scraper_free()`

## API Quick Reference

```javascript
import koffi from "koffi";

const lib = koffi.load("target/release/libservo_scraper.dylib");

const scraper_new = lib.func(
  "void *scraper_new(uint32_t, uint32_t, uint64_t, double, int)",
);
const scraper_screenshot = lib.func(
  "int scraper_screenshot(void*, const char*, _Out_ uint8_t**, _Out_ size_t*)",
);
const scraper_html = lib.func(
  "int scraper_html(void*, const char*, _Out_ void**, _Out_ size_t*)",
);
const scraper_buffer_free = lib.func("void scraper_buffer_free(void*, size_t)");
const scraper_string_free = lib.func("void scraper_string_free(void*)");
const scraper_free = lib.func("void scraper_free(void*)");

const s = scraper_new(1280, 720, 30, 2.0, 0);
// ... use scraper_screenshot / scraper_html ...
scraper_free(s);
```

## Why koffi?

[koffi](https://koffi.dev/) is a fast, zero-dependency Node.js FFI library that supports:
- Direct C function calls without native addons
- Pointer output parameters (`_Out_`)
- Buffer decoding from raw pointers
- Works with Node.js 18+
