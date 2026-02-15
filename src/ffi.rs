/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Layer 3: C FFI — `extern "C"` functions wrapping [`Page`](crate::Page).

use crate::page::Page;
use crate::types::{InputFile, PageError, PageOptions};

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
/// `user_agent` may be NULL to use the default User-Agent.
///
/// # Safety
///
/// The returned pointer must be freed with `page_free()`.
/// `user_agent`, if not NULL, must be a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_new(
    width: u32,
    height: u32,
    timeout: u64,
    wait: f64,
    fullpage: i32,
    user_agent: *const std::ffi::c_char,
) -> *mut Page {
    let ua = if user_agent.is_null() {
        None
    } else {
        match unsafe { std::ffi::CStr::from_ptr(user_agent) }.to_str() {
            Ok(s) => Some(s.to_string()),
            Err(_) => None,
        }
    };
    let options = PageOptions {
        width,
        height,
        timeout,
        wait,
        fullpage: fullpage != 0,
        user_agent: ua,
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

/// Reset all state: drop the WebView, clear blocked URL patterns,
/// and drain buffered console messages and network requests.
///
/// # Safety
///
/// `page` must be a valid pointer from `page_new()`, or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_reset(page: *mut Page) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    page.reset();
    PAGE_OK
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

/// Wait until no new network requests arrive for `idle_ms` milliseconds.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_wait_for_network_idle(
    page: *mut Page,
    idle_ms: u64,
    timeout_secs: u64,
) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.wait_for_network_idle(idle_ms, timeout_secs) {
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

// -- Scroll FFI --

/// Scroll the viewport by the given pixel deltas.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_scroll(page: *mut Page, delta_x: f64, delta_y: f64) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.scroll(delta_x, delta_y) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

/// Scroll an element matching a CSS selector into view.
///
/// # Safety
///
/// `page` and `selector` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_scroll_to_selector(
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
    match page.scroll_to_selector(sel) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

// -- Select FFI --

/// Select an option in a `<select>` element by value.
///
/// # Safety
///
/// `page`, `selector`, and `value` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_select_option(
    page: *mut Page,
    selector: *const std::ffi::c_char,
    value: *const std::ffi::c_char,
) -> i32 {
    if page.is_null() || selector.is_null() || value.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let sel = match unsafe { std::ffi::CStr::from_ptr(selector) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    let val = match unsafe { std::ffi::CStr::from_ptr(value) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    match page.select_option(sel, val) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

// -- File upload FFI --

/// Set files on an `<input type="file">` element.
///
/// `paths` is a comma-separated list of file paths. Each file is read from disk,
/// its MIME type inferred from the extension, and injected via the DataTransfer API.
///
/// # Safety
///
/// `page`, `selector`, and `paths` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_set_input_files(
    page: *mut Page,
    selector: *const std::ffi::c_char,
    paths: *const std::ffi::c_char,
) -> i32 {
    if page.is_null() || selector.is_null() || paths.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let sel = match unsafe { std::ffi::CStr::from_ptr(selector) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    let paths_str = match unsafe { std::ffi::CStr::from_ptr(paths) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };

    let mut files = Vec::new();
    for path_str in paths_str
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        let path = std::path::Path::new(path_str);
        let data = match std::fs::read(path) {
            Ok(d) => d,
            Err(_) => return PAGE_ERR_JS,
        };
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let mime_type = match path.extension().and_then(|e| e.to_str()) {
            Some("txt") => "text/plain",
            Some("html") | Some("htm") => "text/html",
            Some("css") => "text/css",
            Some("js") => "application/javascript",
            Some("json") => "application/json",
            Some("xml") => "application/xml",
            Some("pdf") => "application/pdf",
            Some("png") => "image/png",
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("svg") => "image/svg+xml",
            Some("webp") => "image/webp",
            Some("zip") => "application/zip",
            Some("csv") => "text/csv",
            _ => "application/octet-stream",
        }
        .to_string();
        files.push(InputFile {
            name,
            mime_type,
            data,
        });
    }

    match page.set_input_files(sel, files) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

// -- Cookies FFI --

/// Get cookies for the current page.
///
/// On success, `*out_cookies` and `*out_len` are set. Free with `page_string_free()`.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_get_cookies(
    page: *mut Page,
    out_cookies: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_cookies.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.get_cookies() {
        Ok(cookies) => match std::ffi::CString::new(cookies) {
            Ok(cstr) => {
                let len = cstr.as_bytes().len();
                let ptr = cstr.into_raw();
                unsafe {
                    *out_cookies = ptr;
                    *out_len = len;
                }
                PAGE_OK
            }
            Err(_) => PAGE_ERR_JS,
        },
        Err(e) => error_code(&e),
    }
}

/// Set a cookie via `document.cookie`.
///
/// # Safety
///
/// `page` and `cookie` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_set_cookie(page: *mut Page, cookie: *const std::ffi::c_char) -> i32 {
    if page.is_null() || cookie.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let cookie_str = match unsafe { std::ffi::CStr::from_ptr(cookie) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    match page.set_cookie(cookie_str) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

/// Clear all cookies for the current page.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_clear_cookies(page: *mut Page) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.clear_cookies() {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

// -- Request interception FFI --

/// Set URL patterns to block (comma-separated). Pass NULL to clear.
///
/// # Safety
///
/// `page` must be a valid pointer. `patterns` may be NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_block_urls(
    page: *mut Page,
    patterns: *const std::ffi::c_char,
) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    if patterns.is_null() {
        page.clear_blocked_urls();
    } else {
        let pat_str = match unsafe { std::ffi::CStr::from_ptr(patterns) }.to_str() {
            Ok(s) => s,
            Err(_) => return PAGE_ERR_JS,
        };
        let pats: Vec<String> = pat_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        page.block_urls(pats);
    }
    PAGE_OK
}

// -- Navigation FFI --

/// Reload the current page.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_reload(page: *mut Page) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.reload() {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

/// Navigate back in history. Returns `PAGE_ERR_NO_PAGE` if no history.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_go_back(page: *mut Page) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.go_back() {
        Ok(true) => PAGE_OK,
        Ok(false) => PAGE_ERR_NO_PAGE,
        Err(e) => error_code(&e),
    }
}

