#!/usr/bin/env python3
"""
servo-scraper Python FFI example.

Uses ctypes to call the shared library (libservo_scraper.dylib / .so).

Usage:
    python3 examples/python/scraper.py https://example.com /tmp/shot.png /tmp/page.html

Requires: make build-lib (to produce the shared library)
"""

import ctypes
import os
import sys
from ctypes import (
    POINTER,
    c_char_p,
    c_double,
    c_int,
    c_size_t,
    c_uint8,
    c_uint32,
    c_uint64,
    c_void_p,
)

# Error codes (must match servo_scraper.h)
SCRAPER_OK = 0
SCRAPER_ERR_INIT = 1
SCRAPER_ERR_LOAD = 2
SCRAPER_ERR_TIMEOUT = 3
SCRAPER_ERR_JS = 4
SCRAPER_ERR_SCREENSHOT = 5
SCRAPER_ERR_CHANNEL = 6
SCRAPER_ERR_NULL_PTR = 7

ERROR_NAMES = {
    SCRAPER_OK: "OK",
    SCRAPER_ERR_INIT: "INIT_FAILED",
    SCRAPER_ERR_LOAD: "LOAD_FAILED",
    SCRAPER_ERR_TIMEOUT: "TIMEOUT",
    SCRAPER_ERR_JS: "JS_ERROR",
    SCRAPER_ERR_SCREENSHOT: "SCREENSHOT_FAILED",
    SCRAPER_ERR_CHANNEL: "CHANNEL_CLOSED",
    SCRAPER_ERR_NULL_PTR: "NULL_POINTER",
}


def find_library():
    """Find libservo_scraper shared library relative to the project root."""
    script_dir = os.path.dirname(os.path.abspath(__file__))
    project_root = os.path.join(script_dir, "..", "..")

    if sys.platform == "darwin":
        lib_name = "libservo_scraper.dylib"
    else:
        lib_name = "libservo_scraper.so"

    lib_path = os.path.join(project_root, "target", "release", lib_name)
    if not os.path.exists(lib_path):
        print(f"Error: {lib_path} not found. Run 'make build-lib' first.", file=sys.stderr)
        sys.exit(1)

    return lib_path


def load_library(lib_path):
    """Load the shared library and set up function signatures."""
    lib = ctypes.CDLL(lib_path)

    # scraper_new(width, height, timeout, wait, fullpage) -> *ServoScraper
    lib.scraper_new.restype = c_void_p
    lib.scraper_new.argtypes = [c_uint32, c_uint32, c_uint64, c_double, c_int]

    # scraper_free(scraper)
    lib.scraper_free.restype = None
    lib.scraper_free.argtypes = [c_void_p]

    # scraper_screenshot(scraper, url, &data, &len) -> int
    lib.scraper_screenshot.restype = c_int
    lib.scraper_screenshot.argtypes = [
        c_void_p,
        c_char_p,
        POINTER(POINTER(c_uint8)),
        POINTER(c_size_t),
    ]

    # scraper_html(scraper, url, &html, &len) -> int
    lib.scraper_html.restype = c_int
    lib.scraper_html.argtypes = [
        c_void_p,
        c_char_p,
        POINTER(c_char_p),
        POINTER(c_size_t),
    ]

    # scraper_buffer_free(data, len)
    lib.scraper_buffer_free.restype = None
    lib.scraper_buffer_free.argtypes = [POINTER(c_uint8), c_size_t]

    # scraper_string_free(str)
    lib.scraper_string_free.restype = None
    lib.scraper_string_free.argtypes = [c_char_p]

    return lib


def main():
    if len(sys.argv) < 4:
        print(
            f"Usage: {sys.argv[0]} <URL> <screenshot.png> <output.html>\n"
            f"\n"
            f"Example:\n"
            f"  python3 {sys.argv[0]} https://example.com /tmp/shot.png /tmp/page.html",
            file=sys.stderr,
        )
        sys.exit(1)

    url = sys.argv[1].encode("utf-8")
    png_path = sys.argv[2]
    html_path = sys.argv[3]

    lib_path = find_library()
    lib = load_library(lib_path)

    # 1. Create scraper
    print("Creating scraper...", file=sys.stderr)
    scraper = lib.scraper_new(1280, 720, 30, 2.0, 0)
    if not scraper:
        print("Error: failed to create scraper", file=sys.stderr)
        sys.exit(1)
    print("Scraper created.", file=sys.stderr)

    try:
        # 2. Take screenshot
        print(f"Taking screenshot of {sys.argv[1]}...", file=sys.stderr)
        png_data = POINTER(c_uint8)()
        png_len = c_size_t(0)
        rc = lib.scraper_screenshot(scraper, url, ctypes.byref(png_data), ctypes.byref(png_len))
        if rc != SCRAPER_OK:
            print(
                f"Error: screenshot failed: {ERROR_NAMES.get(rc, 'UNKNOWN')} ({rc})",
                file=sys.stderr,
            )
        else:
            # Copy data before freeing
            data = bytes(png_data[: png_len.value])
            lib.scraper_buffer_free(png_data, png_len)
            with open(png_path, "wb") as f:
                f.write(data)
            print(f"Screenshot saved to {png_path} ({len(data)} bytes)", file=sys.stderr)

        # 3. Capture HTML
        print(f"Capturing HTML of {sys.argv[1]}...", file=sys.stderr)
        html_data = c_char_p()
        html_len = c_size_t(0)
        rc = lib.scraper_html(scraper, url, ctypes.byref(html_data), ctypes.byref(html_len))
        if rc != SCRAPER_OK:
            print(
                f"Error: HTML capture failed: {ERROR_NAMES.get(rc, 'UNKNOWN')} ({rc})",
                file=sys.stderr,
            )
        else:
            html = html_data.value[: html_len.value].decode("utf-8", errors="replace")
            lib.scraper_string_free(html_data)
            with open(html_path, "w") as f:
                f.write(html)
            print(f"HTML saved to {html_path} ({len(html)} bytes)", file=sys.stderr)

    finally:
        # 4. Cleanup
        lib.scraper_free(scraper)
        print("Done.", file=sys.stderr)


if __name__ == "__main__":
    main()
