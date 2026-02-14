/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A library for headless web scraping using the Servo browser engine.
//!
//! Provides two layers:
//!
//! - **[`ScraperEngine`]** — single-threaded, zero-overhead core. Use directly
//!   from Rust or from the main thread.
//! - **[`Scraper`]** — thread-safe wrapper (`Send + Sync`). Spawns a background
//!   thread running `ScraperEngine` and communicates via channels. Safe for FFI
//!   from Python, JavaScript, C, etc.
//!
//! # Example (Rust, direct)
//!
//! ```no_run
//! use servo_scraper::{ScraperEngine, ScraperOptions};
//!
//! let engine = ScraperEngine::new(ScraperOptions::default()).unwrap();
//! let result = engine.scrape("https://example.com", true, true).unwrap();
//! assert!(result.screenshot.is_some());
//! assert!(result.html.is_some());
//! ```
//!
//! # Example (thread-safe / FFI)
//!
//! ```no_run
//! use servo_scraper::{Scraper, ScraperOptions};
//!
//! let scraper = Scraper::new(ScraperOptions::default()).unwrap();
//! let png_bytes = scraper.screenshot("https://example.com").unwrap();
//! ```

use std::cell::{Cell, RefCell};
use std::fmt;
use std::os::fd::{AsRawFd, IntoRawFd};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use image::codecs::png::PngEncoder;
use image::{DynamicImage, ImageEncoder};
use servo::resources::{self, Resource, ResourceReaderMethods};
use servo::{
    EventLoopWaker, JSValue, LoadStatus, RenderingContext, Servo, ServoBuilder,
    SoftwareRenderingContext, WebView, WebViewBuilder, WebViewDelegate,
};
use url::Url;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Options for configuring a scraping session.
#[derive(Debug, Clone)]
pub struct ScraperOptions {
    /// Viewport width in pixels (default: 1280).
    pub width: u32,
    /// Viewport height in pixels (default: 720).
    pub height: u32,
    /// Maximum time in seconds to wait for page load (default: 30).
    pub timeout: u64,
    /// Extra time in seconds after load for JS to settle (default: 2.0).
    pub wait: f64,
    /// Capture the full scrollable page, not just the viewport (default: false).
    pub fullpage: bool,
}

impl Default for ScraperOptions {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            timeout: 30,
            wait: 2.0,
            fullpage: false,
        }
    }
}

/// The result of a scrape operation.
#[derive(Debug, Clone)]
pub struct ScrapeResult {
    /// PNG-encoded screenshot bytes, if requested.
    pub screenshot: Option<Vec<u8>>,
    /// HTML content of the page, if requested.
    pub html: Option<String>,
}

/// Errors that can occur during scraping.
#[derive(Debug)]
pub enum ScraperError {
    /// Failed to initialize the scraping engine.
    InitFailed(String),
    /// Failed to load the page.
    LoadFailed(String),
    /// Page load timed out.
    Timeout,
    /// JavaScript evaluation failed.
    JsError(String),
    /// Screenshot capture failed.
    ScreenshotFailed(String),
    /// Internal channel was closed (FFI wrapper).
    ChannelClosed,
}

impl fmt::Display for ScraperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScraperError::InitFailed(msg) => write!(f, "initialization failed: {msg}"),
            ScraperError::LoadFailed(msg) => write!(f, "page load failed: {msg}"),
            ScraperError::Timeout => write!(f, "timed out waiting for page load"),
            ScraperError::JsError(msg) => write!(f, "JavaScript error: {msg}"),
            ScraperError::ScreenshotFailed(msg) => write!(f, "screenshot failed: {msg}"),
            ScraperError::ChannelClosed => write!(f, "internal channel closed"),
        }
    }
}

impl std::error::Error for ScraperError {}

// ---------------------------------------------------------------------------
// Internal: Suppress stderr from system libraries
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
// Internal: Embedded resources (self-contained binary, no external files)
// ---------------------------------------------------------------------------

struct EmbeddedResourceReader;

