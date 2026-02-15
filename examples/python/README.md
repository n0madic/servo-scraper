# Python Example — servo-scraper FFI

Uses `ctypes` to call the shared library (`libservo_scraper.dylib` / `.so`).

## Prerequisites

- Python 3.8+
- Build the shared library: `make build-lib`

## Usage

```bash
python3 examples/python/scraper.py <URL> <screenshot.png> <output.html>

# Example
python3 examples/python/scraper.py https://example.com /tmp/shot.png /tmp/page.html
```

## How It Works

1. Loads `target/release/libservo_scraper.dylib` via `ctypes.CDLL`
2. Sets up function signatures (argtypes/restype) for type safety
3. Calls `page_new()` to create a thread-safe page handle
4. Calls `page_open()` to navigate, then `page_screenshot()` / `page_html()` to capture data
5. Frees buffers with `page_buffer_free()` / `page_string_free()`
6. Destroys the page with `page_free()`

## API Quick Reference

```python
import ctypes

lib = ctypes.CDLL("target/release/libservo_scraper.dylib")

# Create page (width, height, timeout_sec, wait_sec, fullpage, user_agent)
page = lib.page_new(1280, 720, 30, 2.0, 0, None)

# Open URL
lib.page_open(page, b"https://example.com")

# Screenshot → PNG bytes
png_data = ctypes.POINTER(ctypes.c_uint8)()
png_len = ctypes.c_size_t(0)
rc = lib.page_screenshot(page, ctypes.byref(png_data), ctypes.byref(png_len))
if rc == 0:  # PAGE_OK
    data = bytes(png_data[:png_len.value])
    lib.page_buffer_free(png_data, png_len)

# HTML → string
html_ptr = ctypes.c_char_p()
html_len = ctypes.c_size_t(0)
rc = lib.page_html(page, ctypes.byref(html_ptr), ctypes.byref(html_len))
if rc == 0:
    html = html_ptr.value[:html_len.value].decode("utf-8")
    lib.page_string_free(html_ptr)

lib.page_free(page)
```
