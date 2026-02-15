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
    c_float,
    c_int,
    c_size_t,
    c_uint8,
    c_uint32,
    c_uint64,
    c_void_p,
)

# Error codes (must match servo_scraper.h)
PAGE_OK = 0
PAGE_ERR_INIT = 1
PAGE_ERR_LOAD = 2
PAGE_ERR_TIMEOUT = 3
PAGE_ERR_JS = 4
PAGE_ERR_SCREENSHOT = 5
PAGE_ERR_CHANNEL = 6
PAGE_ERR_NULL_PTR = 7
PAGE_ERR_NO_PAGE = 8
PAGE_ERR_SELECTOR = 9

ERROR_NAMES = {
    PAGE_OK: "OK",
    PAGE_ERR_INIT: "INIT_FAILED",
    PAGE_ERR_LOAD: "LOAD_FAILED",
    PAGE_ERR_TIMEOUT: "TIMEOUT",
    PAGE_ERR_JS: "JS_ERROR",
    PAGE_ERR_SCREENSHOT: "SCREENSHOT_FAILED",
    PAGE_ERR_CHANNEL: "CHANNEL_CLOSED",
    PAGE_ERR_NULL_PTR: "NULL_POINTER",
    PAGE_ERR_NO_PAGE: "NO_PAGE",
    PAGE_ERR_SELECTOR: "SELECTOR_NOT_FOUND",
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

    # page_new(width, height, timeout, wait, fullpage, user_agent) -> *ServoPage
    lib.page_new.restype = c_void_p
    lib.page_new.argtypes = [c_uint32, c_uint32, c_uint64, c_double, c_int, c_char_p]

    # page_free(page)
    lib.page_free.restype = None
    lib.page_free.argtypes = [c_void_p]

    # page_open(page, url) -> int
    lib.page_open.restype = c_int
    lib.page_open.argtypes = [c_void_p, c_char_p]

    # page_evaluate(page, script, &json, &len) -> int
    lib.page_evaluate.restype = c_int
    lib.page_evaluate.argtypes = [c_void_p, c_char_p, POINTER(c_char_p), POINTER(c_size_t)]

    # page_screenshot(page, &data, &len) -> int
    lib.page_screenshot.restype = c_int
    lib.page_screenshot.argtypes = [c_void_p, POINTER(POINTER(c_uint8)), POINTER(c_size_t)]

    # page_screenshot_fullpage(page, &data, &len) -> int
    lib.page_screenshot_fullpage.restype = c_int
    lib.page_screenshot_fullpage.argtypes = [c_void_p, POINTER(POINTER(c_uint8)), POINTER(c_size_t)]

    # page_html(page, &html, &len) -> int
    lib.page_html.restype = c_int
    lib.page_html.argtypes = [c_void_p, POINTER(c_char_p), POINTER(c_size_t)]

    # page_url(page, &url, &len) -> int
    lib.page_url.restype = c_int
    lib.page_url.argtypes = [c_void_p, POINTER(c_char_p), POINTER(c_size_t)]

    # page_title(page, &title, &len) -> int
    lib.page_title.restype = c_int
    lib.page_title.argtypes = [c_void_p, POINTER(c_char_p), POINTER(c_size_t)]

    # page_console_messages(page, &json, &len) -> int
    lib.page_console_messages.restype = c_int
    lib.page_console_messages.argtypes = [c_void_p, POINTER(c_char_p), POINTER(c_size_t)]

    # page_network_requests(page, &json, &len) -> int
    lib.page_network_requests.restype = c_int
    lib.page_network_requests.argtypes = [c_void_p, POINTER(c_char_p), POINTER(c_size_t)]

    # page_wait_for_selector(page, selector, timeout) -> int
    lib.page_wait_for_selector.restype = c_int
    lib.page_wait_for_selector.argtypes = [c_void_p, c_char_p, c_uint64]

    # page_wait_for_condition(page, js_expr, timeout) -> int
    lib.page_wait_for_condition.restype = c_int
    lib.page_wait_for_condition.argtypes = [c_void_p, c_char_p, c_uint64]

    # page_wait(page, seconds) -> int
    lib.page_wait.restype = c_int
    lib.page_wait.argtypes = [c_void_p, c_double]

    # page_wait_for_navigation(page, timeout) -> int
    lib.page_wait_for_navigation.restype = c_int
    lib.page_wait_for_navigation.argtypes = [c_void_p, c_uint64]

    # page_click(page, x, y) -> int
    lib.page_click.restype = c_int
    lib.page_click.argtypes = [c_void_p, c_float, c_float]

    # page_click_selector(page, selector) -> int
    lib.page_click_selector.restype = c_int
    lib.page_click_selector.argtypes = [c_void_p, c_char_p]

    # page_type_text(page, text) -> int
    lib.page_type_text.restype = c_int
    lib.page_type_text.argtypes = [c_void_p, c_char_p]

    # page_key_press(page, key_name) -> int
    lib.page_key_press.restype = c_int
    lib.page_key_press.argtypes = [c_void_p, c_char_p]

    # page_mouse_move(page, x, y) -> int
    lib.page_mouse_move.restype = c_int
    lib.page_mouse_move.argtypes = [c_void_p, c_float, c_float]

    # page_reset(page) -> int
    lib.page_reset.restype = c_int
    lib.page_reset.argtypes = [c_void_p]

    # page_get_cookies(page, &cookies, &len) -> int
    lib.page_get_cookies.restype = c_int
    lib.page_get_cookies.argtypes = [c_void_p, POINTER(c_char_p), POINTER(c_size_t)]

    # page_set_cookie(page, cookie) -> int
    lib.page_set_cookie.restype = c_int
    lib.page_set_cookie.argtypes = [c_void_p, c_char_p]

    # page_clear_cookies(page) -> int
    lib.page_clear_cookies.restype = c_int
    lib.page_clear_cookies.argtypes = [c_void_p]

    # page_block_urls(page, patterns) -> int
    lib.page_block_urls.restype = c_int
    lib.page_block_urls.argtypes = [c_void_p, c_char_p]

    # page_reload(page) -> int
    lib.page_reload.restype = c_int
    lib.page_reload.argtypes = [c_void_p]

    # page_go_back(page) -> int
    lib.page_go_back.restype = c_int
    lib.page_go_back.argtypes = [c_void_p]

    # page_go_forward(page) -> int
    lib.page_go_forward.restype = c_int
    lib.page_go_forward.argtypes = [c_void_p]

    # page_element_rect(page, selector, &json, &len) -> int
    lib.page_element_rect.restype = c_int
    lib.page_element_rect.argtypes = [c_void_p, c_char_p, POINTER(c_char_p), POINTER(c_size_t)]

    # page_element_text(page, selector, &text, &len) -> int
    lib.page_element_text.restype = c_int
    lib.page_element_text.argtypes = [c_void_p, c_char_p, POINTER(c_char_p), POINTER(c_size_t)]

    # page_element_attribute(page, selector, attribute, &value, &len) -> int
    lib.page_element_attribute.restype = c_int
    lib.page_element_attribute.argtypes = [c_void_p, c_char_p, c_char_p, POINTER(c_char_p), POINTER(c_size_t)]

    # page_element_html(page, selector, &html, &len) -> int
    lib.page_element_html.restype = c_int
    lib.page_element_html.argtypes = [c_void_p, c_char_p, POINTER(c_char_p), POINTER(c_size_t)]

    # page_buffer_free(data, len)
    lib.page_buffer_free.restype = None
    lib.page_buffer_free.argtypes = [POINTER(c_uint8), c_size_t]

    # page_string_free(str)
    lib.page_string_free.restype = None
    lib.page_string_free.argtypes = [c_char_p]

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

    # 1. Create page
    print("Creating page...", file=sys.stderr)
    page = lib.page_new(1280, 720, 30, 2.0, 0, None)
    if not page:
        print("Error: failed to create page", file=sys.stderr)
        sys.exit(1)
    print("Page created.", file=sys.stderr)

    try:
        # 2. Open URL
        print(f"Opening {sys.argv[1]}...", file=sys.stderr)
        rc = lib.page_open(page, url)
        if rc != PAGE_OK:
            print(
                f"Error: page_open failed: {ERROR_NAMES.get(rc, 'UNKNOWN')} ({rc})",
                file=sys.stderr,
            )
            sys.exit(1)
        print("Page loaded.", file=sys.stderr)

        # 3. Evaluate JS to get the title
        title_data = c_char_p()
        title_len = c_size_t(0)
        rc = lib.page_evaluate(page, b"document.title", ctypes.byref(title_data), ctypes.byref(title_len))
        if rc == PAGE_OK:
            title = title_data.value[:title_len.value].decode("utf-8", errors="replace")
            lib.page_string_free(title_data)
            print(f"Page title: {title}", file=sys.stderr)

        # 4. Take screenshot
        print("Taking screenshot...", file=sys.stderr)
        png_data = POINTER(c_uint8)()
        png_len = c_size_t(0)
        rc = lib.page_screenshot(page, ctypes.byref(png_data), ctypes.byref(png_len))
        if rc != PAGE_OK:
            print(
                f"Error: screenshot failed: {ERROR_NAMES.get(rc, 'UNKNOWN')} ({rc})",
                file=sys.stderr,
            )
        else:
            data = bytes(png_data[: png_len.value])
            lib.page_buffer_free(png_data, png_len)
            with open(png_path, "wb") as f:
                f.write(data)
            print(f"Screenshot saved to {png_path} ({len(data)} bytes)", file=sys.stderr)

        # 5. Capture HTML
        print("Capturing HTML...", file=sys.stderr)
        html_data = c_char_p()
        html_len = c_size_t(0)
        rc = lib.page_html(page, ctypes.byref(html_data), ctypes.byref(html_len))
        if rc != PAGE_OK:
            print(
                f"Error: HTML capture failed: {ERROR_NAMES.get(rc, 'UNKNOWN')} ({rc})",
                file=sys.stderr,
            )
        else:
            html = html_data.value[: html_len.value].decode("utf-8", errors="replace")
            lib.page_string_free(html_data)
            with open(html_path, "w") as f:
                f.write(html)
            print(f"HTML saved to {html_path} ({len(html)} bytes)", file=sys.stderr)

    finally:
        # 6. Cleanup
        lib.page_free(page)
        print("Done.", file=sys.stderr)


if __name__ == "__main__":
    main()
