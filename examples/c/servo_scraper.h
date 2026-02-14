/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

/**
 * @file servo_scraper.h
 * @brief C API for servo-scraper — headless web scraping with the Servo engine.
 *
 * Thread-safe: all functions can be called from any thread.
 * The scraper handle internally runs Servo on a dedicated thread.
 *
 * Usage:
 *   ServoScraper *s = scraper_new(1280, 720, 30, 2.0, 0);
 *   uint8_t *png; size_t png_len;
 *   if (scraper_screenshot(s, "https://example.com", &png, &png_len) == SCRAPER_OK) {
 *       // write png to file...
 *       scraper_buffer_free(png, png_len);
 *   }
 *   scraper_free(s);
 */

#ifndef SERVO_SCRAPER_H
#define SERVO_SCRAPER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Error codes */
#define SCRAPER_OK            0
#define SCRAPER_ERR_INIT      1
#define SCRAPER_ERR_LOAD      2
#define SCRAPER_ERR_TIMEOUT   3
#define SCRAPER_ERR_JS        4
#define SCRAPER_ERR_SCREENSHOT 5
#define SCRAPER_ERR_CHANNEL   6
#define SCRAPER_ERR_NULL_PTR  7

/* Opaque handle */
typedef struct ServoScraper ServoScraper;

/**
 * Create a new scraper instance.
 *
 * @param width     Viewport width in pixels.
 * @param height    Viewport height in pixels.
 * @param timeout   Maximum page load time in seconds.
 * @param wait      Post-load JS settle time in seconds.
 * @param fullpage  Non-zero to capture full scrollable page.
 * @return Opaque handle, or NULL on failure. Must be freed with scraper_free().
 */
ServoScraper *scraper_new(uint32_t width, uint32_t height, uint64_t timeout,
                          double wait, int fullpage);

/**
 * Destroy a scraper instance. Safe to call with NULL.
 */
void scraper_free(ServoScraper *scraper);

/**
 * Take a screenshot of a URL.
 *
 * On success, *out_data is set to a heap-allocated PNG buffer and *out_len
 * to its size in bytes. The caller must free it with scraper_buffer_free().
 *
 * @return SCRAPER_OK on success, or an error code.
 */
int scraper_screenshot(ServoScraper *scraper, const char *url,
                       uint8_t **out_data, size_t *out_len);

/**
 * Capture the HTML content of a URL.
 *
 * On success, *out_html is set to a heap-allocated null-terminated string
 * and *out_len to its length (excluding the null terminator).
 * The caller must free it with scraper_string_free().
 *
 * @return SCRAPER_OK on success, or an error code.
 */
int scraper_html(ServoScraper *scraper, const char *url,
                 char **out_html, size_t *out_len);

/**
 * Free a PNG buffer returned by scraper_screenshot(). Safe to call with NULL.
 */
void scraper_buffer_free(uint8_t *data, size_t len);

/**
 * Free a string returned by scraper_html(). Safe to call with NULL.
 */
void scraper_string_free(char *s);

#ifdef __cplusplus
}
#endif

#endif /* SERVO_SCRAPER_H */
