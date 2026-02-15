/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A minimal headless CLI for web scraping using Servo.
//!
//! Thin wrapper around [`servo_scraper::PageEngine`].
//!
//! ```bash
//! servo-scraper --screenshot page.png https://example.com
//! servo-scraper --html page.html https://example.com
//! servo-scraper --eval "document.title" https://example.com
//! servo-scraper --eval-file script.js https://example.com
//! servo-scraper --wait-for "h1" --screenshot page.png https://example.com
//! ```

use std::process;

use bpaf::Bpaf;
use image::ImageFormat;
use log::error;
use servo_scraper::{PageEngine, PageOptions};
use url::Url;

// ---------------------------------------------------------------------------
// CLI parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Bpaf)]
#[bpaf(options, usage("servo-scraper [OPTIONS] <URL>"))]
struct CliConfig {
    /// Save a screenshot to the given file (png, jpg, bmp)
    #[bpaf(long, short, argument("PATH"))]
    screenshot: Option<String>,

    /// Save the page HTML to the given file
    #[bpaf(long, argument("PATH"))]
    html: Option<String>,

    /// Evaluate JavaScript and print result (JSON) to stdout
    #[bpaf(long, argument("JS"))]
    eval: Option<String>,

    /// Evaluate JavaScript from a file and print result (JSON) to stdout
    #[bpaf(long("eval-file"), argument("PATH"))]
    eval_file: Option<String>,

    /// Wait for a CSS selector before capturing
    #[bpaf(long("wait-for"), argument("SELECTOR"))]
    wait_for: Option<String>,

    /// Viewport width in pixels
    #[bpaf(long, argument("PIXELS"), fallback(1280u32))]
    width: u32,

    /// Viewport height in pixels
    #[bpaf(long, argument("PIXELS"), fallback(720u32))]
    height: u32,

    /// Maximum time to wait for page load
    #[bpaf(long, argument("SECONDS"), fallback(30u64))]
    timeout: u64,

    /// Extra time after load for JS to settle
    #[bpaf(long, argument("SECONDS"), fallback(2.0f64))]
    wait: f64,

    /// Capture the full scrollable page, not just the viewport
    #[bpaf(long, short)]
    fullpage: bool,

    /// URL to load
    #[bpaf(positional::<String>("URL"), parse(parse_url))]
    url: Url,
}

fn parse_url(s: String) -> Result<Url, String> {
    Url::parse(&s).map_err(|e| format!("Invalid URL: {e}"))
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let config = cli_config().run();

    if config.screenshot.is_none()
        && config.html.is_none()
        && config.eval.is_none()
        && config.eval_file.is_none()
    {
        eprintln!(
            "Error: at least one of --screenshot, --html, --eval, or --eval-file must be specified"
        );
        process::exit(1);
    }

    let options = PageOptions {
        width: config.width,
        height: config.height,
        timeout: config.timeout,
        wait: config.wait,
        fullpage: config.fullpage,
    };

    let mut engine = PageEngine::new(options).unwrap_or_else(|e| {
        eprintln!("Error: failed to initialize engine: {e}");
        process::exit(1);
    });

    eprintln!("Loading {}...", config.url);

    engine.open(config.url.as_str()).unwrap_or_else(|e| {
        eprintln!("Error: page load failed: {e}");
        process::exit(1);
    });

    if config.wait > 0.0 {
        eprintln!("Page loaded after {:.1}s settle time.", config.wait);
    }

    // Wait for selector if specified.
    if let Some(ref selector) = config.wait_for {
        eprintln!("Waiting for selector: {selector}");
        engine
            .wait_for_selector(selector, config.timeout)
            .unwrap_or_else(|e| {
                eprintln!("Error: wait for selector failed: {e}");
                process::exit(1);
            });
        eprintln!("Selector found.");
    }

    // Evaluate JS if specified.
    if let Some(ref script) = config.eval {
        match engine.evaluate(script) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("Error: JS evaluation failed: {e}");
                process::exit(1);
            }
        }
    }

    // Evaluate JS from file if specified.
    if let Some(ref path) = config.eval_file {
        let script = std::fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("Error: failed to read JS file {path}: {e}");
            process::exit(1);
        });
        match engine.evaluate(&script) {
            Ok(json) => println!("{json}"),
            Err(e) => {
                eprintln!("Error: JS evaluation failed: {e}");
                process::exit(1);
            }
        }
    }

    // Write screenshot to file.
    if let Some(ref path) = config.screenshot {
        let screenshot_result = if config.fullpage {
            engine.screenshot_fullpage()
        } else {
            engine.screenshot()
        };

        match screenshot_result {
            Ok(png_bytes) => {
                let format = ImageFormat::from_path(path).unwrap_or(ImageFormat::Png);
                if format == ImageFormat::Png {
                    if let Err(e) = std::fs::write(path, &png_bytes) {
                        error!("Failed to save screenshot to {path}: {e}");
                        eprintln!("Error: failed to save screenshot: {e}");
                    } else {
                        eprintln!("Screenshot saved to {path}");
                    }
                } else {
                    match image::load_from_memory(&png_bytes) {
                        Ok(img) => {
                            if let Err(e) = img.save_with_format(path, format) {
                                error!("Failed to save screenshot to {path}: {e}");
                                eprintln!("Error: failed to save screenshot: {e}");
                            } else {
                                eprintln!("Screenshot saved to {path}");
                            }
                        }
                        Err(e) => {
                            error!("Failed to decode PNG for re-encoding: {e}");
                            eprintln!("Error: failed to decode screenshot: {e}");
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("Error: screenshot failed: {e}");
                process::exit(1);
            }
        }
    }

    // Write HTML to file.
    if let Some(ref path) = config.html {
        match engine.html() {
            Ok(html) => {
                if let Err(e) = std::fs::write(path, &html) {
                    error!("Failed to write HTML to {path}: {e}");
                    eprintln!("Error: failed to write HTML: {e}");
                } else {
                    eprintln!("HTML saved to {path} ({} bytes)", html.len());
                }
            }
            Err(e) => {
                eprintln!("Error: HTML capture failed: {e}");
                process::exit(1);
            }
        }
    }
}
