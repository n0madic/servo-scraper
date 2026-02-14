/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A minimal headless CLI for web scraping using Servo.
//!
//! Thin wrapper around [`servo_scraper::ScraperEngine`].
//!
//! ```bash
//! servo-scraper --screenshot page.png https://example.com
//! servo-scraper --html page.html https://example.com
//! servo-scraper --screenshot page.png --html page.html --width 1920 --height 1080 https://example.com
//! ```

use std::process;

use bpaf::Bpaf;
use image::ImageFormat;
use log::error;
use servo_scraper::{ScraperEngine, ScraperOptions};
use url::Url;

// ---------------------------------------------------------------------------
// CLI parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Bpaf)]
#[bpaf(options, usage("servo-scraper [OPTIONS] <URL>"))]
struct ScraperConfig {
    /// Save a screenshot to the given file (png, jpg, bmp)
    #[bpaf(long, short, argument("PATH"))]
    screenshot: Option<String>,

    /// Save the page HTML to the given file
    #[bpaf(long, argument("PATH"))]
    html: Option<String>,

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
    let config = scraper_config().run();

    if config.screenshot.is_none() && config.html.is_none() {
        eprintln!("Error: at least one of --screenshot or --html must be specified");
        process::exit(1);
    }

    let options = ScraperOptions {
        width: config.width,
        height: config.height,
        timeout: config.timeout,
        wait: config.wait,
        fullpage: config.fullpage,
    };

    let engine = ScraperEngine::new(options).unwrap_or_else(|e| {
        eprintln!("Error: failed to initialize scraper: {e}");
        process::exit(1);
    });

    eprintln!("Loading {}...", config.url);

    let result = engine
        .scrape(
            config.url.as_str(),
            config.screenshot.is_some(),
            config.html.is_some(),
        )
        .unwrap_or_else(|e| {
            eprintln!("Error: scrape failed: {e}");
            process::exit(1);
        });

    if config.wait > 0.0 {
        eprintln!("Page loaded after {:.1}s settle time.", config.wait);
    }

    // Write screenshot to file.
    if let (Some(path), Some(png_bytes)) = (&config.screenshot, &result.screenshot) {
        let format = ImageFormat::from_path(path).unwrap_or(ImageFormat::Png);
        if format == ImageFormat::Png {
            if let Err(e) = std::fs::write(path, png_bytes) {
                error!("Failed to save screenshot to {path}: {e}");
                eprintln!("Error: failed to save screenshot: {e}");
            } else {
                eprintln!("Screenshot saved to {path}");
            }
        } else {
            // Re-encode from PNG to the requested format.
            match image::load_from_memory(png_bytes) {
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

    // Write HTML to file.
    if let (Some(path), Some(html)) = (&config.html, &result.html) {
        if let Err(e) = std::fs::write(path, html) {
            error!("Failed to write HTML to {path}: {e}");
            eprintln!("Error: failed to write HTML: {e}");
        } else {
            eprintln!("HTML saved to {path} ({} bytes)", html.len());
        }
    }
}
