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
use std::os::fd::{AsRawFd, IntoRawFd};
use std::path::PathBuf;
use std::process;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use bpaf::Bpaf;
use dpi::PhysicalSize;
use image::{DynamicImage, ImageFormat};
use log::error;
use servo::resources::{self, Resource, ResourceReaderMethods};
use servo::{
    EventLoopWaker, JSValue, JavaScriptEvaluationError, LoadStatus, RenderingContext, Servo,
    ServoBuilder, SoftwareRenderingContext, WebView, WebViewBuilder, WebViewDelegate,
};
use url::Url;

// ---------------------------------------------------------------------------
// Suppress stderr from system libraries (e.g. macOS OpenGL diagnostics)
// ---------------------------------------------------------------------------

/// Temporarily redirects stderr to /dev/null for the duration of the closure.
/// This suppresses noise from system libraries (e.g. Apple's OpenGL "UNSUPPORTED"
/// warnings) that write directly to fd 2 rather than going through Rust's log system.
fn with_stderr_suppressed<T>(f: impl FnOnce() -> T) -> T {
    unsafe {
        let stderr_fd = std::io::stderr().as_raw_fd();
        let saved = libc::dup(stderr_fd);
        if saved >= 0 {
            let devnull = std::fs::File::open("/dev/null")
                .map(|f| f.into_raw_fd())
                .unwrap_or(-1);
            if devnull >= 0 {
                libc::dup2(devnull, stderr_fd);
                libc::close(devnull);
                let result = f();
                libc::dup2(saved, stderr_fd);
                libc::close(saved);
                return result;
            }
            libc::close(saved);
        }
    }
    f()
}

// ---------------------------------------------------------------------------
// Embedded resources (so the binary is self-contained, no external files needed)
// ---------------------------------------------------------------------------

struct EmbeddedResourceReader;

impl ResourceReaderMethods for EmbeddedResourceReader {
    fn read(&self, res: Resource) -> Vec<u8> {
        match res {
            Resource::BluetoothBlocklist => {
                include_bytes!("../servo/resources/gatt_blocklist.txt").to_vec()
            },
            Resource::DomainList => {
                include_bytes!("../servo/resources/public_domains.txt").to_vec()
            },
            Resource::HstsPreloadList => {
                include_bytes!("../servo/resources/hsts_preload.fstmap").to_vec()
            },
            Resource::BadCertHTML => include_bytes!("../servo/resources/badcert.html").to_vec(),
            Resource::NetErrorHTML => include_bytes!("../servo/resources/neterror.html").to_vec(),
            Resource::BrokenImageIcon => include_bytes!("../servo/resources/rippy.png").to_vec(),
            Resource::CrashHTML => include_bytes!("../servo/resources/crash.html").to_vec(),
            Resource::DirectoryListingHTML => {
                include_bytes!("../servo/resources/directory-listing.html").to_vec()
            },
            Resource::AboutMemoryHTML => {
                include_bytes!("../servo/resources/about-memory.html").to_vec()
            },
            Resource::DebuggerJS => include_bytes!("../servo/resources/debugger.js").to_vec(),
        }
    }
    fn sandbox_access_files(&self) -> Vec<PathBuf> {
        vec![]
    }
    fn sandbox_access_files_dirs(&self) -> Vec<PathBuf> {
        vec![]
    }
}

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
    let config = scraper_config().run();

    if config.screenshot.is_none() && config.html.is_none() {
        eprintln!("Error: at least one of --screenshot or --html must be specified");
        process::exit(1);
    }

    // 1. Embedded resources — must be set before Servo reads them.
    resources::set(Box::new(EmbeddedResourceReader));

    // 2. Crypto init — required for any HTTPS request.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install crypto provider");

    // 3. Event loop + waker (condvar-based, no display server needed).
    let event_loop = ScraperEventLoop::default();
    let waker = event_loop.create_waker();

    // 4. Software rendering context (headless, no GPU).
    let rendering_context = Rc::new(
        SoftwareRenderingContext::new(PhysicalSize::new(config.width, config.height))
            .expect("Failed to create SoftwareRenderingContext"),
    );
    assert!(
        rendering_context.make_current().is_ok(),
        "Failed to make rendering context current"
    );

    // 5. Build Servo.
    let servo = ServoBuilder::default()
        .event_loop_waker(waker)
        .build();
    servo.setup_logging();

    // 6. Create WebView with URL and delegate.
    let delegate = Rc::new(ScraperDelegate::default());
    let webview = WebViewBuilder::new(&servo, rendering_context.clone())
        .delegate(delegate.clone())
        .url(config.url.clone())
        .build();

    eprintln!("Loading {}...", config.url);

    // 7. Wait for the page to finish loading, then let JS settle.
    //    Suppress stderr during rendering to hide OpenGL diagnostics
    //    ("UNSUPPORTED ... GLD_TEXTURE_INDEX_2D") which are harmless but noisy.
    let d = delegate.clone();
    with_stderr_suppressed(|| {
        spin_until(
            &servo,
            &event_loop,
            move || d.load_complete.get(),
            config.timeout,
        );

        // 8. Let JS settle after load event (async scripts, requestAnimationFrame, etc.).
        if config.wait > 0.0 {
            spin_for(
                &servo,
                &event_loop,
                Duration::from_secs_f64(config.wait),
            );
        }
    });
    if config.wait > 0.0 {
        eprintln!("Page loaded after {:.1}s settle time.", config.wait);
    }

    // 9. For full-page screenshots, resize viewport to full document height.
    if config.fullpage && config.screenshot.is_some() {
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

    // 10. Capture results.
    if let Some(ref path) = config.screenshot {
        take_screenshot_to_file(&servo, &event_loop, &webview, path);
    }
    if let Some(ref path) = config.html {
        capture_html_to_file(&servo, &event_loop, &webview, path);
    }

    // 11. Cleanup is automatic via Drop on WebView and Servo.
    drop(webview);
    drop(servo);
}
