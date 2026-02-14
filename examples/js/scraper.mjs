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
const SCRAPER_OK = 0;
const ERROR_NAMES = {
  0: "OK",
  1: "INIT_FAILED",
  2: "LOAD_FAILED",
  3: "TIMEOUT",
  4: "JS_ERROR",
  5: "SCREENSHOT_FAILED",
  6: "CHANNEL_CLOSED",
  7: "NULL_POINTER",
};

// Load library and define functions
const lib = koffi.load(libPath);

// Opaque pointer type
const ServoScraper = koffi.pointer("ServoScraper", koffi.opaque());

const scraper_new = lib.func(
  "ServoScraper *scraper_new(uint32_t width, uint32_t height, uint64_t timeout, double wait, int fullpage)",
);
const scraper_free = lib.func("void scraper_free(ServoScraper *scraper)");
const scraper_screenshot = lib.func(
  "int scraper_screenshot(ServoScraper *scraper, const char *url, _Out_ uint8_t **out_data, _Out_ size_t *out_len)",
);
const scraper_html = lib.func(
  "int scraper_html(ServoScraper *scraper, const char *url, _Out_ void **out_html, _Out_ size_t *out_len)",
);
const scraper_buffer_free = lib.func(
  "void scraper_buffer_free(void *data, size_t len)",
);
const scraper_string_free = lib.func("void scraper_string_free(void *s)");

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

// 1. Create scraper
console.error("Creating scraper...");
const scraper = scraper_new(1280, 720, 30, 2.0, 0);
if (!scraper) {
  console.error("Error: failed to create scraper");
  process.exit(1);
}
console.error("Scraper created.");

try {
  // 2. Take screenshot
  console.error(`Taking screenshot of ${url}...`);
  const pngDataPtr = [null];
  const pngLen = [0];
  let rc = scraper_screenshot(scraper, url, pngDataPtr, pngLen);
  if (rc !== SCRAPER_OK) {
    console.error(
      `Error: screenshot failed: ${ERROR_NAMES[rc] || "UNKNOWN"} (${rc})`,
    );
  } else {
    const buf = koffi.decode(pngDataPtr[0], koffi.array("uint8_t", pngLen[0]));
    scraper_buffer_free(pngDataPtr[0], pngLen[0]);
    writeFileSync(pngPath, Buffer.from(buf));
    console.error(`Screenshot saved to ${pngPath} (${pngLen[0]} bytes)`);
  }

  // 3. Capture HTML
  console.error(`Capturing HTML of ${url}...`);
  const htmlDataPtr = [null];
  const htmlLen = [0];
  rc = scraper_html(scraper, url, htmlDataPtr, htmlLen);
  if (rc !== SCRAPER_OK) {
    console.error(
      `Error: HTML capture failed: ${ERROR_NAMES[rc] || "UNKNOWN"} (${rc})`,
    );
  } else {
    const rawBuf = koffi.decode(htmlDataPtr[0], koffi.array("uint8_t", htmlLen[0]));
    scraper_string_free(htmlDataPtr[0]);
    const html = Buffer.from(rawBuf).toString("utf-8");
    writeFileSync(htmlPath, html);
    console.error(`HTML saved to ${htmlPath} (${htmlLen[0]} bytes)`);
  }
} finally {
  // 4. Cleanup
  scraper_free(scraper);
  console.error("Done.");
}
