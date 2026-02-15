/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

/**
 * @file servo_scraper.h
 * @brief C API for servo-scraper — headless web scraping with the Servo engine.
 *
 * Thread-safe: all functions can be called from any thread.
 * The page handle internally runs Servo on a dedicated thread.
 *
 * Usage:
 *   ServoPage *p = page_new(1280, 720, 30, 2.0, 0, NULL);
 *   page_open(p, "https://example.com");
 *   uint8_t *png; size_t png_len;
 *   if (page_screenshot(p, &png, &png_len) == PAGE_OK) {
 *       // write png to file...
 *       page_buffer_free(png, png_len);
 *   }
 *   page_free(p);
 */

#ifndef SERVO_SCRAPER_H
#define SERVO_SCRAPER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Error codes */
#define PAGE_OK              0
#define PAGE_ERR_INIT        1
#define PAGE_ERR_LOAD        2
#define PAGE_ERR_TIMEOUT     3
#define PAGE_ERR_JS          4
#define PAGE_ERR_SCREENSHOT  5
#define PAGE_ERR_CHANNEL     6
#define PAGE_ERR_NULL_PTR    7
#define PAGE_ERR_NO_PAGE     8
#define PAGE_ERR_SELECTOR    9

/* Opaque handle */
typedef struct ServoPage ServoPage;

/* ── Lifecycle ─────────────────────────────────────────────────────── */

/**
 * Create a new page instance.
 *
 * @param width      Viewport width in pixels.
 * @param height     Viewport height in pixels.
 * @param timeout    Maximum page load time in seconds.
 * @param wait       Post-load JS settle time in seconds.
 * @param fullpage   Non-zero to capture full scrollable page.
 * @param user_agent Custom User-Agent string, or NULL for default.
 * @return Opaque handle, or NULL on failure. Must be freed with page_free().
 */
ServoPage *page_new(uint32_t width, uint32_t height, uint64_t timeout,
                     double wait, int fullpage, const char *user_agent);

/**
 * Destroy a page instance. Safe to call with NULL.
 */
void page_free(ServoPage *page);

/**
 * Reset the page: drop the WebView and clear all internal state
 * (blocked URL patterns, buffered console messages, network requests).
 *
 * After reset(), call page_open() to start a fresh session.
 *
 * @return PAGE_OK on success, or an error code.
 */
int page_reset(ServoPage *page);

/* ── Navigation ────────────────────────────────────────────────────── */

/**
 * Open a URL in the page (creates or navigates the WebView).
 *
 * @return PAGE_OK on success, or an error code.
 */
int page_open(ServoPage *page, const char *url);

/* ── Capture ───────────────────────────────────────────────────────── */

/**
 * Evaluate JavaScript and return the result as a JSON string.
 *
 * On success, *out_json is set to a heap-allocated null-terminated string
 * and *out_len to its length. Free with page_string_free().
 *
 * @return PAGE_OK on success, or an error code.
 */
int page_evaluate(ServoPage *page, const char *script,
                   char **out_json, size_t *out_len);

/**
 * Take a screenshot of the current viewport.
 *
 * On success, *out_data is set to a heap-allocated PNG buffer and *out_len
 * to its size in bytes. Free with page_buffer_free().
 *
 * @return PAGE_OK on success, or an error code.
 */
int page_screenshot(ServoPage *page, uint8_t **out_data, size_t *out_len);

/**
 * Take a full-page screenshot (captures full scrollable page).
 *
 * @return PAGE_OK on success, or an error code.
 */
int page_screenshot_fullpage(ServoPage *page, uint8_t **out_data, size_t *out_len);

/**
 * Capture the HTML content of the current page.
 *
 * On success, *out_html is set to a heap-allocated null-terminated string
 * and *out_len to its length. Free with page_string_free().
 *
 * @return PAGE_OK on success, or an error code.
 */
int page_html(ServoPage *page, char **out_html, size_t *out_len);

/* ── Page info ─────────────────────────────────────────────────────── */

/**
 * Get the current page URL.
 * Free the result with page_string_free().
 */
int page_url(ServoPage *page, char **out_url, size_t *out_len);

/**
 * Get the current page title.
 * Free the result with page_string_free().
 */
int page_title(ServoPage *page, char **out_title, size_t *out_len);

/* ── Events (JSON arrays) ─────────────────────────────────────────── */

/**
 * Get captured console messages as a JSON array.
 * Free the result with page_string_free().
 */
int page_console_messages(ServoPage *page, char **out_json, size_t *out_len);

