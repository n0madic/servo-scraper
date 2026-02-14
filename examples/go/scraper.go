// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// servo-scraper Go FFI example.
//
// Uses CGo to call the shared library (libservo_scraper.dylib / .so).
//
// Build and run:
//
//	CGO_ENABLED=1 go run examples/go/scraper.go https://example.com /tmp/shot.png /tmp/page.html
//
// Or on macOS with explicit library path:
//
//	CGO_ENABLED=1 DYLD_LIBRARY_PATH=target/release go run examples/go/scraper.go https://example.com /tmp/shot.png /tmp/page.html
//
// Or on Linux:
//
//	CGO_ENABLED=1 LD_LIBRARY_PATH=target/release go run examples/go/scraper.go https://example.com /tmp/shot.png /tmp/page.html
//
// Requires: make build-lib (to produce the shared library)

package main

/*
#cgo CFLAGS: -I../c
#cgo LDFLAGS: -L../../target/release -lservo_scraper
#include <stdlib.h>
#include "servo_scraper.h"
*/
import "C"
import (
	"fmt"
	"os"
	"unsafe"
)

// Error codes matching servo_scraper.h
const (
	scraperOK            = C.SCRAPER_OK
	scraperErrInit       = C.SCRAPER_ERR_INIT
	scraperErrLoad       = C.SCRAPER_ERR_LOAD
	scraperErrTimeout    = C.SCRAPER_ERR_TIMEOUT
	scraperErrJS         = C.SCRAPER_ERR_JS
	scraperErrScreenshot = C.SCRAPER_ERR_SCREENSHOT
	scraperErrChannel    = C.SCRAPER_ERR_CHANNEL
	scraperErrNullPtr    = C.SCRAPER_ERR_NULL_PTR
	scraperErrNoPage     = C.SCRAPER_ERR_NO_PAGE
	scraperErrSelector   = C.SCRAPER_ERR_SELECTOR
)

// errorName returns a human-readable name for error codes
func errorName(code C.int) string {
	switch code {
	case scraperOK:
		return "OK"
	case scraperErrInit:
		return "INIT_FAILED"
	case scraperErrLoad:
		return "LOAD_FAILED"
	case scraperErrTimeout:
		return "TIMEOUT"
	case scraperErrJS:
		return "JS_ERROR"
	case scraperErrScreenshot:
		return "SCREENSHOT_FAILED"
	case scraperErrChannel:
		return "CHANNEL_CLOSED"
	case scraperErrNullPtr:
		return "NULL_POINTER"
	case scraperErrNoPage:
		return "NO_PAGE"
	case scraperErrSelector:
		return "SELECTOR_NOT_FOUND"
	default:
		return "UNKNOWN"
	}
}

func main() {
	if len(os.Args) < 4 {
		fmt.Fprintf(os.Stderr,
			"Usage: %s <URL> <screenshot.png> <output.html>\n\n"+
				"Example:\n"+
				"  %s https://example.com /tmp/shot.png /tmp/page.html\n",
			os.Args[0], os.Args[0])
		os.Exit(1)
	}

	url := os.Args[1]
	pngPath := os.Args[2]
	htmlPath := os.Args[3]

	// 1. Create page (1280x720, 30s timeout, 2s settle, no fullpage)
	fmt.Fprintf(os.Stderr, "Creating page...\n")
	page := C.page_new(1280, 720, 30, 2.0, 0)
	if page == nil {
		fmt.Fprintf(os.Stderr, "Error: failed to create page\n")
		os.Exit(1)
	}
	fmt.Fprintf(os.Stderr, "Page created.\n")

	// Ensure cleanup on exit
	defer func() {
		C.page_free(page)
		fmt.Fprintf(os.Stderr, "Done.\n")
	}()

	// 2. Open URL
	fmt.Fprintf(os.Stderr, "Opening %s...\n", url)
	cURL := C.CString(url)
	defer C.free(unsafe.Pointer(cURL))

	rc := C.page_open(page, cURL)
	if rc != scraperOK {
		fmt.Fprintf(os.Stderr, "Error: page_open failed: %s (%d)\n", errorName(rc), rc)
		os.Exit(1)
	}
	fmt.Fprintf(os.Stderr, "Page loaded.\n")

	// 3. Evaluate JS to get the title
	var titleData *C.char
	var titleLen C.size_t
	cScript := C.CString("document.title")
	defer C.free(unsafe.Pointer(cScript))

	rc = C.page_evaluate(page, cScript, &titleData, &titleLen)
	if rc == scraperOK {
		title := C.GoStringN(titleData, C.int(titleLen))
		C.page_string_free(titleData)
		fmt.Fprintf(os.Stderr, "Page title: %s\n", title)
	}

	// 4. Take a screenshot
	fmt.Fprintf(os.Stderr, "Taking screenshot...\n")
	var pngData *C.uint8_t
	var pngLen C.size_t

	rc = C.page_screenshot(page, &pngData, &pngLen)
	if rc != scraperOK {
		fmt.Fprintf(os.Stderr, "Error: screenshot failed: %s (%d)\n", errorName(rc), rc)
	} else {
		pngBytes := C.GoBytes(unsafe.Pointer(pngData), C.int(pngLen))
		C.page_buffer_free(pngData, pngLen)

		if err := os.WriteFile(pngPath, pngBytes, 0644); err != nil {
			fmt.Fprintf(os.Stderr, "Error: cannot write to %s: %v\n", pngPath, err)
		} else {
			fmt.Fprintf(os.Stderr, "Screenshot saved to %s (%d bytes)\n", pngPath, len(pngBytes))
		}
	}

	// 5. Capture HTML
	fmt.Fprintf(os.Stderr, "Capturing HTML...\n")
	var htmlData *C.char
	var htmlLen C.size_t

	rc = C.page_html(page, &htmlData, &htmlLen)
	if rc != scraperOK {
		fmt.Fprintf(os.Stderr, "Error: HTML capture failed: %s (%d)\n", errorName(rc), rc)
	} else {
		htmlStr := C.GoStringN(htmlData, C.int(htmlLen))
		C.page_string_free(htmlData)

		if err := os.WriteFile(htmlPath, []byte(htmlStr), 0644); err != nil {
			fmt.Fprintf(os.Stderr, "Error: cannot write to %s: %v\n", htmlPath, err)
		} else {
			fmt.Fprintf(os.Stderr, "HTML saved to %s (%d bytes)\n", htmlPath, len(htmlStr))
		}
	}
}
