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
// Create a scraper (spawns background Servo thread)
ServoScraper *scraper_new(uint32_t width, uint32_t height,
                          uint64_t timeout, double wait, int fullpage);

// Take a screenshot, returns PNG bytes
// Caller must free with scraper_buffer_free()
int scraper_screenshot(ServoScraper *s, const char *url,
                       uint8_t **out_data, size_t *out_len);

// Capture HTML, returns null-terminated string
// Caller must free with scraper_string_free()
int scraper_html(ServoScraper *s, const char *url,
                 char **out_html, size_t *out_len);

// Cleanup
void scraper_free(ServoScraper *s);
void scraper_buffer_free(uint8_t *data, size_t len);
void scraper_string_free(char *s);
```

### Error Codes

| Code | Name | Value |
|---|---|---|
| `SCRAPER_OK` | Success | 0 |
| `SCRAPER_ERR_INIT` | Initialization failed | 1 |
| `SCRAPER_ERR_LOAD` | Page load failed | 2 |
| `SCRAPER_ERR_TIMEOUT` | Timeout | 3 |
| `SCRAPER_ERR_JS` | JavaScript error | 4 |
| `SCRAPER_ERR_SCREENSHOT` | Screenshot failed | 5 |
| `SCRAPER_ERR_CHANNEL` | Channel closed | 6 |
| `SCRAPER_ERR_NULL_PTR` | Null pointer | 7 |

### Minimal Example

```c
#include "servo_scraper.h"

ServoScraper *s = scraper_new(1280, 720, 30, 2.0, 0);

uint8_t *png; size_t png_len;
if (scraper_screenshot(s, "https://example.com", &png, &png_len) == SCRAPER_OK) {
    // write png to file...
    scraper_buffer_free(png, png_len);
}

scraper_free(s);
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
