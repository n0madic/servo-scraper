/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A minimal headless utility for web scraping using Servo.
//!
//! Supports capturing screenshots and/or HTML content from web pages.
//!
//! ```bash
//! servo-scraper --screenshot page.png https://example.com
//! servo-scraper --html page.html https://example.com
//! servo-scraper --screenshot page.png --html page.html --width 1920 --height 1080 https://example.com
//! ```

use std::cell::{Cell, RefCell};
use std::process;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use image::{DynamicImage, ImageFormat};
use log::error;
use servo::{
    EventLoopWaker, JSValue, JavaScriptEvaluationError, LoadStatus, RenderingContext, Servo,
    ServoBuilder, SoftwareRenderingContext, WebView, WebViewBuilder, WebViewDelegate,
};
use url::Url;

// ---------------------------------------------------------------------------
// CLI parsing
// ---------------------------------------------------------------------------

struct ScraperConfig {
    url: Url,
    screenshot_path: Option<String>,
    html_path: Option<String>,
    width: u32,
    height: u32,
    timeout_secs: u64,
    wait_secs: f64,
    fullpage: bool,
}

fn print_usage() {
    eprintln!(
        "Usage: servo-scraper [OPTIONS] <URL>\n\
         \n\
         Options:\n\
         \x20 --screenshot <PATH>  Save a screenshot to the given file (png, jpg, bmp, etc.)\n\
         \x20 --html <PATH>        Save the page HTML to the given file\n\
         \x20 --width <PIXELS>     Viewport width  (default: 1280)\n\
         \x20 --height <PIXELS>    Viewport height (default: 720)\n\
         \x20 --timeout <SECONDS>  Maximum time to wait for page load (default: 30)\n\
         \x20 --wait <SECONDS>     Extra time after load for JS to settle (default: 2.0)\n\
         \x20 --fullpage           Capture the full scrollable page, not just the viewport\n\
         \x20 --help               Show this help message"
    );
}

fn parse_args() -> ScraperConfig {
    let mut args = std::env::args().skip(1).peekable();
    let mut screenshot_path = None;
    let mut html_path = None;
    let mut width: u32 = 1280;
    let mut height: u32 = 720;
    let mut timeout_secs: u64 = 30;
    let mut wait_secs: f64 = 2.0;
    let mut fullpage = false;
    let mut url_string = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                process::exit(0);
            },
            "--screenshot" => {
                screenshot_path = Some(
                    args.next()
                        .unwrap_or_else(|| die("--screenshot requires a file path")),
                );
            },
            "--html" => {
                html_path = Some(
                    args.next()
                        .unwrap_or_else(|| die("--html requires a file path")),
                );
            },
            "--width" => {
                width = args
                    .next()
                    .unwrap_or_else(|| die("--width requires a number"))
                    .parse()
                    .unwrap_or_else(|_| die("--width must be a positive integer"));
            },
            "--height" => {
                height = args
                    .next()
                    .unwrap_or_else(|| die("--height requires a number"))
                    .parse()
                    .unwrap_or_else(|_| die("--height must be a positive integer"));
            },
            "--timeout" => {
                timeout_secs = args
                    .next()
                    .unwrap_or_else(|| die("--timeout requires a number"))
                    .parse()
                    .unwrap_or_else(|_| die("--timeout must be a positive integer"));
            },
            "--wait" => {
                wait_secs = args
                    .next()
                    .unwrap_or_else(|| die("--wait requires a number"))
                    .parse()
                    .unwrap_or_else(|_| die("--wait must be a number (seconds)"));
            },
            "--fullpage" => {
                fullpage = true;
            },
            other if other.starts_with('-') => {
                die(&format!("Unknown option: {other}"));
            },
            _ => {
                url_string = Some(arg);
            },
        }
    }

    let url_string = url_string.unwrap_or_else(|| {
        print_usage();
        die("A URL argument is required")
    });

    let url = Url::parse(&url_string).unwrap_or_else(|e| die(&format!("Invalid URL: {e}")));

    if screenshot_path.is_none() && html_path.is_none() {
        die("At least one of --screenshot or --html must be specified");
    }

    ScraperConfig {
        url,
        screenshot_path,
        html_path,
        width,
        height,
        timeout_secs,
        wait_secs,
        fullpage,
    }
}

fn die(msg: &str) -> ! {
    eprintln!("Error: {msg}");
    process::exit(1);
}

