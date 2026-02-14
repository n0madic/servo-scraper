# Go Example — servo-scraper FFI

Uses CGo to call the shared library (`libservo_scraper.dylib` / `.so`).

## Prerequisites

- Go 1.18+
- CGo enabled
- Build the shared library: `make build-lib`

## Setup

Initialize the Go module (from the `examples/go/` directory):

```bash
cd examples/go
go mod init servo-scraper-go-example
```

Or use the provided `go.mod` file.

## Usage

From the project root:

```bash
# macOS
CGO_ENABLED=1 DYLD_LIBRARY_PATH=target/release go run examples/go/scraper.go <URL> <screenshot.png> <output.html>

# Linux
CGO_ENABLED=1 LD_LIBRARY_PATH=target/release go run examples/go/scraper.go <URL> <screenshot.png> <output.html>

# Example
CGO_ENABLED=1 DYLD_LIBRARY_PATH=target/release go run examples/go/scraper.go https://example.com /tmp/shot.png /tmp/page.html
```

## How It Works

1. Uses CGo with `#cgo LDFLAGS` to link against `libservo_scraper.dylib`
2. Includes the C header via `#cgo CFLAGS: -I../c`
3. Calls `scraper_new()` to create a thread-safe scraper handle
4. Calls `scraper_screenshot()` / `scraper_html()` to capture data
5. Copies data from C using `C.GoBytes()` and `C.GoStringN()` before freeing
6. Frees buffers with `scraper_buffer_free()` / `scraper_string_free()`
7. Destroys the scraper with `scraper_free()` via defer

## API Quick Reference

```go
package main

/*
#cgo CFLAGS: -I../c
#cgo LDFLAGS: -L../../target/release -lservo_scraper
#include <stdlib.h>
#include "servo_scraper.h"
*/
import "C"
import "unsafe"

// Create scraper (width, height, timeout_sec, wait_sec, fullpage)
scraper := C.scraper_new(1280, 720, 30, 2.0, 0)
defer C.scraper_free(scraper)

// Screenshot → PNG bytes
var pngData *C.uint8_t
var pngLen C.size_t
cURL := C.CString("https://example.com")
defer C.free(unsafe.Pointer(cURL))

rc := C.scraper_screenshot(scraper, cURL, &pngData, &pngLen)
if rc == C.SCRAPER_OK {
    // Copy data before freeing
    pngBytes := C.GoBytes(unsafe.Pointer(pngData), C.int(pngLen))
    C.scraper_buffer_free(pngData, pngLen)
    // Use pngBytes...
}

// HTML → string
var htmlData *C.char
var htmlLen C.size_t
rc = C.scraper_html(scraper, cURL, &htmlData, &htmlLen)
if rc == C.SCRAPER_OK {
    htmlStr := C.GoStringN(htmlData, C.int(htmlLen))
    C.scraper_string_free(htmlData)
    // Use htmlStr...
}
```

## Error Codes

| Constant | Name | Value |
|---|---|---|
| `SCRAPER_OK` | Success | 0 |
| `SCRAPER_ERR_INIT` | Initialization failed | 1 |
| `SCRAPER_ERR_LOAD` | Page load failed | 2 |
| `SCRAPER_ERR_TIMEOUT` | Timeout | 3 |
| `SCRAPER_ERR_JS` | JavaScript error | 4 |
| `SCRAPER_ERR_SCREENSHOT` | Screenshot failed | 5 |
| `SCRAPER_ERR_CHANNEL` | Channel closed | 6 |
| `SCRAPER_ERR_NULL_PTR` | Null pointer | 7 |

## Important Notes

- **CGo must be enabled**: Set `CGO_ENABLED=1`
- **Library path**: Use `DYLD_LIBRARY_PATH` (macOS) or `LD_LIBRARY_PATH` (Linux) to point to `target/release/`
- **Memory safety**: Always copy data with `C.GoBytes()` or `C.GoStringN()` before freeing C memory
- **Cleanup**: Use `defer` to ensure proper cleanup of C resources
- **Thread safety**: The scraper handle is thread-safe and can be used from multiple goroutines

## Building a Binary

To build a standalone binary:

```bash
cd examples/go
CGO_ENABLED=1 go build -o scraper scraper.go

# Run with library path
DYLD_LIBRARY_PATH=../../target/release ./scraper https://example.com /tmp/shot.png /tmp/page.html
```

## Troubleshooting

### "library not found" error

Make sure the shared library exists:

```bash
make build-lib
ls -l target/release/libservo_scraper.{dylib,so}
```

### CGo linking errors

Ensure you're using the correct library path in the `#cgo LDFLAGS` directive. The path is relative to the source file location.

### Runtime library loading errors

Set the appropriate environment variable:

- macOS: `export DYLD_LIBRARY_PATH=target/release`
- Linux: `export LD_LIBRARY_PATH=target/release`
