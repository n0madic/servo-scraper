#!/usr/bin/env node
/**
 * servo-scraper Node.js FFI example.
 *
 * Uses node:ffi via koffi to call the shared library (libservo_scraper.dylib / .so).
 *
 * Usage:
 *   node examples/js/scraper.mjs https://example.com /tmp/shot.png /tmp/page.html
 *
 * Requires:
 *   npm install --save-dev koffi   (in examples/js/)
 *   make build-lib                  (to produce the shared library)
 */

import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import process from "node:process";
import koffi from "koffi";

const __dirname = dirname(fileURLToPath(import.meta.url));
const projectRoot = join(__dirname, "..", "..");

// Find the shared library
const libName =
  process.platform === "darwin"
    ? "libservo_scraper.dylib"
    : "libservo_scraper.so";
const libPath = join(projectRoot, "target", "release", libName);

// Error codes
const PAGE_OK = 0;
const ERROR_NAMES = {
  0: "OK",
  1: "INIT_FAILED",
  2: "LOAD_FAILED",
  3: "TIMEOUT",
  4: "JS_ERROR",
  5: "SCREENSHOT_FAILED",
  6: "CHANNEL_CLOSED",
  7: "NULL_POINTER",
  8: "NO_PAGE",
  9: "SELECTOR_NOT_FOUND",
};

// Load library and define functions
const lib = koffi.load(libPath);

// Opaque pointer type
const ServoPage = koffi.pointer("ServoPage", koffi.opaque());

const page_new = lib.func(
  "ServoPage *page_new(uint32_t width, uint32_t height, uint64_t timeout, double wait, int fullpage)",
);
const page_free = lib.func("void page_free(ServoPage *page)");
const page_open = lib.func(
  "int page_open(ServoPage *page, const char *url)",
);
const page_evaluate = lib.func(
  "int page_evaluate(ServoPage *page, const char *script, _Out_ void **out_json, _Out_ size_t *out_len)",
);
const page_screenshot = lib.func(
  "int page_screenshot(ServoPage *page, _Out_ uint8_t **out_data, _Out_ size_t *out_len)",
);
const page_html = lib.func(
  "int page_html(ServoPage *page, _Out_ void **out_html, _Out_ size_t *out_len)",
);
const page_buffer_free = lib.func(
  "void page_buffer_free(void *data, size_t len)",
);
const page_string_free = lib.func("void page_string_free(void *s)");

// CLI
if (process.argv.length < 5) {
  console.error(
    `Usage: node ${process.argv[1]} <URL> <screenshot.png> <output.html>\n` +
      `\nExample:\n` +
      `  node ${process.argv[1]} https://example.com /tmp/shot.png /tmp/page.html`,
  );
  process.exit(1);
}

const [, , url, pngPath, htmlPath] = process.argv;

// 1. Create page
console.error("Creating page...");
const page = page_new(1280, 720, 30, 2.0, 0);
if (!page) {
  console.error("Error: failed to create page");
  process.exit(1);
}
console.error("Page created.");

try {
  // 2. Open URL
  console.error(`Opening ${url}...`);
  let rc = page_open(page, url);
  if (rc !== PAGE_OK) {
    console.error(
      `Error: page_open failed: ${ERROR_NAMES[rc] || "UNKNOWN"} (${rc})`,
    );
    process.exit(1);
  }
  console.error("Page loaded.");

  // 3. Evaluate JS to get the title
  const titlePtr = [null];
  const titleLen = [0];
  rc = page_evaluate(page, "document.title", titlePtr, titleLen);
  if (rc === PAGE_OK) {
    const rawBuf = koffi.decode(titlePtr[0], koffi.array("uint8_t", titleLen[0]));
    page_string_free(titlePtr[0]);
    const title = Buffer.from(rawBuf).toString("utf-8");
    console.error(`Page title: ${title}`);
  }

  // 4. Take screenshot
  console.error("Taking screenshot...");
  const pngDataPtr = [null];
  const pngLen = [0];
  rc = page_screenshot(page, pngDataPtr, pngLen);
  if (rc !== PAGE_OK) {
    console.error(
      `Error: screenshot failed: ${ERROR_NAMES[rc] || "UNKNOWN"} (${rc})`,
    );
  } else {
    const buf = koffi.decode(pngDataPtr[0], koffi.array("uint8_t", pngLen[0]));
    page_buffer_free(pngDataPtr[0], pngLen[0]);
    writeFileSync(pngPath, Buffer.from(buf));
    console.error(`Screenshot saved to ${pngPath} (${pngLen[0]} bytes)`);
  }

  // 5. Capture HTML
  console.error("Capturing HTML...");
  const htmlDataPtr = [null];
  const htmlLen = [0];
  rc = page_html(page, htmlDataPtr, htmlLen);
  if (rc !== PAGE_OK) {
    console.error(
      `Error: HTML capture failed: ${ERROR_NAMES[rc] || "UNKNOWN"} (${rc})`,
    );
  } else {
    const rawBuf = koffi.decode(htmlDataPtr[0], koffi.array("uint8_t", htmlLen[0]));
    page_string_free(htmlDataPtr[0]);
    const html = Buffer.from(rawBuf).toString("utf-8");
    writeFileSync(htmlPath, html);
    console.error(`HTML saved to ${htmlPath} (${htmlLen[0]} bytes)`);
  }
} finally {
  // 6. Cleanup
  page_free(page);
  console.error("Done.");
}
