/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

/**
 * Minimal C test for servo-scraper Page FFI.
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
    case PAGE_OK:             return "OK";
    case PAGE_ERR_INIT:       return "INIT_FAILED";
    case PAGE_ERR_LOAD:       return "LOAD_FAILED";
    case PAGE_ERR_TIMEOUT:    return "TIMEOUT";
    case PAGE_ERR_JS:         return "JS_ERROR";
    case PAGE_ERR_SCREENSHOT: return "SCREENSHOT_FAILED";
    case PAGE_ERR_CHANNEL:    return "CHANNEL_CLOSED";
    case PAGE_ERR_NULL_PTR:   return "NULL_POINTER";
    case PAGE_ERR_NO_PAGE:    return "NO_PAGE";
    case PAGE_ERR_SELECTOR:   return "SELECTOR_NOT_FOUND";
    default:                     return "UNKNOWN";
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

    /* 1. Create page (1280x720, 30s timeout, 2s settle, no fullpage) */
    fprintf(stderr, "Creating page...\n");
    ServoPage *page = page_new(1280, 720, 30, 2.0, 0, NULL);
    if (!page) {
        fprintf(stderr, "Error: failed to create page\n");
        return 1;
    }
    fprintf(stderr, "Page created.\n");

    /* 2. Open URL */
    fprintf(stderr, "Opening %s...\n", url);
    int rc = page_open(page, url);
    if (rc != PAGE_OK) {
        fprintf(stderr, "Error: page_open failed: %s (%d)\n", error_name(rc), rc);
        page_free(page);
        return 1;
    }
    fprintf(stderr, "Page loaded.\n");

    /* 3. Evaluate JS to get the title */
    char *title_json = NULL;
    size_t title_len = 0;
    rc = page_evaluate(page, "document.title", &title_json, &title_len);
    if (rc == PAGE_OK) {
        fprintf(stderr, "Page title: %s\n", title_json);
        page_string_free(title_json);
    }

    /* 4. Take a screenshot */
    fprintf(stderr, "Taking screenshot...\n");
    uint8_t *png_data = NULL;
    size_t png_len = 0;
    rc = page_screenshot(page, &png_data, &png_len);
    if (rc != PAGE_OK) {
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
        page_buffer_free(png_data, png_len);
    }

    /* 5. Capture HTML */
    fprintf(stderr, "Capturing HTML...\n");
    char *html_data = NULL;
    size_t html_len = 0;
    rc = page_html(page, &html_data, &html_len);
    if (rc != PAGE_OK) {
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
        page_string_free(html_data);
    }

    /* 6. Cleanup */
    page_free(page);
    fprintf(stderr, "Done.\n");
    return 0;
}
