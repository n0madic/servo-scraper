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

    /// Custom User-Agent string
    #[bpaf(long("user-agent"), argument("STRING"))]
    user_agent: Option<String>,

    /// Wait for network idle (no new requests for N ms) before capturing
    #[bpaf(long("wait-for-network-idle"), argument("MS"))]
    wait_for_network_idle: Option<u64>,

    /// Comma-separated URL patterns to block (e.g. ".png,.jpg,.gif")
    #[bpaf(long("block-urls"), argument("PATTERNS"))]
    block_urls: Option<String>,

    /// Use temporary in-memory storage (clean per-run session)
    #[bpaf(long("temporary-storage"))]
    temporary_storage: bool,

    /// Extra HTTP header for navigation, "Name: Value" (repeatable)
    #[bpaf(long("header"), argument::<String>("HEADER"), many)]
    header: Vec<String>,

    /// Block requests by resource type: image, font, script, stylesheet,
    /// media, document, frame, object, embed, track, worker (repeatable)
    #[bpaf(long("block-resource-type"), argument::<String>("TYPE"), many)]
    block_resource_type: Vec<String>,

    /// Inject a JS file into every page before its scripts run (repeatable)
    #[bpaf(long("init-script"), argument::<String>("PATH"), many)]
    init_script: Vec<String>,

    /// Inject a CSS file into every page as a user stylesheet (repeatable)
    #[bpaf(long("init-style"), argument::<String>("PATH"), many)]
    init_style: Vec<String>,

    /// URL to load
    #[bpaf(positional::<String>("URL"), parse(parse_url))]
    url: Url,
}

/// Parse a `"Name: Value"` header line into a `(name, value)` pair.
fn parse_header(line: &str) -> Result<(String, String), String> {
    let (name, value) = line
        .split_once(':')
        .ok_or_else(|| format!("invalid header (expected 'Name: Value'): {line}"))?;
    Ok((name.trim().to_string(), value.trim().to_string()))
}

/// Read each path into a string, exiting on failure.
fn read_files(paths: &[String], kind: &str) -> Vec<String> {
    paths
        .iter()
        .map(|path| {
            std::fs::read_to_string(path).unwrap_or_else(|e| {
                eprintln!("Error: failed to read {kind} file {path}: {e}");
                process::exit(1);
            })
        })
        .collect()
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

    let headers: Vec<(String, String)> = config
        .header
        .iter()
        .map(|h| {
            parse_header(h).unwrap_or_else(|e| {
                eprintln!("Error: {e}");
                process::exit(1);
            })
        })
        .collect();

    let options = PageOptions {
        width: config.width,
        height: config.height,
        timeout: config.timeout,
        wait: config.wait,
        fullpage: config.fullpage,
        user_agent: config.user_agent.clone(),
        temporary_storage: config.temporary_storage,
        headers,
        init_scripts: read_files(&config.init_script, "init-script"),
        init_stylesheets: read_files(&config.init_style, "init-style"),
    };

    let mut engine = PageEngine::new(options).unwrap_or_else(|e| {
        eprintln!("Error: failed to initialize engine: {e}");
        process::exit(1);
    });

    // If request blocking is requested, create the page explicitly so the
    // patterns/destinations are in place *before* the first navigation.
    if config.block_urls.is_some() || !config.block_resource_type.is_empty() {
        let id = engine.new_page().unwrap_or_else(|e| {
            eprintln!("Error: failed to create page: {e}");
            process::exit(1);
        });
        engine.switch_to(id).unwrap();

        if let Some(ref patterns_str) = config.block_urls {
            let patterns: Vec<String> = patterns_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            engine.block_urls(patterns);
        }

        if !config.block_resource_type.is_empty() {
            engine.block_resource_type_names(&config.block_resource_type);
        }
    }

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

    // Wait for network idle if specified.
    if let Some(idle_ms) = config.wait_for_network_idle {
        eprintln!("Waiting for network idle ({idle_ms}ms)...");
        engine
            .wait_for_network_idle(idle_ms, config.timeout)
            .unwrap_or_else(|e| {
                eprintln!("Error: wait for network idle failed: {e}");
                process::exit(1);
            });
        eprintln!("Network idle achieved.");
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
                        eprintln!("Error: failed to save screenshot: {e}");
                        process::exit(1);
                    }
                    eprintln!("Screenshot saved to {path}");
                } else {
                    match image::load_from_memory(&png_bytes) {
                        Ok(img) => {
                            if let Err(e) = img.save_with_format(path, format) {
                                eprintln!("Error: failed to save screenshot: {e}");
                                process::exit(1);
                            }
                            eprintln!("Screenshot saved to {path}");
                        }
                        Err(e) => {
                            eprintln!("Error: failed to decode screenshot: {e}");
                            process::exit(1);
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
                    eprintln!("Error: failed to write HTML: {e}");
                    process::exit(1);
                }
                eprintln!("HTML saved to {path} ({} bytes)", html.len());
            }
            Err(e) => {
                eprintln!("Error: HTML capture failed: {e}");
                process::exit(1);
            }
        }
    }
}
