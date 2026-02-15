/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Layer 3: C FFI — `extern "C"` functions wrapping [`Page`](crate::Page).

use crate::page::Page;
use crate::types::{PageError, PageOptions};

const PAGE_OK: i32 = 0;
const PAGE_ERR_INIT: i32 = 1;
const PAGE_ERR_LOAD: i32 = 2;
const PAGE_ERR_TIMEOUT: i32 = 3;
const PAGE_ERR_JS: i32 = 4;
const PAGE_ERR_SCREENSHOT: i32 = 5;
const PAGE_ERR_CHANNEL: i32 = 6;
const PAGE_ERR_NULL_PTR: i32 = 7;
const PAGE_ERR_NO_PAGE: i32 = 8;
const PAGE_ERR_SELECTOR: i32 = 9;

fn error_code(e: &PageError) -> i32 {
    match e {
        PageError::InitFailed(_) => PAGE_ERR_INIT,
        PageError::LoadFailed(_) => PAGE_ERR_LOAD,
        PageError::Timeout => PAGE_ERR_TIMEOUT,
        PageError::JsError(_) => PAGE_ERR_JS,
        PageError::ScreenshotFailed(_) => PAGE_ERR_SCREENSHOT,
        PageError::ChannelClosed => PAGE_ERR_CHANNEL,
        PageError::NoPage => PAGE_ERR_NO_PAGE,
        PageError::SelectorNotFound(_) => PAGE_ERR_SELECTOR,
    }
}

// -- Lifecycle --

/// Create a new page instance.
///
/// Returns an opaque pointer, or NULL on failure.
/// The caller must free it with `page_free()`.
///
/// # Safety
///
/// The returned pointer must be freed with `page_free()`.
#[unsafe(no_mangle)]
pub extern "C" fn page_new(
    width: u32,
    height: u32,
    timeout: u64,
    wait: f64,
    fullpage: i32,
) -> *mut Page {
    let options = PageOptions {
        width,
        height,
        timeout,
        wait,
        fullpage: fullpage != 0,
    };
    match Page::new(options) {
        Ok(p) => Box::into_raw(Box::new(p)),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Destroy a page instance. Safe to call with NULL.
///
/// # Safety
///
/// `page` must be a valid pointer returned by `page_new()`, or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_free(page: *mut Page) {
    if !page.is_null() {
        unsafe { drop(Box::from_raw(page)) };
    }
}

// -- Navigation --

/// Open a URL in the page.
///
/// # Safety
///
/// `page` must be a valid pointer from `page_new()`. `url` must be a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_open(page: *mut Page, url: *const std::ffi::c_char) -> i32 {
    if page.is_null() || url.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let url_str = match unsafe { std::ffi::CStr::from_ptr(url) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_LOAD,
    };
    match page.open(url_str) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

// -- Capture --

/// Evaluate JavaScript and return the result as a JSON string.
///
/// On success, `*out_json` is set to a heap-allocated null-terminated string
/// and `*out_len` to its length. Free with `page_string_free()`.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_evaluate(
    page: *mut Page,
    script: *const std::ffi::c_char,
    out_json: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || script.is_null() || out_json.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let script_str = match unsafe { std::ffi::CStr::from_ptr(script) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    match page.evaluate(script_str) {
        Ok(json) => match std::ffi::CString::new(json) {
            Ok(cstr) => {
                let len = cstr.as_bytes().len();
                let ptr = cstr.into_raw();
                unsafe {
                    *out_json = ptr;
                    *out_len = len;
                }
                PAGE_OK
            }
            Err(_) => PAGE_ERR_JS,
        },
        Err(e) => error_code(&e),
    }
}

/// Take a screenshot. Returns PNG bytes.
///
/// On success, `*out_data` and `*out_len` are set. Free with `page_buffer_free()`.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_screenshot(
    page: *mut Page,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_data.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.screenshot() {
        Ok(png_bytes) => {
            let boxed = png_bytes.into_boxed_slice();
            let len = boxed.len();
            let ptr = Box::into_raw(boxed) as *mut u8;
            unsafe {
                *out_data = ptr;
                *out_len = len;
            }
            PAGE_OK
        }
        Err(e) => error_code(&e),
    }
}

/// Take a full-page screenshot. Returns PNG bytes.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_screenshot_fullpage(
    page: *mut Page,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_data.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.screenshot_fullpage() {
        Ok(png_bytes) => {
            let boxed = png_bytes.into_boxed_slice();
            let len = boxed.len();
            let ptr = Box::into_raw(boxed) as *mut u8;
            unsafe {
                *out_data = ptr;
                *out_len = len;
            }
            PAGE_OK
        }
        Err(e) => error_code(&e),
    }
}

/// Capture the page HTML.
///
/// On success, `*out_html` and `*out_len` are set. Free with `page_string_free()`.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_html(
    page: *mut Page,
    out_html: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_html.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.html() {
        Ok(html) => match std::ffi::CString::new(html) {
            Ok(cstr) => {
                let len = cstr.as_bytes().len();
                let ptr = cstr.into_raw();
                unsafe {
                    *out_html = ptr;
                    *out_len = len;
                }
                PAGE_OK
            }
            Err(_) => PAGE_ERR_JS,
        },
        Err(e) => error_code(&e),
    }
}

// -- Page info --