/// Navigate forward in history. Returns `PAGE_ERR_NO_PAGE` if no forward history.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_go_forward(page: *mut Page) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.go_forward() {
        Ok(true) => PAGE_OK,
        Ok(false) => PAGE_ERR_NO_PAGE,
        Err(e) => error_code(&e),
    }
}

// -- Element info FFI --

/// Get the bounding rectangle of an element as JSON (`{"x":..,"y":..,"width":..,"height":..}`).
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_element_rect(
    page: *mut Page,
    selector: *const std::ffi::c_char,
    out_json: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || selector.is_null() || out_json.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let sel = match unsafe { std::ffi::CStr::from_ptr(selector) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    match page.element_rect(sel) {
        Ok(rect) => {
            let json = serde_json::to_string(&rect).unwrap_or_else(|_| "{}".to_string());
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
        Err(e) => error_code(&e),
    }
}

/// Get the text content of an element.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_element_text(
    page: *mut Page,
    selector: *const std::ffi::c_char,
    out_text: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || selector.is_null() || out_text.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let sel = match unsafe { std::ffi::CStr::from_ptr(selector) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    match page.element_text(sel) {
        Ok(text) => match std::ffi::CString::new(text) {
            Ok(cstr) => {
                let len = cstr.as_bytes().len();
                let ptr = cstr.into_raw();
                unsafe {
                    *out_text = ptr;
                    *out_len = len;
                }
                PAGE_OK
            }
            Err(_) => PAGE_ERR_JS,
        },
        Err(e) => error_code(&e),
    }
}

/// Get an attribute value of an element. Returns empty string if attribute doesn't exist.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_element_attribute(
    page: *mut Page,
    selector: *const std::ffi::c_char,
    attribute: *const std::ffi::c_char,
    out_value: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null()
        || selector.is_null()
        || attribute.is_null()
        || out_value.is_null()
        || out_len.is_null()
    {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let sel = match unsafe { std::ffi::CStr::from_ptr(selector) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    let attr = match unsafe { std::ffi::CStr::from_ptr(attribute) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    match page.element_attribute(sel, attr) {
        Ok(value) => {
            let s = value.unwrap_or_default();
            match std::ffi::CString::new(s) {
                Ok(cstr) => {
                    let len = cstr.as_bytes().len();
                    let ptr = cstr.into_raw();
                    unsafe {
                        *out_value = ptr;
                        *out_len = len;
                    }
                    PAGE_OK
                }
                Err(_) => PAGE_ERR_JS,
            }
        }
        Err(e) => error_code(&e),
    }
}

/// Get the outer HTML of an element.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_element_html(
    page: *mut Page,
    selector: *const std::ffi::c_char,
    out_html: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || selector.is_null() || out_html.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let sel = match unsafe { std::ffi::CStr::from_ptr(selector) }.to_str() {
        Ok(s) => s,
        Err(_) => return PAGE_ERR_JS,
    };
    match page.element_html(sel) {
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

// -- Multi-page FFI --

/// Create a new page with the default viewport size.
/// On success, `*out_id` is set to the new page ID.
///
/// # Safety
///
/// `page` and `out_id` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_new_page(page: *mut Page, out_id: *mut u32) -> i32 {
    if page.is_null() || out_id.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.new_page() {
        Ok(id) => {
            unsafe { *out_id = id };
            PAGE_OK
        }
        Err(e) => error_code(&e),
    }
}

/// Create a new page with a custom viewport size.
/// On success, `*out_id` is set to the new page ID.
///
/// # Safety
///
/// `page` and `out_id` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_new_page_with_size(
    page: *mut Page,
    width: u32,
    height: u32,
    out_id: *mut u32,
) -> i32 {
    if page.is_null() || out_id.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.new_page_with_size(width, height) {
        Ok(id) => {
            unsafe { *out_id = id };
            PAGE_OK
        }
        Err(e) => error_code(&e),
    }
}

/// Switch the active page to the given ID.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_switch_to(page: *mut Page, page_id: u32) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.switch_to(page_id) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

/// Close a specific page by ID.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_close_page(page: *mut Page, page_id: u32) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.close_page(page_id) {
        Ok(()) => PAGE_OK,
        Err(e) => error_code(&e),
    }
}

/// Get the active page ID.
/// On success, `*out_id` is set. Returns `PAGE_ERR_NO_PAGE` if no page is active.
///
/// # Safety
///
/// `page` and `out_id` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_active_page_id(page: *mut Page, out_id: *mut u32) -> i32 {
    if page.is_null() || out_id.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.active_page_id() {
        Some(id) => {
            unsafe { *out_id = id };
            PAGE_OK
        }
        None => PAGE_ERR_NO_PAGE,
    }
}