// ---------------------------------------------------------------------------
// Event loop (condvar-based, pattern from servoshell HeadlessEventLoop)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ScraperEventLoop {
    flag: Arc<Mutex<bool>>,
    condvar: Condvar,
}

impl ScraperEventLoop {
    fn create_waker(&self) -> Box<dyn EventLoopWaker> {
        Box::new(ScraperWaker {
            flag: self.flag.clone(),
            condvar: &self.condvar as *const Condvar as usize,
        })
    }

    fn sleep(&self) {
        let guard = self.flag.lock().unwrap();
        if *guard {
            return;
        }
        let _ = self
            .condvar
            .wait_timeout(guard, Duration::from_millis(5))
            .unwrap();
    }

    fn clear(&self) {
        *self.flag.lock().unwrap() = false;
    }
}

/// The waker needs to be Send + Sync. We store a raw pointer-as-usize
/// for the Condvar so the struct is Send. This is safe because the event
/// loop (and its Condvar) always outlives the waker in our usage.
#[derive(Clone)]
struct ScraperWaker {
    flag: Arc<Mutex<bool>>,
    condvar: usize,
}

// Safety: The Condvar lives on the stack of `main`, which outlives all
// servo threads that hold a clone of this waker.
unsafe impl Send for ScraperWaker {}
unsafe impl Sync for ScraperWaker {}

impl EventLoopWaker for ScraperWaker {
    fn wake(&self) {
        let mut flag = self.flag.lock().unwrap();
        *flag = true;
        // Safety: see above — the Condvar pointer is valid for the
        // lifetime of the program.
        let condvar = unsafe { &*(self.condvar as *const Condvar) };
        condvar.notify_all();
    }

    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }
}

/// Spin the Servo event loop until `done` returns true, or `timeout_secs` elapses.
fn spin_until(
    servo: &Servo,
    event_loop: &ScraperEventLoop,
    done: impl Fn() -> bool,
    timeout_secs: u64,
) {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while !done() {
        if Instant::now() >= deadline {
            eprintln!("Warning: timed out after {timeout_secs}s waiting for page load");
            break;
        }
        event_loop.sleep();
        servo.spin_event_loop();
        event_loop.clear();
    }
}

/// Keep spinning the event loop for `duration`, allowing JS to continue executing.
fn spin_for(servo: &Servo, event_loop: &ScraperEventLoop, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        event_loop.sleep();
        servo.spin_event_loop();
        event_loop.clear();
    }
}

// ---------------------------------------------------------------------------
// WebView delegate
// ---------------------------------------------------------------------------

#[derive(Default)]
struct ScraperDelegate {
    load_complete: Cell<bool>,
}

impl WebViewDelegate for ScraperDelegate {
    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        if status == LoadStatus::Complete {
            self.load_complete.set(true);
        }
    }

    fn notify_new_frame_ready(&self, webview: WebView) {
        // Paint is required so that screenshots contain actual content.
        webview.paint();
    }
}

// ---------------------------------------------------------------------------
// Capture helpers
// ---------------------------------------------------------------------------

/// Evaluate JavaScript synchronously and return the result.
fn eval_js(
    servo: &Servo,
    event_loop: &ScraperEventLoop,
    webview: &WebView,
    script: &str,
) -> Option<Result<JSValue, JavaScriptEvaluationError>> {
    let result: Rc<RefCell<Option<Result<JSValue, JavaScriptEvaluationError>>>> =
        Rc::new(RefCell::new(None));
    let cb_result = result.clone();

    webview.evaluate_javascript(script, move |value| {
        *cb_result.borrow_mut() = Some(value);
    });

    spin_until(servo, event_loop, || result.borrow().is_some(), 30);
    result.borrow_mut().take()
}

