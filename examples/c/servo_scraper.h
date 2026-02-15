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
 *   ServoPage *p = page_new(1280, 720, 30, 2.0, 0);
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
 * @param width     Viewport width in pixels.
 * @param height    Viewport height in pixels.
 * @param timeout   Maximum page load time in seconds.
 * @param wait      Post-load JS settle time in seconds.
 * @param fullpage  Non-zero to capture full scrollable page.
 * @return Opaque handle, or NULL on failure. Must be freed with page_free().
 */
ServoPage *page_new(uint32_t width, uint32_t height, uint64_t timeout,
                     double wait, int fullpage);

/**
 * Destroy a page instance. Safe to call with NULL.
 */
void page_free(ServoPage *page);

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