/// Get the current page URL.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_url(
    page: *mut Page,
    out_url: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_url.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.url() {
        Some(url_str) => match std::ffi::CString::new(url_str) {
            Ok(cstr) => {
                let len = cstr.as_bytes().len();
                let ptr = cstr.into_raw();
                unsafe {
                    *out_url = ptr;
                    *out_len = len;
                }
                PAGE_OK
            }
            Err(_) => PAGE_ERR_JS,
        },
        None => PAGE_ERR_NO_PAGE,
    }
}

/// Get the current page title.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_title(
    page: *mut Page,
    out_title: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_title.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.title() {
        Some(title_str) => match std::ffi::CString::new(title_str) {
            Ok(cstr) => {
                let len = cstr.as_bytes().len();
                let ptr = cstr.into_raw();
                unsafe {
                    *out_title = ptr;
                    *out_len = len;
                }
                PAGE_OK
            }
            Err(_) => PAGE_ERR_JS,
        },
        None => PAGE_ERR_NO_PAGE,
    }
}

// -- Events (JSON) --

/// Get console messages as a JSON array.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_console_messages(
    page: *mut Page,
    out_json: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_json.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let msgs = page.console_messages();
    let json = serde_json::to_string(&msgs).unwrap_or_else(|_| "[]".to_string());
    match std::ffi::CString::new(json) {
        Ok(cstr) => {
            let len = cstr.as_bytes().len();
            let ptr = cstr.into_raw();
            unsafe {
                *out_json = ptr;
                *out_len = len;
            }
            PAGE_OK
        }
        Err(_) => PAGE_ERR_JS,
    }
}

/// Get network requests as a JSON array.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_network_requests(
    page: *mut Page,
    out_json: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_json.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let reqs = page.network_requests();
    let json = serde_json::to_string(&reqs).unwrap_or_else(|_| "[]".to_string());
    match std::ffi::CString::new(json) {
        Ok(cstr) => {
            let len = cstr.as_bytes().len();
            let ptr = cstr.into_raw();
            unsafe {
                *out_json = ptr;
                *out_len = len;
            }
            PAGE_OK
        }
        Err(_) => PAGE_ERR_JS,
    }
}

// -- Wait FFI --

/// Wait for a CSS selector to match an element.
///
/// # Safety
///
/// `page` and `selector` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_wait_for_selector(
    page: *mut Page,
    selector: *const std::ffi::c_char,
    timeout_secs: u64,
) -> i32 {
    if page.is_null() || selector.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let sel = match unsafe { std::ffi::CStr::from_ptr(selector) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    match page.wait_for_selector(sel, timeout_secs) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

/// Wait for a JS expression to evaluate to a truthy value.
///
/// # Safety
///
/// `page` and `js_expr` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_wait_for_condition(
    page: *mut Page,
    js_expr: *const std::ffi::c_char,
    timeout_secs: u64,
) -> i32 {
    if page.is_null() || js_expr.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let expr = match unsafe { std::ffi::CStr::from_ptr(js_expr) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    match page.wait_for_condition(expr, timeout_secs) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

/// Wait for a fixed number of seconds.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_wait(page: *mut Page, seconds: f64) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    page.wait(seconds);
    PAGE_OK
}

/// Wait for the next navigation to complete.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_wait_for_navigation(page: *mut Page, timeout_secs: u64) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.wait_for_navigation(timeout_secs) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

// -- Input FFI --

/// Click at the given coordinates.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_click(page: *mut Page, x: f32, y: f32) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.click(x, y) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

/// Click on an element matching a CSS selector.
///
/// # Safety
///
/// `page` and `selector` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_click_selector(
    page: *mut Page,
    selector: *const std::ffi::c_char,
) -> i32 {
    if page.is_null() || selector.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let sel = match unsafe { std::ffi::CStr::from_ptr(selector) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    match page.click_selector(sel) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

/// Type text by sending individual key events.
///
/// # Safety
///
/// `page` and `text` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_type_text(page: *mut Page, text: *const std::ffi::c_char) -> i32 {
    if page.is_null() || text.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let text_str = match unsafe { std::ffi::CStr::from_ptr(text) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    match page.type_text(text_str) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

/// Press a single key by name (e.g. "Enter", "Tab", "a").
///
/// # Safety
///
/// `page` and `key_name` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_key_press(page: *mut Page, key_name: *const std::ffi::c_char) -> i32 {
    if page.is_null() || key_name.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let name = match unsafe { std::ffi::CStr::from_ptr(key_name) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    match page.key_press(name) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

/// Move the mouse to the given coordinates.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_mouse_move(page: *mut Page, x: f32, y: f32) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.mouse_move(x, y) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

// -- Memory --

/// Free a buffer returned by `page_screenshot()` or `page_screenshot_fullpage()`.
///
/// # Safety
///
/// `data` must be a pointer returned by a page screenshot function, or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_buffer_free(data: *mut u8, len: usize) {
    if !data.is_null() && len > 0 {
        unsafe {
            let slice = std::slice::from_raw_parts_mut(data, len);
            drop(Box::from_raw(slice));
        }
    }
}

/// Free a string returned by page FFI functions.
///
/// # Safety
///
/// `s` must be a pointer returned by a page string function, or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_string_free(s: *mut std::ffi::c_char) {
    if !s.is_null() {
        unsafe { drop(std::ffi::CString::from_raw(s)) };
    }
}
