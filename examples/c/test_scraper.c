/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

/**
 * Minimal C test for servo-scraper FFI.
 *
 * Build:
 *   make test-c
 *
 * Usage:
 *   ./target/release/test_scraper https://example.com /tmp/test.png /tmp/test.html
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "servo_scraper.h"

static const char *error_name(int code) {
    switch (code) {
    case SCRAPER_OK:            return "OK";
    case SCRAPER_ERR_INIT:      return "INIT_FAILED";
    case SCRAPER_ERR_LOAD:      return "LOAD_FAILED";
    case SCRAPER_ERR_TIMEOUT:   return "TIMEOUT";
    case SCRAPER_ERR_JS:        return "JS_ERROR";
    case SCRAPER_ERR_SCREENSHOT: return "SCREENSHOT_FAILED";
    case SCRAPER_ERR_CHANNEL:   return "CHANNEL_CLOSED";
    case SCRAPER_ERR_NULL_PTR:  return "NULL_POINTER";
    default:                    return "UNKNOWN";
    }
}

int main(int argc, char *argv[]) {
    if (argc < 4) {
        fprintf(stderr,
                "Usage: %s <URL> <screenshot.png> <output.html>\n"
                "\n"
                "Example:\n"
                "  %s https://example.com /tmp/shot.png /tmp/page.html\n",
                argv[0], argv[0]);
        return 1;
    }

    const char *url = argv[1];
    const char *png_path = argv[2];
    const char *html_path = argv[3];

    /* 1. Create scraper (1280x720, 30s timeout, 2s settle, no fullpage) */
    fprintf(stderr, "Creating scraper...\n");
    ServoScraper *scraper = scraper_new(1280, 720, 30, 2.0, 0);
    if (!scraper) {
        fprintf(stderr, "Error: failed to create scraper\n");
        return 1;
    }
    fprintf(stderr, "Scraper created.\n");

    /* 2. Take a screenshot */
    fprintf(stderr, "Taking screenshot of %s...\n", url);
    uint8_t *png_data = NULL;
    size_t png_len = 0;
    int rc = scraper_screenshot(scraper, url, &png_data, &png_len);
    if (rc != SCRAPER_OK) {
        fprintf(stderr, "Error: screenshot failed: %s (%d)\n", error_name(rc), rc);
    } else {
        FILE *f = fopen(png_path, "wb");
        if (f) {
            fwrite(png_data, 1, png_len, f);
            fclose(f);
            fprintf(stderr, "Screenshot saved to %s (%zu bytes)\n", png_path, png_len);
        } else {
            fprintf(stderr, "Error: cannot open %s for writing\n", png_path);
        }
        scraper_buffer_free(png_data, png_len);
    }

    /* 3. Capture HTML */
    fprintf(stderr, "Capturing HTML of %s...\n", url);
    char *html_data = NULL;
    size_t html_len = 0;
    rc = scraper_html(scraper, url, &html_data, &html_len);
    if (rc != SCRAPER_OK) {
        fprintf(stderr, "Error: HTML capture failed: %s (%d)\n", error_name(rc), rc);
    } else {
        FILE *f = fopen(html_path, "w");
        if (f) {
            fwrite(html_data, 1, html_len, f);
            fclose(f);
            fprintf(stderr, "HTML saved to %s (%zu bytes)\n", html_path, html_len);
        } else {
            fprintf(stderr, "Error: cannot open %s for writing\n", html_path);
        }
        scraper_string_free(html_data);
    }

    /* 4. Cleanup */
    scraper_free(scraper);
    fprintf(stderr, "Done.\n");
    return 0;
}
