# C Example — servo-scraper FFI

Links against `libservo_scraper.dylib` (macOS) / `.so` (Linux) to take screenshots and capture HTML from C code.

## Files

| File | Description |
|---|---|
| `servo_scraper.h` | C header — opaque handle, error codes, function declarations |
| `test_scraper.c` | Test utility — screenshots a URL and captures HTML |

## Build

From the project root:

```bash
make test-c
```

Binary: `target/release/test_scraper`

## Usage

```bash
# macOS
DYLD_LIBRARY_PATH=target/release ./target/release/test_scraper <URL> <screenshot.png> <output.html>

# Linux
LD_LIBRARY_PATH=target/release ./target/release/test_scraper <URL> <screenshot.png> <output.html>
```

## API

### Functions

```c
// Create a page (spawns background Servo thread)
ServoPage *page_new(uint32_t width, uint32_t height,
                     uint64_t timeout, double wait, int fullpage,
                     const char *user_agent);

// Take a screenshot, returns PNG bytes
// Caller must free with page_buffer_free()
int page_screenshot(ServoPage *p, uint8_t **out_data, size_t *out_len);

// Capture HTML, returns null-terminated string
// Caller must free with page_string_free()
int page_html(ServoPage *p, char **out_html, size_t *out_len);

// Cleanup
void page_free(ServoPage *p);
void page_buffer_free(uint8_t *data, size_t len);
void page_string_free(char *s);
```

### Error Codes

| Code | Name | Value |
|---|---|---|
| `PAGE_OK` | Success | 0 |
| `PAGE_ERR_INIT` | Initialization failed | 1 |
| `PAGE_ERR_LOAD` | Page load failed | 2 |
| `PAGE_ERR_TIMEOUT` | Timeout | 3 |
| `PAGE_ERR_JS` | JavaScript error | 4 |
| `PAGE_ERR_SCREENSHOT` | Screenshot failed | 5 |
| `PAGE_ERR_CHANNEL` | Channel closed | 6 |
| `PAGE_ERR_NULL_PTR` | Null pointer | 7 |
| `PAGE_ERR_NO_PAGE` | No page open | 8 |
| `PAGE_ERR_SELECTOR` | CSS selector not found | 9 |

### Minimal Example

```c
#include "servo_scraper.h"

ServoPage *p = page_new(1280, 720, 30, 2.0, 0, NULL);
page_open(p, "https://example.com");

uint8_t *png; size_t png_len;
if (page_screenshot(p, &png, &png_len) == PAGE_OK) {
    // write png to file...
    page_buffer_free(png, png_len);
}

page_free(p);
```

## Linking (manual)

### Dynamic (shared library)

```bash
cc -o test_scraper test_scraper.c -Iexamples/c -Ltarget/release -lservo_scraper
```

Requires `DYLD_LIBRARY_PATH` (macOS) or `LD_LIBRARY_PATH` (Linux) at runtime.

### Static (self-contained binary)

```bash
# macOS
cc -o test_scraper test_scraper.c -Iexamples/c \
    target/release/libservo_scraper.a \
    -framework AppKit -framework CoreFoundation -framework CoreGraphics \
    -framework CoreText -framework IOSurface -framework Metal \
    -framework OpenGL -framework QuartzCore -framework Security \
    -framework SystemConfiguration \
    -lc++ -lresolv -lz -Wl,-no_fixup_chains
```

No runtime library path needed — all Servo code is embedded in the binary.