/**
 * Get captured network requests as a JSON array.
 * Free the result with page_string_free().
 */
int page_network_requests(ServoPage *page, char **out_json, size_t *out_len);

/* ── Wait mechanisms ───────────────────────────────────────────────── */

/**
 * Wait for a CSS selector to match an element on the page.
 */
int page_wait_for_selector(ServoPage *page, const char *selector, uint64_t timeout_secs);

/**
 * Wait for a JS expression to evaluate to a truthy value.
 */
int page_wait_for_condition(ServoPage *page, const char *js_expr, uint64_t timeout_secs);

/**
 * Wait for a fixed number of seconds while keeping the event loop alive.
 */
int page_wait(ServoPage *page, double seconds);

/**
 * Wait for the next navigation to complete.
 */
int page_wait_for_navigation(ServoPage *page, uint64_t timeout_secs);

/* ── Input events ──────────────────────────────────────────────────── */

/**
 * Click at the given device coordinates.
 */
int page_click(ServoPage *page, float x, float y);

/**
 * Click on an element matching a CSS selector.
 */
int page_click_selector(ServoPage *page, const char *selector);

/**
 * Type text by sending individual key events.
 */
int page_type_text(ServoPage *page, const char *text);

/**
 * Press a single key by name (e.g. "Enter", "Tab", "a").
 */
int page_key_press(ServoPage *page, const char *key_name);

/**
 * Move the mouse to the given device coordinates.
 */
int page_mouse_move(ServoPage *page, float x, float y);

/* ── Scroll ────────────────────────────────────────────────────────── */

/**
 * Scroll the viewport by the given pixel deltas.
 */
int page_scroll(ServoPage *page, double delta_x, double delta_y);

/**
 * Scroll an element matching a CSS selector into view.
 */
int page_scroll_to_selector(ServoPage *page, const char *selector);

/* ── Select ────────────────────────────────────────────────────────── */

/**
 * Select an option in a <select> element by value.
 */
int page_select_option(ServoPage *page, const char *selector, const char *value);

/* ── File upload ───────────────────────────────────────────────────── */

/**
 * Set files on an <input type="file"> element.
 * `paths` is a comma-separated list of file paths.
 */
int page_set_input_files(ServoPage *page, const char *selector, const char *paths);

/* ── Cookies ───────────────────────────────────────────────────────── */

/**
 * Get cookies for the current page.
 * Free the result with page_string_free().
 */
int page_get_cookies(ServoPage *page, char **out_cookies, size_t *out_len);

/**
 * Set a cookie via document.cookie.
 */
int page_set_cookie(ServoPage *page, const char *cookie);

/**
 * Clear all cookies for the current page.
 */
int page_clear_cookies(ServoPage *page);

/* ── Request interception ──────────────────────────────────────────── */

/**
 * Set URL patterns to block (comma-separated). Pass NULL to clear.
 */
int page_block_urls(ServoPage *page, const char *patterns);

/* ── Navigation (extended) ─────────────────────────────────────────── */

/**
 * Reload the current page.
 */
int page_reload(ServoPage *page);

/**
 * Navigate back in history. Returns PAGE_ERR_NO_PAGE if no history.
 */
int page_go_back(ServoPage *page);

/**
 * Navigate forward in history. Returns PAGE_ERR_NO_PAGE if no forward history.
 */
int page_go_forward(ServoPage *page);

/* ── Element info ──────────────────────────────────────────────────── */

/**
 * Get the bounding rectangle of an element as JSON.
 * Free the result with page_string_free().
 */
int page_element_rect(ServoPage *page, const char *selector,
                       char **out_json, size_t *out_len);

/**
 * Get the text content of an element.
 * Free the result with page_string_free().
 */
int page_element_text(ServoPage *page, const char *selector,
                       char **out_text, size_t *out_len);

/**
 * Get an attribute value of an element.
 * Free the result with page_string_free().
 */
int page_element_attribute(ServoPage *page, const char *selector,
                            const char *attribute,
                            char **out_value, size_t *out_len);

/**
 * Get the outer HTML of an element.
 * Free the result with page_string_free().
 */
int page_element_html(ServoPage *page, const char *selector,
                       char **out_html, size_t *out_len);

/* ── Memory ────────────────────────────────────────────────────────── */

/**
 * Free a PNG buffer returned by page_screenshot(). Safe to call with NULL.
 */
void page_buffer_free(uint8_t *data, size_t len);

/**
 * Free a string returned by page functions. Safe to call with NULL.
 */
void page_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif /* SERVO_SCRAPER_H */