fn take_screenshot_to_file(
    servo: &Servo,
    event_loop: &ScraperEventLoop,
    webview: &WebView,
    path: &str,
) {
    let result: Rc<RefCell<Option<Result<servo::RgbaImage, _>>>> = Rc::new(RefCell::new(None));
    let cb_result = result.clone();

    webview.take_screenshot(None, move |image| {
        *cb_result.borrow_mut() = Some(image);
    });

    spin_until(servo, event_loop, || result.borrow().is_some(), 30);

    let image_result = result.borrow_mut().take();
    match image_result {
        Some(Ok(image)) => {
            let format = ImageFormat::from_path(path).unwrap_or(ImageFormat::Png);
            if let Err(e) = DynamicImage::ImageRgba8(image).save_with_format(path, format) {
                error!("Failed to save screenshot to {path}: {e}");
                eprintln!("Error: failed to save screenshot: {e}");
            } else {
                eprintln!("Screenshot saved to {path}");
            }
        },
        Some(Err(e)) => {
            error!("Screenshot capture failed: {e:?}");
            eprintln!("Error: screenshot capture failed: {e:?}");
        },
        None => {
            eprintln!("Error: screenshot callback was never called (timeout)");
        },
    }
}

fn capture_html_to_file(
    servo: &Servo,
    event_loop: &ScraperEventLoop,
    webview: &WebView,
    path: &str,
) {
    let js_result = eval_js(servo, event_loop, webview, "document.documentElement.outerHTML");
    match js_result {
        Some(Ok(JSValue::String(html))) => {
            if let Err(e) = std::fs::write(path, &html) {
                error!("Failed to write HTML to {path}: {e}");
                eprintln!("Error: failed to write HTML: {e}");
            } else {
                eprintln!("HTML saved to {path} ({} bytes)", html.len());
            }
        },
        Some(Ok(other)) => {
            eprintln!("Error: unexpected JS result type: {other:?}");
        },
        Some(Err(e)) => {
            error!("JavaScript evaluation failed: {e:?}");
            eprintln!("Error: JavaScript evaluation failed: {e:?}");
        },
        None => {
            eprintln!("Error: JavaScript callback was never called (timeout)");
        },
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let config = parse_args();

    // 1. Crypto init — required for any HTTPS request.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install crypto provider");

    // 2. Event loop + waker (condvar-based, no display server needed).
    let event_loop = ScraperEventLoop::default();
    let waker = event_loop.create_waker();

    // 3. Software rendering context (headless, no GPU).
    let rendering_context = Rc::new(
        SoftwareRenderingContext::new(PhysicalSize::new(config.width, config.height))
            .expect("Failed to create SoftwareRenderingContext"),
    );
    assert!(
        rendering_context.make_current().is_ok(),
        "Failed to make rendering context current"
    );

    // 4. Build Servo.
    let servo = ServoBuilder::default()
        .event_loop_waker(waker)
        .build();
    servo.setup_logging();

    // 5. Create WebView with URL and delegate.
    let delegate = Rc::new(ScraperDelegate::default());
    let webview = WebViewBuilder::new(&servo, rendering_context.clone())
        .delegate(delegate.clone())
        .url(config.url.clone())
        .build();

    eprintln!("Loading {}...", config.url);

    // 6. Wait for the page to finish loading.
    let d = delegate.clone();
    spin_until(
        &servo,
        &event_loop,
        move || d.load_complete.get(),
        config.timeout_secs,
    );

    // 7. Let JS settle after load event (async scripts, requestAnimationFrame, etc.).
    if config.wait_secs > 0.0 {
        eprintln!(
            "Page loaded, waiting {:.1}s for JS to settle...",
            config.wait_secs
        );
        spin_for(
            &servo,
            &event_loop,
            Duration::from_secs_f64(config.wait_secs),
        );
    }

    // 8. For full-page screenshots, resize viewport to full document height.
    if config.fullpage && config.screenshot_path.is_some() {
        let js = "Math.max(document.documentElement.scrollHeight, document.body.scrollHeight)";
        if let Some(Ok(JSValue::Number(doc_height))) =
            eval_js(&servo, &event_loop, &webview, js)
        {
            let doc_height = doc_height as u32;
            if doc_height > config.height {
                eprintln!("Resizing viewport to {0}x{doc_height} for full-page capture...", config.width);
                webview.resize(PhysicalSize::new(config.width, doc_height));
                // Let the page re-layout and repaint at the new size.
                spin_for(&servo, &event_loop, Duration::from_secs(1));
            }
        }
    }

    // 9. Capture results.
    if let Some(ref path) = config.screenshot_path {
        take_screenshot_to_file(&servo, &event_loop, &webview, path);
    }
    if let Some(ref path) = config.html_path {
        capture_html_to_file(&servo, &event_loop, &webview, path);
    }

    // 10. Cleanup is automatic via Drop on WebView and Servo.
    drop(webview);
    drop(servo);
}