/// Get all open page IDs as a JSON array string (e.g. `[0,1,2]`).
/// Free the result with `page_string_free()`.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_page_ids(
    page: *mut Page,
    out_json: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_json.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let ids = page.page_ids();
    let json = serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string());
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

/// Get the number of open pages.
///
/// # Safety
///
/// `page` and `out_count` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_page_count(page: *mut Page, out_count: *mut usize) -> i32 {
    if page.is_null() || out_count.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    unsafe { *out_count = page.page_count() };
    PAGE_OK
}

/// Enable or disable popup capture. Pass non-zero to enable.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_set_popup_handling(page: *mut Page, enabled: i32) -> i32 {
    if page.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    page.set_popup_handling(enabled != 0);
    PAGE_OK
}

/// Drain pending popup pages and return their IDs as a JSON array.
/// Free the result with `page_string_free()`.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_popup_pages(
    page: *mut Page,
    out_json: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_json.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let ids = page.popup_pages();
    let json = serde_json::to_string(&ids).unwrap_or_else(|_| "[]".to_string());
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

/// Get the URL of a specific page by ID.
/// Free the result with `page_string_free()`.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_page_url(
    page: *mut Page,
    page_id: u32,
    out_url: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_url.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.page_url(page_id) {
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

/// Get the title of a specific page by ID.
/// Free the result with `page_string_free()`.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_page_title(
    page: *mut Page,
    page_id: u32,
    out_title: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_title.is_null() || out_len.is_null() {
        return PAGE_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.page_title(page_id) {
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