impl ResourceReaderMethods for EmbeddedResourceReader {
    fn read(&self, res: Resource) -> Vec<u8> {
        match res {
            Resource::BluetoothBlocklist => {
                include_bytes!("../servo/resources/gatt_blocklist.txt").to_vec()
            }
            Resource::DomainList => {
                include_bytes!("../servo/resources/public_domains.txt").to_vec()
            }
            Resource::HstsPreloadList => {
                include_bytes!("../servo/resources/hsts_preload.fstmap").to_vec()
            }
            Resource::BadCertHTML => include_bytes!("../servo/resources/badcert.html").to_vec(),
            Resource::NetErrorHTML => include_bytes!("../servo/resources/neterror.html").to_vec(),
            Resource::BrokenImageIcon => include_bytes!("../servo/resources/rippy.png").to_vec(),
            Resource::CrashHTML => include_bytes!("../servo/resources/crash.html").to_vec(),
            Resource::DirectoryListingHTML => {
                include_bytes!("../servo/resources/directory-listing.html").to_vec()
            }
            Resource::AboutMemoryHTML => {
                include_bytes!("../servo/resources/about-memory.html").to_vec()
            }
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
// Internal: Event loop (condvar-based)
// ---------------------------------------------------------------------------

struct ScraperEventLoop {
    flag: Arc<Mutex<bool>>,
    condvar: Arc<Condvar>,
}

impl Default for ScraperEventLoop {
    fn default() -> Self {
        Self {
            flag: Arc::new(Mutex::new(false)),
            condvar: Arc::new(Condvar::new()),
        }
    }
}

impl ScraperEventLoop {
    fn create_waker(&self) -> Box<dyn EventLoopWaker> {
        Box::new(ScraperWaker {
            flag: self.flag.clone(),
            condvar: self.condvar.clone(),
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

#[derive(Clone)]
struct ScraperWaker {
    flag: Arc<Mutex<bool>>,
    condvar: Arc<Condvar>,
}

impl EventLoopWaker for ScraperWaker {
    fn wake(&self) {
        let mut flag = self.flag.lock().unwrap();
        *flag = true;
        self.condvar.notify_all();
    }

    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }
}

/// Spin the Servo event loop until `done` returns true, or `timeout_secs` elapses.
/// Returns `true` if the condition was met, `false` on timeout.
fn spin_until(
    servo: &Servo,
    event_loop: &ScraperEventLoop,
    done: impl Fn() -> bool,
    timeout_secs: u64,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    while !done() {
        if Instant::now() >= deadline {
            return false;
        }
        event_loop.sleep();
        servo.spin_event_loop();
        event_loop.clear();
    }
    true
}

/// Keep spinning the event loop for `duration`.
fn spin_for(servo: &Servo, event_loop: &ScraperEventLoop, duration: Duration) {
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        event_loop.sleep();
        servo.spin_event_loop();
        event_loop.clear();
    }
}

// ---------------------------------------------------------------------------
// Internal: WebView delegate
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
// Internal: capture helpers
// ---------------------------------------------------------------------------

/// Evaluate JavaScript synchronously and return the result.
fn eval_js(
    servo: &Servo,
    event_loop: &ScraperEventLoop,
    webview: &WebView,
    script: &str,
    timeout_secs: u64,
) -> Result<JSValue, ScraperError> {
    let result: Rc<RefCell<Option<Result<JSValue, servo::JavaScriptEvaluationError>>>> =
        Rc::new(RefCell::new(None));
    let cb_result = result.clone();

    webview.evaluate_javascript(script, move |value| {
        *cb_result.borrow_mut() = Some(value);
    });

    let completed = spin_until(
        servo,
        event_loop,
        || result.borrow().is_some(),
        timeout_secs,
    );
    if !completed {
        return Err(ScraperError::Timeout);
    }

    match result.borrow_mut().take() {
        Some(Ok(value)) => Ok(value),
        Some(Err(e)) => Err(ScraperError::JsError(format!("{e:?}"))),
        None => Err(ScraperError::Timeout),
    }
}

/// Take a screenshot and return PNG bytes.
fn take_screenshot_bytes(
    servo: &Servo,
    event_loop: &ScraperEventLoop,
    webview: &WebView,
    timeout_secs: u64,
) -> Result<Vec<u8>, ScraperError> {
    let result: Rc<RefCell<Option<Result<servo::RgbaImage, _>>>> = Rc::new(RefCell::new(None));
    let cb_result = result.clone();

    webview.take_screenshot(None, move |image| {
        *cb_result.borrow_mut() = Some(image);
    });

    let completed = spin_until(
        servo,
        event_loop,
        || result.borrow().is_some(),
        timeout_secs,
    );
    if !completed {
        return Err(ScraperError::Timeout);
    }

    match result.borrow_mut().take() {
        Some(Ok(image)) => {
            let dynamic = DynamicImage::ImageRgba8(image);
            let rgba8 = dynamic.to_rgba8();
            let (w, h) = (rgba8.width(), rgba8.height());
            let mut png_buf = Vec::new();
            PngEncoder::new(&mut png_buf)
                .write_image(&rgba8, w, h, image::ExtendedColorType::Rgba8)
                .map_err(|e| ScraperError::ScreenshotFailed(format!("PNG encoding failed: {e}")))?;
            Ok(png_buf)
        }
        Some(Err(e)) => Err(ScraperError::ScreenshotFailed(format!("{e:?}"))),
        None => Err(ScraperError::Timeout),
    }
}

/// Capture the page's HTML via JavaScript.
fn capture_html(
    servo: &Servo,
    event_loop: &ScraperEventLoop,
    webview: &WebView,
    timeout_secs: u64,
) -> Result<String, ScraperError> {
    match eval_js(
        servo,
        event_loop,
        webview,
        "document.documentElement.outerHTML",
        timeout_secs,
    )? {
        JSValue::String(html) => Ok(html),
        other => Err(ScraperError::JsError(format!(
            "unexpected JS result type: {other:?}"
        ))),
    }
}

// ===========================================================================
// Layer 1: ScraperEngine (single-threaded, zero overhead)
// ===========================================================================

/// Single-threaded scraping engine. **Not** `Send` or `Sync`.
///
/// Use this directly from Rust when you control the thread (e.g. from a CLI
/// binary). For FFI or multi-threaded use, see [`Scraper`].
pub struct ScraperEngine {
    servo: Servo,
    event_loop: ScraperEventLoop,
    rendering_context: Rc<SoftwareRenderingContext>,
    options: ScraperOptions,
}

impl ScraperEngine {
    /// Create a new scraping engine with the given options.
    ///
    /// This sets up embedded resources, initializes crypto, creates a software
    /// rendering context, and builds the Servo instance. Must be called on the
    /// thread that will drive the event loop.
    pub fn new(options: ScraperOptions) -> Result<Self, ScraperError> {
        // Embedded resources — must be set before Servo reads them.
        resources::set(Box::new(EmbeddedResourceReader));

        // Crypto init — required for HTTPS.
        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .ok(); // Ignore error if already installed.

        let event_loop = ScraperEventLoop::default();
        let waker = event_loop.create_waker();

        let rendering_context = Rc::new(
            SoftwareRenderingContext::new(PhysicalSize::new(options.width, options.height))
                .map_err(|e| ScraperError::InitFailed(format!("rendering context: {e:?}")))?,
        );
        rendering_context
            .make_current()
            .map_err(|e| ScraperError::InitFailed(format!("make_current: {e:?}")))?;

        let servo = ServoBuilder::default().event_loop_waker(waker).build();
        servo.setup_logging();

        Ok(Self {
            servo,
            event_loop,
            rendering_context,
            options,
        })
    }

    /// Scrape a URL, optionally capturing a screenshot and/or HTML.
    ///
    /// Returns a [`ScrapeResult`] with the requested data. Stderr is
    /// suppressed during rendering to hide system library noise (e.g.
    /// macOS OpenGL diagnostics).
    pub fn scrape(
        &self,
        url: &str,
        want_screenshot: bool,
        want_html: bool,
    ) -> Result<ScrapeResult, ScraperError> {
        let parsed_url =
            Url::parse(url).map_err(|e| ScraperError::LoadFailed(format!("invalid URL: {e}")))?;

        let delegate = Rc::new(ScraperDelegate::default());
        let webview = WebViewBuilder::new(&self.servo, self.rendering_context.clone())
            .delegate(delegate.clone())
            .url(parsed_url)
            .build();

        // Wait for load to complete (suppress stderr during rendering).
        let d = delegate.clone();
        let loaded = with_stderr_suppressed(|| {
            let loaded = spin_until(
                &self.servo,
                &self.event_loop,
                move || d.load_complete.get(),
                self.options.timeout,
            );

            // Let JS settle.
            if loaded && self.options.wait > 0.0 {
                spin_for(
                    &self.servo,
                    &self.event_loop,
                    Duration::from_secs_f64(self.options.wait),
                );
            }

            loaded
        });

        if !loaded {
            drop(webview);
            return Err(ScraperError::Timeout);
        }

        // Full-page resize if needed.
        if self.options.fullpage && want_screenshot {
            let js = "Math.max(document.documentElement.scrollHeight, document.body.scrollHeight)";
            if let Ok(JSValue::Number(doc_height)) = eval_js(
                &self.servo,
                &self.event_loop,
                &webview,
                js,
                self.options.timeout,
            ) {
                let doc_height = doc_height as u32;
                if doc_height > self.options.height {
                    let new_size = PhysicalSize::new(self.options.width, doc_height);
                    self.rendering_context.resize(new_size);
                    webview.resize(new_size);
                    spin_for(&self.servo, &self.event_loop, Duration::from_secs(1));
                }
            }
        }

        // Capture results.
        let screenshot = if want_screenshot {
            Some(take_screenshot_bytes(
                &self.servo,
                &self.event_loop,
                &webview,
                self.options.timeout,
            )?)
        } else {
            None
        };

        let html = if want_html {
            Some(capture_html(
                &self.servo,
                &self.event_loop,
                &webview,
                self.options.timeout,
            )?)
        } else {
            None
        };

        drop(webview);
        Ok(ScrapeResult { screenshot, html })
    }

    /// Convenience: capture only a screenshot (PNG bytes).
    pub fn screenshot(&self, url: &str) -> Result<Vec<u8>, ScraperError> {
        self.scrape(url, true, false)?
            .screenshot
            .ok_or_else(|| ScraperError::ScreenshotFailed("no screenshot data".into()))
    }

    /// Convenience: capture only the page HTML.
    pub fn html(&self, url: &str) -> Result<String, ScraperError> {
        self.scrape(url, false, true)?
            .html
            .ok_or_else(|| ScraperError::JsError("no HTML data".into()))
    }
}

// ===========================================================================
// Layer 2: Scraper (thread-safe FFI wrapper)
// ===========================================================================

/// Commands sent from the `Scraper` handle to the background thread.
enum Command {
    Scrape {
        url: String,
        want_screenshot: bool,
        want_html: bool,
        response: mpsc::Sender<Result<ScrapeResult, ScraperError>>,
    },
    Shutdown,
}

/// Thread-safe scraping handle. `Send + Sync` — safe for FFI.
///
/// Spawns a dedicated background thread running a [`ScraperEngine`].
/// All Servo logic stays on that thread; callers communicate via channels.
pub struct Scraper {
    sender: Mutex<mpsc::Sender<Command>>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

// Explicitly mark as Send + Sync — the Mutex<mpsc::Sender> is Send+Sync,
// and the JoinHandle is Send.
unsafe impl Send for Scraper {}
unsafe impl Sync for Scraper {}

impl Scraper {
    /// Create a new thread-safe scraper.
    ///
    /// Spawns a background thread that initializes a [`ScraperEngine`] and
    /// processes commands until [`Drop`].
    pub fn new(options: ScraperOptions) -> Result<Self, ScraperError> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
        let (init_tx, init_rx) = mpsc::channel::<Result<(), ScraperError>>();

        let thread = thread::spawn(move || {
            let engine = match ScraperEngine::new(options) {
                Ok(engine) => {
                    let _ = init_tx.send(Ok(()));
                    engine
                }
                Err(e) => {
                    let _ = init_tx.send(Err(e));
                    return;
                }
            };

            while let Ok(cmd) = cmd_rx.recv() {
                match cmd {
                    Command::Scrape {
                        url,
                        want_screenshot,
                        want_html,
                        response,
                    } => {
                        let result = engine.scrape(&url, want_screenshot, want_html);
                        let _ = response.send(result);
                    }
                    Command::Shutdown => break,
                }
            }
        });

        // Wait for initialization to complete.
        init_rx
            .recv()
            .map_err(|_| ScraperError::InitFailed("background thread panicked".into()))??;

        Ok(Self {
            sender: Mutex::new(cmd_tx),
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Scrape a URL, optionally capturing a screenshot and/or HTML.
    pub fn scrape(
        &self,
        url: &str,
        want_screenshot: bool,
        want_html: bool,
    ) -> Result<ScrapeResult, ScraperError> {
        let (resp_tx, resp_rx) = mpsc::channel();
        let sender = self
            .sender
            .lock()
            .map_err(|_| ScraperError::ChannelClosed)?;
        sender
            .send(Command::Scrape {
                url: url.to_string(),
                want_screenshot,
                want_html,
                response: resp_tx,
            })
            .map_err(|_| ScraperError::ChannelClosed)?;
        drop(sender);

        resp_rx.recv().map_err(|_| ScraperError::ChannelClosed)?
    }

    /// Convenience: capture only a screenshot (PNG bytes).
    pub fn screenshot(&self, url: &str) -> Result<Vec<u8>, ScraperError> {
        self.scrape(url, true, false)?
            .screenshot
            .ok_or_else(|| ScraperError::ScreenshotFailed("no screenshot data".into()))
    }

    /// Convenience: capture only the page HTML.
    pub fn html(&self, url: &str) -> Result<String, ScraperError> {
        self.scrape(url, false, true)?
            .html
            .ok_or_else(|| ScraperError::JsError("no HTML data".into()))
    }
}

impl Drop for Scraper {
    fn drop(&mut self) {
        if let Ok(sender) = self.sender.lock() {
            let _ = sender.send(Command::Shutdown);
        }
        if let Ok(mut handle) = self.thread.lock() {
            if let Some(thread) = handle.take() {
                let _ = thread.join();
            }
        }
    }
}

// ===========================================================================
// Layer 3: C FFI — extern "C" functions for Python/JS/C consumers
// ===========================================================================

/// Error codes returned by C FFI functions.
const SCRAPER_OK: i32 = 0;
const SCRAPER_ERR_INIT: i32 = 1;
const SCRAPER_ERR_LOAD: i32 = 2;
const SCRAPER_ERR_TIMEOUT: i32 = 3;
const SCRAPER_ERR_JS: i32 = 4;
const SCRAPER_ERR_SCREENSHOT: i32 = 5;
const SCRAPER_ERR_CHANNEL: i32 = 6;
const SCRAPER_ERR_NULL_PTR: i32 = 7;

fn error_code(e: &ScraperError) -> i32 {
    match e {
        ScraperError::InitFailed(_) => SCRAPER_ERR_INIT,
        ScraperError::LoadFailed(_) => SCRAPER_ERR_LOAD,
        ScraperError::Timeout => SCRAPER_ERR_TIMEOUT,
        ScraperError::JsError(_) => SCRAPER_ERR_JS,
        ScraperError::ScreenshotFailed(_) => SCRAPER_ERR_SCREENSHOT,
        ScraperError::ChannelClosed => SCRAPER_ERR_CHANNEL,
    }
}

/// Create a new thread-safe scraper instance.
///
/// Returns an opaque pointer, or NULL on failure.
/// The caller must free it with `scraper_free()`.
#[unsafe(no_mangle)]
pub extern "C" fn scraper_new(
    width: u32,
    height: u32,
    timeout: u64,
    wait: f64,
    fullpage: i32,
) -> *mut Scraper {
    let options = ScraperOptions {
        width,
        height,
        timeout,
        wait,
        fullpage: fullpage != 0,
    };
    match Scraper::new(options) {
        Ok(s) => Box::into_raw(Box::new(s)),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Destroy a scraper instance. Safe to call with NULL.
#[unsafe(no_mangle)]
pub extern "C" fn scraper_free(scraper: *mut Scraper) {
    if !scraper.is_null() {
        unsafe {
            drop(Box::from_raw(scraper));
        }
    }
}

/// Take a screenshot of a URL. Returns PNG bytes.
///
/// On success, `*out_data` is set to a heap-allocated buffer and `*out_len`
/// to its length. The caller must free it with `scraper_buffer_free()`.
///
/// Returns `SCRAPER_OK` (0) on success, or an error code.
#[unsafe(no_mangle)]
pub extern "C" fn scraper_screenshot(
    scraper: *mut Scraper,
    url: *const std::ffi::c_char,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    if scraper.is_null() || url.is_null() || out_data.is_null() || out_len.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }

    let scraper = unsafe { &*scraper };
    let url_str = unsafe { std::ffi::CStr::from_ptr(url) };
    let url_str = match url_str.to_str() {
        Ok(s) => s,
        Err(_) => return SCRAPER_ERR_LOAD,
    };

    match scraper.screenshot(url_str) {
        Ok(png_bytes) => {
            let boxed = png_bytes.into_boxed_slice();
            let len = boxed.len();
            let ptr = Box::into_raw(boxed) as *mut u8;
            unsafe {
                *out_data = ptr;
                *out_len = len;
            }
            SCRAPER_OK
        }
        Err(e) => error_code(&e),
    }
}

/// Capture the HTML of a URL.
///
/// On success, `*out_html` is set to a heap-allocated null-terminated string
/// and `*out_len` to its length (excluding the null terminator).
/// The caller must free it with `scraper_string_free()`.
///
/// Returns `SCRAPER_OK` (0) on success, or an error code.
#[unsafe(no_mangle)]
pub extern "C" fn scraper_html(
    scraper: *mut Scraper,
    url: *const std::ffi::c_char,
    out_html: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if scraper.is_null() || url.is_null() || out_html.is_null() || out_len.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }

    let scraper = unsafe { &*scraper };
    let url_str = unsafe { std::ffi::CStr::from_ptr(url) };
    let url_str = match url_str.to_str() {
        Ok(s) => s,
        Err(_) => return SCRAPER_ERR_LOAD,
    };

    match scraper.html(url_str) {
        Ok(html) => {
            match std::ffi::CString::new(html) {
                Ok(cstr) => {
                    let bytes = cstr.as_bytes(); // without null terminator
                    let len = bytes.len();
                    let ptr = cstr.into_raw();
                    unsafe {
                        *out_html = ptr;
                        *out_len = len;
                    }
                    SCRAPER_OK
                }
                Err(_) => SCRAPER_ERR_JS, // HTML contained interior null bytes
            }
        }
        Err(e) => error_code(&e),
    }
}

/// Free a buffer returned by `scraper_screenshot()`. Safe to call with NULL.
#[unsafe(no_mangle)]
pub extern "C" fn scraper_buffer_free(data: *mut u8, len: usize) {
    if !data.is_null() && len > 0 {
        unsafe {
            let slice = std::slice::from_raw_parts_mut(data, len);
            drop(Box::from_raw(slice));
        }
    }
}

/// Free a string returned by `scraper_html()`. Safe to call with NULL.
#[unsafe(no_mangle)]
pub extern "C" fn scraper_string_free(s: *mut std::ffi::c_char) {
    if !s.is_null() {
        unsafe {
            drop(std::ffi::CString::from_raw(s));
        }
    }
}
