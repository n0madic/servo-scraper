/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! A library for headless web scraping using the Servo browser engine.
//!
//! Provides two layers:
//!
//! - **[`PageEngine`]** — single-threaded, zero-overhead core. Use directly
//!   from Rust or from the main thread.
//! - **[`Page`]** — thread-safe wrapper (`Send + Sync`). Spawns a background
//!   thread running `PageEngine` and communicates via channels. Safe for FFI
//!   from Python, JavaScript, C, etc.
//!
//! # Example (Rust, direct)
//!
//! ```no_run
//! use servo_scraper::{PageEngine, ScraperOptions};
//!
//! let mut engine = PageEngine::new(ScraperOptions::default()).unwrap();
//! engine.open("https://example.com").unwrap();
//! let html = engine.html().unwrap();
//! let png = engine.screenshot().unwrap();
//! ```
//!
//! # Example (thread-safe / FFI)
//!
//! ```no_run
//! use servo_scraper::{Page, ScraperOptions};
//!
//! let page = Page::new(ScraperOptions::default()).unwrap();
//! page.open("https://example.com").unwrap();
//! let png = page.screenshot().unwrap();
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
use serde::Serialize;
use servo::resources::{self, Resource, ResourceReaderMethods};
use servo::{
    ConsoleLogLevel, DevicePoint, EmbedderControl, EventLoopWaker, InputEvent, JSValue, Key,
    KeyState, KeyboardEvent, LoadStatus, MouseButton, MouseButtonAction, MouseButtonEvent,
    MouseMoveEvent, NamedKey, RenderingContext, Servo, ServoBuilder, SimpleDialog,
    SoftwareRenderingContext, WebResourceLoad, WebView, WebViewBuilder, WebViewDelegate,
    WebViewPoint,
};
use url::Url;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Options for configuring a page session.
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

/// A console message captured from the page.
#[derive(Debug, Clone, Serialize)]
pub struct ConsoleMessage {
    pub level: String,
    pub message: String,
}

/// A network request observed during page loading.
#[derive(Debug, Clone, Serialize)]
pub struct NetworkRequest {
    pub method: String,
    pub url: String,
    pub is_main_frame: bool,
}

/// Errors that can occur during page operations.
#[derive(Debug)]
pub enum ScraperError {
    /// Failed to initialize the engine.
    InitFailed(String),
    /// Failed to load the page.
    LoadFailed(String),
    /// Operation timed out.
    Timeout,
    /// JavaScript evaluation failed.
    JsError(String),
    /// Screenshot capture failed.
    ScreenshotFailed(String),
    /// Internal channel was closed (FFI wrapper).
    ChannelClosed,
    /// No page is open (WebView not created).
    NoPage,
    /// CSS selector matched nothing.
    SelectorNotFound(String),
}

impl fmt::Display for ScraperError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScraperError::InitFailed(msg) => write!(f, "initialization failed: {msg}"),
            ScraperError::LoadFailed(msg) => write!(f, "page load failed: {msg}"),
            ScraperError::Timeout => write!(f, "timed out"),
            ScraperError::JsError(msg) => write!(f, "JavaScript error: {msg}"),
            ScraperError::ScreenshotFailed(msg) => write!(f, "screenshot failed: {msg}"),
            ScraperError::ChannelClosed => write!(f, "internal channel closed"),
            ScraperError::NoPage => write!(f, "no page open"),
            ScraperError::SelectorNotFound(sel) => write!(f, "selector not found: {sel}"),
        }
    }
}

impl std::error::Error for ScraperError {}

// ---------------------------------------------------------------------------
// Internal: Suppress stderr from system libraries
// ---------------------------------------------------------------------------

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
// Internal: Embedded resources
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

#[derive(Clone)]
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

/// Wait until at least one new frame is painted, or timeout.
/// Returns true if a frame was painted, false on timeout.
fn wait_for_frame(
    servo: &Servo,
    event_loop: &ScraperEventLoop,
    delegate: &PageDelegate,
    timeout: Duration,
) -> bool {
    let start = delegate.frame_count.get();
    let deadline = Instant::now() + timeout;
    while delegate.frame_count.get() == start {
        if Instant::now() >= deadline {
            return false;
        }
        event_loop.sleep();
        servo.spin_event_loop();
        event_loop.clear();
    }
    true
}

/// Wait until no new frames arrive for `idle_duration`, or `max_timeout` elapses.
fn wait_for_idle(
    servo: &Servo,
    event_loop: &ScraperEventLoop,
    delegate: &PageDelegate,
    idle_duration: Duration,
    max_timeout: Duration,
) {
    let max_deadline = Instant::now() + max_timeout;
    let mut idle_deadline = Instant::now() + idle_duration;
    let mut last = delegate.frame_count.get();
    loop {
        event_loop.sleep();
        servo.spin_event_loop();
        event_loop.clear();
        let now = Instant::now();
        let current = delegate.frame_count.get();
        if current != last {
            last = current;
            idle_deadline = now + idle_duration;
        }
        if now >= idle_deadline || now >= max_deadline {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: PageDelegate — enhanced WebView delegate
// ---------------------------------------------------------------------------

#[derive(Default)]
struct PageDelegate {
    load_complete: Cell<bool>,
    frame_count: Cell<u64>,
    console_messages: RefCell<Vec<ConsoleMessage>>,
    network_requests: RefCell<Vec<NetworkRequest>>,
}

impl WebViewDelegate for PageDelegate {
    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        if status == LoadStatus::Complete {
            self.load_complete.set(true);
        }
    }

    fn notify_new_frame_ready(&self, webview: WebView) {
        webview.paint();
        self.frame_count.set(self.frame_count.get() + 1);
    }

    fn show_console_message(&self, _webview: WebView, level: ConsoleLogLevel, message: String) {
        let level_str = match level {
            ConsoleLogLevel::Log => "log",
            ConsoleLogLevel::Debug => "debug",
            ConsoleLogLevel::Info => "info",
            ConsoleLogLevel::Warn => "warn",
            ConsoleLogLevel::Error => "error",
            ConsoleLogLevel::Trace => "trace",
        };
        self.console_messages.borrow_mut().push(ConsoleMessage {
            level: level_str.to_string(),
            message,
        });
    }

    fn load_web_resource(&self, _webview: WebView, load: WebResourceLoad) {
        let request = load.request();
        self.network_requests.borrow_mut().push(NetworkRequest {
            method: request.method.to_string(),
            url: request.url.to_string(),
            is_main_frame: request.is_for_main_frame,
        });
        // Don't intercept — let the load continue normally by dropping `load`.
    }

    fn show_embedder_control(&self, _webview: WebView, embedder_control: EmbedderControl) {
        // Auto-dismiss dialogs.
        match embedder_control {
            EmbedderControl::SimpleDialog(dialog) => match dialog {
                SimpleDialog::Alert(alert) => {
                    alert.confirm();
                }
                SimpleDialog::Confirm(confirm) => {
                    confirm.dismiss();
                }
                SimpleDialog::Prompt(prompt) => {
                    prompt.dismiss();
                }
            },
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: capture helpers
// ---------------------------------------------------------------------------

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

/// Serialize a JSValue to a JSON string.
fn jsvalue_to_json(value: &JSValue) -> String {
    match value {
        JSValue::Undefined => "undefined".to_string(),
        JSValue::Null => "null".to_string(),
        JSValue::Boolean(b) => serde_json::to_string(b).unwrap(),
        JSValue::Number(n) => serde_json::to_string(n).unwrap(),
        JSValue::String(s) => serde_json::to_string(s).unwrap(),
        JSValue::Array(arr) => {
            let items: Vec<String> = arr.iter().map(jsvalue_to_json).collect();
            format!("[{}]", items.join(","))
        }
        JSValue::Object(map) => {
            let entries: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap(),
                        jsvalue_to_json(v)
                    )
                })
                .collect();
            format!("{{{}}}", entries.join(","))
        }
        // Element, ShadowRoot, Frame, Window — return as JSON string with type prefix.
        JSValue::Element(id) => serde_json::to_string(&format!("[Element:{id}]")).unwrap(),
        JSValue::ShadowRoot(id) => serde_json::to_string(&format!("[ShadowRoot:{id}]")).unwrap(),
        JSValue::Frame(id) => serde_json::to_string(&format!("[Frame:{id}]")).unwrap(),
        JSValue::Window(id) => serde_json::to_string(&format!("[Window:{id}]")).unwrap(),
    }
}

/// Map a key name string to a `Key`.
fn parse_key_name(name: &str) -> Key {
    match name {
        "Enter" => Key::Named(NamedKey::Enter),
        "Tab" => Key::Named(NamedKey::Tab),
        "Escape" => Key::Named(NamedKey::Escape),
        "Backspace" => Key::Named(NamedKey::Backspace),
        "Delete" => Key::Named(NamedKey::Delete),
        "ArrowUp" => Key::Named(NamedKey::ArrowUp),
        "ArrowDown" => Key::Named(NamedKey::ArrowDown),
        "ArrowLeft" => Key::Named(NamedKey::ArrowLeft),
        "ArrowRight" => Key::Named(NamedKey::ArrowRight),
        "Home" => Key::Named(NamedKey::Home),
        "End" => Key::Named(NamedKey::End),
        "PageUp" => Key::Named(NamedKey::PageUp),
        "PageDown" => Key::Named(NamedKey::PageDown),
        "Space" | " " => Key::Character(" ".into()),
        other => Key::Character(other.into()),
    }
}

// ===========================================================================
// Layer 1: PageEngine (single-threaded, zero overhead)
// ===========================================================================

/// Single-threaded page engine. **Not** `Send` or `Sync`.
///
/// Use this directly from Rust when you control the thread (e.g. from a CLI
/// binary). For FFI or multi-threaded use, see [`Page`].
pub struct PageEngine {
    servo: Servo,
    event_loop: ScraperEventLoop,
    rendering_context: Rc<SoftwareRenderingContext>,
    webview: Option<WebView>,
    delegate: Rc<PageDelegate>,
    options: ScraperOptions,
}

impl PageEngine {
    /// Create a new page engine with the given options.
    pub fn new(options: ScraperOptions) -> Result<Self, ScraperError> {
        resources::set(Box::new(EmbeddedResourceReader));

        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .ok();

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
            webview: None,
            delegate: Rc::new(PageDelegate::default()),
            options,
        })
    }

    fn webview(&self) -> Result<&WebView, ScraperError> {
        self.webview.as_ref().ok_or(ScraperError::NoPage)
    }

    /// Open a URL. Creates a new WebView or navigates the existing one.
    pub fn open(&mut self, url: &str) -> Result<(), ScraperError> {
        let parsed_url =
            Url::parse(url).map_err(|e| ScraperError::LoadFailed(format!("invalid URL: {e}")))?;

        self.delegate.load_complete.set(false);

        if let Some(ref webview) = self.webview {
            webview.load(parsed_url);
        } else {
            let webview = WebViewBuilder::new(&self.servo, self.rendering_context.clone())
                .delegate(self.delegate.clone())
                .url(parsed_url)
                .build();
            self.webview = Some(webview);
        }

        let delegate = self.delegate.clone();
        let loaded = with_stderr_suppressed(|| {
            let loaded = spin_until(
                &self.servo,
                &self.event_loop,
                move || delegate.load_complete.get(),
                self.options.timeout,
            );

            if loaded && self.options.wait > 0.0 {
                wait_for_idle(
                    &self.servo,
                    &self.event_loop,
                    &self.delegate,
                    Duration::from_secs_f64(self.options.wait),
                    Duration::from_secs(self.options.timeout),
                );
            }

            loaded
        });

        if !loaded {
            return Err(ScraperError::Timeout);
        }

        Ok(())
    }

    /// Evaluate JavaScript and return the result as a JSON string.
    pub fn evaluate(&self, script: &str) -> Result<String, ScraperError> {
        let webview = self.webview()?;
        let value = eval_js(
            &self.servo,
            &self.event_loop,
            webview,
            script,
            self.options.timeout,
        )?;
        Ok(jsvalue_to_json(&value))
    }

    /// Take a screenshot of the current viewport (PNG bytes).
    pub fn screenshot(&self) -> Result<Vec<u8>, ScraperError> {
        let webview = self.webview()?;
        take_screenshot_bytes(&self.servo, &self.event_loop, webview, self.options.timeout)
    }

    /// Take a full-page screenshot (PNG bytes).
    pub fn screenshot_fullpage(&self) -> Result<Vec<u8>, ScraperError> {
        let webview = self.webview()?;
        let js = "Math.max(document.documentElement.scrollHeight, document.body.scrollHeight)";
        if let Ok(JSValue::Number(doc_height)) = eval_js(
            &self.servo,
            &self.event_loop,
            webview,
            js,
            self.options.timeout,
        ) {
            let doc_height = doc_height as u32;
            if doc_height > self.options.height {
                let new_size = PhysicalSize::new(self.options.width, doc_height);
                webview.resize(new_size);
                let got_frame = wait_for_frame(
                    &self.servo,
                    &self.event_loop,
                    &self.delegate,
                    Duration::from_secs(self.options.timeout),
                );
                if !got_frame {
                    return Err(ScraperError::ScreenshotFailed(
                        "timed out waiting for repaint after resize".to_string(),
                    ));
                }
                // Wait for layout to stabilize at the new size.
                wait_for_idle(
                    &self.servo,
                    &self.event_loop,
                    &self.delegate,
                    Duration::from_millis(500),
                    Duration::from_secs(5),
                );
            }
        }
        take_screenshot_bytes(&self.servo, &self.event_loop, webview, self.options.timeout)
    }

    /// Capture the page's HTML.
    pub fn html(&self) -> Result<String, ScraperError> {
        let webview = self.webview()?;
        capture_html(&self.servo, &self.event_loop, webview, self.options.timeout)
    }

    /// Get the current page URL.
    pub fn url(&self) -> Option<String> {
        self.webview
            .as_ref()
            .and_then(|wv| wv.url().map(|u| u.to_string()))
    }

    /// Get the current page title.
    pub fn title(&self) -> Option<String> {
        self.webview.as_ref().and_then(|wv| wv.page_title())
    }

    /// Drain and return captured console messages.
    pub fn console_messages(&self) -> Vec<ConsoleMessage> {
        self.delegate
            .console_messages
            .borrow_mut()
            .drain(..)
            .collect()
    }

    /// Drain and return captured network requests.
    pub fn network_requests(&self) -> Vec<NetworkRequest> {
        self.delegate
            .network_requests
            .borrow_mut()
            .drain(..)
            .collect()
    }

    /// Close the current page (drop the WebView).
    pub fn close(&mut self) {
        self.webview = None;
    }

    // -- Phase 2: Wait mechanisms --

    /// Wait until a CSS selector matches an element on the page.
    pub fn wait_for_selector(&self, selector: &str, timeout_secs: u64) -> Result<(), ScraperError> {
        let webview = self.webview()?;
        let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!("document.querySelector('{escaped}') !== null");

        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        loop {
            if let Ok(JSValue::Boolean(true)) =
                eval_js(&self.servo, &self.event_loop, webview, &js, timeout_secs)
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(ScraperError::Timeout);
            }
            wait_for_frame(
                &self.servo,
                &self.event_loop,
                &self.delegate,
                Duration::from_secs(1),
            );
        }
    }

    /// Wait until a JS expression evaluates to a truthy value.
    pub fn wait_for_condition(&self, js_expr: &str, timeout_secs: u64) -> Result<(), ScraperError> {
        let webview = self.webview()?;
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        loop {
            match eval_js(
                &self.servo,
                &self.event_loop,
                webview,
                js_expr,
                timeout_secs,
            ) {
                Ok(JSValue::Boolean(true)) => return Ok(()),
                Ok(JSValue::Number(n)) if n != 0.0 => return Ok(()),
                Ok(JSValue::String(s)) if !s.is_empty() => return Ok(()),
                Ok(JSValue::Array(_)) | Ok(JSValue::Object(_)) => return Ok(()),
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(ScraperError::Timeout);
            }
            wait_for_frame(
                &self.servo,
                &self.event_loop,
                &self.delegate,
                Duration::from_secs(1),
            );
        }
    }

    /// Wait for a fixed duration while keeping the event loop alive.
    pub fn wait(&self, seconds: f64) {
        spin_for(
            &self.servo,
            &self.event_loop,
            Duration::from_secs_f64(seconds),
        );
    }

    /// Wait for the next navigation to complete.
    pub fn wait_for_navigation(&self, timeout_secs: u64) -> Result<(), ScraperError> {
        self.webview()?;
        self.delegate.load_complete.set(false);
        let delegate = self.delegate.clone();
        let loaded = spin_until(
            &self.servo,
            &self.event_loop,
            move || delegate.load_complete.get(),
            timeout_secs,
        );
        if !loaded {
            return Err(ScraperError::Timeout);
        }
        Ok(())
    }

    // -- Phase 3: Input events --

    /// Click at the given device coordinates.
    pub fn click(&self, x: f32, y: f32) -> Result<(), ScraperError> {
        let webview = self.webview()?;
        let point = WebViewPoint::from(DevicePoint::new(x, y));

        webview.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
            MouseButtonAction::Down,
            MouseButton::Left,
            point,
        )));
        webview.notify_input_event(InputEvent::MouseButton(MouseButtonEvent::new(
            MouseButtonAction::Up,
            MouseButton::Left,
            point,
        )));
        wait_for_frame(
            &self.servo,
            &self.event_loop,
            &self.delegate,
            Duration::from_secs(2),
        );

        Ok(())
    }

    /// Click on an element matching a CSS selector.
    pub fn click_selector(&self, selector: &str) -> Result<(), ScraperError> {
        let webview = self.webview()?;
        let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!(
            "(function() {{ \
                var el = document.querySelector('{escaped}'); \
                if (!el) return null; \
                var r = el.getBoundingClientRect(); \
                return [r.left + r.width/2, r.top + r.height/2]; \
            }})()"
        );

        match eval_js(
            &self.servo,
            &self.event_loop,
            webview,
            &js,
            self.options.timeout,
        )? {
            JSValue::Array(coords) if coords.len() == 2 => {
                let x = match &coords[0] {
                    JSValue::Number(n) => *n as f32,
                    _ => return Err(ScraperError::JsError("invalid coordinate".into())),
                };
                let y = match &coords[1] {
                    JSValue::Number(n) => *n as f32,
                    _ => return Err(ScraperError::JsError("invalid coordinate".into())),
                };
                self.click(x, y)
            }
            JSValue::Null | JSValue::Undefined => {
                Err(ScraperError::SelectorNotFound(selector.to_string()))
            }
            other => Err(ScraperError::JsError(format!(
                "unexpected getBoundingClientRect result: {other:?}"
            ))),
        }
    }

    /// Type text by sending individual key events.
    pub fn type_text(&self, text: &str) -> Result<(), ScraperError> {
        let webview = self.webview()?;
        for ch in text.chars() {
            let key = Key::Character(ch.to_string().into());

            webview.notify_input_event(InputEvent::Keyboard(KeyboardEvent::from_state_and_key(
                KeyState::Down,
                key.clone(),
            )));
            webview.notify_input_event(InputEvent::Keyboard(KeyboardEvent::from_state_and_key(
                KeyState::Up,
                key,
            )));
            wait_for_frame(
                &self.servo,
                &self.event_loop,
                &self.delegate,
                Duration::from_secs(2),
            );
        }
        Ok(())
    }

    /// Press a single key by name (e.g. "Enter", "Tab", "a").
    pub fn key_press(&self, key_name: &str) -> Result<(), ScraperError> {
        let webview = self.webview()?;
        let key = parse_key_name(key_name);

        webview.notify_input_event(InputEvent::Keyboard(KeyboardEvent::from_state_and_key(
            KeyState::Down,
            key.clone(),
        )));
        webview.notify_input_event(InputEvent::Keyboard(KeyboardEvent::from_state_and_key(
            KeyState::Up,
            key,
        )));
        wait_for_frame(
            &self.servo,
            &self.event_loop,
            &self.delegate,
            Duration::from_secs(2),
        );

        Ok(())
    }

    /// Move the mouse to the given device coordinates.
    pub fn mouse_move(&self, x: f32, y: f32) -> Result<(), ScraperError> {
        let webview = self.webview()?;
        let point = WebViewPoint::from(DevicePoint::new(x, y));
        webview.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(point)));
        wait_for_frame(
            &self.servo,
            &self.event_loop,
            &self.delegate,
            Duration::from_secs(2),
        );
        Ok(())
    }
}

// ===========================================================================
// Layer 2: Page (thread-safe FFI wrapper)
// ===========================================================================

/// Commands sent from the `Page` handle to the background thread.
enum Command {
    Open {
        url: String,
        response: mpsc::Sender<Result<(), ScraperError>>,
    },
    Evaluate {
        script: String,
        response: mpsc::Sender<Result<String, ScraperError>>,
    },
    Screenshot {
        response: mpsc::Sender<Result<Vec<u8>, ScraperError>>,
    },
    ScreenshotFullpage {
        response: mpsc::Sender<Result<Vec<u8>, ScraperError>>,
    },
    Html {
        response: mpsc::Sender<Result<String, ScraperError>>,
    },
    Url {
        response: mpsc::Sender<Option<String>>,
    },
    Title {
        response: mpsc::Sender<Option<String>>,
    },
    ConsoleMessages {
        response: mpsc::Sender<Vec<ConsoleMessage>>,
    },
    NetworkRequests {
        response: mpsc::Sender<Vec<NetworkRequest>>,
    },
    Close {
        response: mpsc::Sender<()>,
    },
    // Phase 2: Wait commands
    WaitForSelector {
        selector: String,
        timeout: u64,
        response: mpsc::Sender<Result<(), ScraperError>>,
    },
    WaitForCondition {
        js_expr: String,
        timeout: u64,
        response: mpsc::Sender<Result<(), ScraperError>>,
    },
    Wait {
        seconds: f64,
        response: mpsc::Sender<()>,
    },
    WaitForNavigation {
        timeout: u64,
        response: mpsc::Sender<Result<(), ScraperError>>,
    },
    // Phase 3: Input commands
    Click {
        x: f32,
        y: f32,
        response: mpsc::Sender<Result<(), ScraperError>>,
    },
    ClickSelector {
        selector: String,
        response: mpsc::Sender<Result<(), ScraperError>>,
    },
    TypeText {
        text: String,
        response: mpsc::Sender<Result<(), ScraperError>>,
    },
    KeyPress {
        key: String,
        response: mpsc::Sender<Result<(), ScraperError>>,
    },
    MouseMove {
        x: f32,
        y: f32,
        response: mpsc::Sender<Result<(), ScraperError>>,
    },
    Shutdown,
}

/// Thread-safe page handle. `Send + Sync` — safe for FFI.
///
/// Spawns a dedicated background thread running a [`PageEngine`].
/// All Servo logic stays on that thread; callers communicate via channels.
pub struct Page {
    sender: Mutex<mpsc::Sender<Command>>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

unsafe impl Send for Page {}
unsafe impl Sync for Page {}

impl Page {
    /// Create a new thread-safe page handle.
    pub fn new(options: ScraperOptions) -> Result<Self, ScraperError> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
        let (init_tx, init_rx) = mpsc::channel::<Result<(), ScraperError>>();

        let thread = thread::spawn(move || {
            let mut engine = match PageEngine::new(options) {
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
                    Command::Open { url, response } => {
                        let _ = response.send(engine.open(&url));
                    }
                    Command::Evaluate { script, response } => {
                        let _ = response.send(engine.evaluate(&script));
                    }
                    Command::Screenshot { response } => {
                        let _ = response.send(engine.screenshot());
                    }
                    Command::ScreenshotFullpage { response } => {
                        let _ = response.send(engine.screenshot_fullpage());
                    }
                    Command::Html { response } => {
                        let _ = response.send(engine.html());
                    }
                    Command::Url { response } => {
                        let _ = response.send(engine.url());
                    }
                    Command::Title { response } => {
                        let _ = response.send(engine.title());
                    }
                    Command::ConsoleMessages { response } => {
                        let _ = response.send(engine.console_messages());
                    }
                    Command::NetworkRequests { response } => {
                        let _ = response.send(engine.network_requests());
                    }
                    Command::Close { response } => {
                        engine.close();
                        let _ = response.send(());
                    }
                    Command::WaitForSelector {
                        selector,
                        timeout,
                        response,
                    } => {
                        let _ = response.send(engine.wait_for_selector(&selector, timeout));
                    }
                    Command::WaitForCondition {
                        js_expr,
                        timeout,
                        response,
                    } => {
                        let _ = response.send(engine.wait_for_condition(&js_expr, timeout));
                    }
                    Command::Wait { seconds, response } => {
                        engine.wait(seconds);
                        let _ = response.send(());
                    }
                    Command::WaitForNavigation { timeout, response } => {
                        let _ = response.send(engine.wait_for_navigation(timeout));
                    }
                    Command::Click { x, y, response } => {
                        let _ = response.send(engine.click(x, y));
                    }
                    Command::ClickSelector { selector, response } => {
                        let _ = response.send(engine.click_selector(&selector));
                    }
                    Command::TypeText { text, response } => {
                        let _ = response.send(engine.type_text(&text));
                    }
                    Command::KeyPress { key, response } => {
                        let _ = response.send(engine.key_press(&key));
                    }
                    Command::MouseMove { x, y, response } => {
                        let _ = response.send(engine.mouse_move(x, y));
                    }
                    Command::Shutdown => break,
                }
            }
        });

        init_rx
            .recv()
            .map_err(|_| ScraperError::InitFailed("background thread panicked".into()))??;

        Ok(Self {
            sender: Mutex::new(cmd_tx),
            thread: Mutex::new(Some(thread)),
        })
    }

    fn send_cmd<T>(
        &self,
        make_cmd: impl FnOnce(mpsc::Sender<T>) -> Command,
    ) -> Result<T, ScraperError> {
        let (resp_tx, resp_rx) = mpsc::channel();
        let sender = self
            .sender
            .lock()
            .map_err(|_| ScraperError::ChannelClosed)?;
        sender
            .send(make_cmd(resp_tx))
            .map_err(|_| ScraperError::ChannelClosed)?;
        drop(sender);
        resp_rx.recv().map_err(|_| ScraperError::ChannelClosed)
    }

    pub fn open(&self, url: &str) -> Result<(), ScraperError> {
        self.send_cmd(|response| Command::Open {
            url: url.to_string(),
            response,
        })?
    }

    pub fn evaluate(&self, script: &str) -> Result<String, ScraperError> {
        self.send_cmd(|response| Command::Evaluate {
            script: script.to_string(),
            response,
        })?
    }

    pub fn screenshot(&self) -> Result<Vec<u8>, ScraperError> {
        self.send_cmd(|response| Command::Screenshot { response })?
    }

    pub fn screenshot_fullpage(&self) -> Result<Vec<u8>, ScraperError> {
        self.send_cmd(|response| Command::ScreenshotFullpage { response })?
    }

    pub fn html(&self) -> Result<String, ScraperError> {
        self.send_cmd(|response| Command::Html { response })?
    }

    pub fn url(&self) -> Option<String> {
        self.send_cmd(|response| Command::Url { response })
            .ok()
            .flatten()
    }

    pub fn title(&self) -> Option<String> {
        self.send_cmd(|response| Command::Title { response })
            .ok()
            .flatten()
    }

    pub fn console_messages(&self) -> Vec<ConsoleMessage> {
        self.send_cmd(|response| Command::ConsoleMessages { response })
            .unwrap_or_default()
    }

    pub fn network_requests(&self) -> Vec<NetworkRequest> {
        self.send_cmd(|response| Command::NetworkRequests { response })
            .unwrap_or_default()
    }

    pub fn close(&self) {
        let _ = self.send_cmd(|response| Command::Close { response });
    }

    pub fn wait_for_selector(&self, selector: &str, timeout: u64) -> Result<(), ScraperError> {
        self.send_cmd(|response| Command::WaitForSelector {
            selector: selector.to_string(),
            timeout,
            response,
        })?
    }

    pub fn wait_for_condition(&self, js_expr: &str, timeout: u64) -> Result<(), ScraperError> {
        self.send_cmd(|response| Command::WaitForCondition {
            js_expr: js_expr.to_string(),
            timeout,
            response,
        })?
    }

    pub fn wait(&self, seconds: f64) {
        let _ = self.send_cmd(|response| Command::Wait { seconds, response });
    }

    pub fn wait_for_navigation(&self, timeout: u64) -> Result<(), ScraperError> {
        self.send_cmd(|response| Command::WaitForNavigation { timeout, response })?
    }

    pub fn click(&self, x: f32, y: f32) -> Result<(), ScraperError> {
        self.send_cmd(|response| Command::Click { x, y, response })?
    }

    pub fn click_selector(&self, selector: &str) -> Result<(), ScraperError> {
        self.send_cmd(|response| Command::ClickSelector {
            selector: selector.to_string(),
            response,
        })?
    }

    pub fn type_text(&self, text: &str) -> Result<(), ScraperError> {
        self.send_cmd(|response| Command::TypeText {
            text: text.to_string(),
            response,
        })?
    }

    pub fn key_press(&self, key: &str) -> Result<(), ScraperError> {
        self.send_cmd(|response| Command::KeyPress {
            key: key.to_string(),
            response,
        })?
    }

    pub fn mouse_move(&self, x: f32, y: f32) -> Result<(), ScraperError> {
        self.send_cmd(|response| Command::MouseMove { x, y, response })?
    }
}

impl Drop for Page {
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
// Layer 3: C FFI
// ===========================================================================

const SCRAPER_OK: i32 = 0;
const SCRAPER_ERR_INIT: i32 = 1;
const SCRAPER_ERR_LOAD: i32 = 2;
const SCRAPER_ERR_TIMEOUT: i32 = 3;
const SCRAPER_ERR_JS: i32 = 4;
const SCRAPER_ERR_SCREENSHOT: i32 = 5;
const SCRAPER_ERR_CHANNEL: i32 = 6;
const SCRAPER_ERR_NULL_PTR: i32 = 7;
const SCRAPER_ERR_NO_PAGE: i32 = 8;
const SCRAPER_ERR_SELECTOR: i32 = 9;

fn error_code(e: &ScraperError) -> i32 {
    match e {
        ScraperError::InitFailed(_) => SCRAPER_ERR_INIT,
        ScraperError::LoadFailed(_) => SCRAPER_ERR_LOAD,
        ScraperError::Timeout => SCRAPER_ERR_TIMEOUT,
        ScraperError::JsError(_) => SCRAPER_ERR_JS,
        ScraperError::ScreenshotFailed(_) => SCRAPER_ERR_SCREENSHOT,
        ScraperError::ChannelClosed => SCRAPER_ERR_CHANNEL,
        ScraperError::NoPage => SCRAPER_ERR_NO_PAGE,
        ScraperError::SelectorNotFound(_) => SCRAPER_ERR_SELECTOR,
    }
}

// -- Lifecycle --

/// Create a new page instance.
///
/// Returns an opaque pointer, or NULL on failure.
/// The caller must free it with `page_free()`.
///
/// # Safety
///
/// The returned pointer must be freed with `page_free()`.
#[unsafe(no_mangle)]
pub extern "C" fn page_new(
    width: u32,
    height: u32,
    timeout: u64,
    wait: f64,
    fullpage: i32,
) -> *mut Page {
    let options = ScraperOptions {
        width,
        height,
        timeout,
        wait,
        fullpage: fullpage != 0,
    };
    match Page::new(options) {
        Ok(p) => Box::into_raw(Box::new(p)),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Destroy a page instance. Safe to call with NULL.
///
/// # Safety
///
/// `page` must be a valid pointer returned by `page_new()`, or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_free(page: *mut Page) {
    if !page.is_null() {
        unsafe { drop(Box::from_raw(page)) };
    }
}

// -- Navigation --

/// Open a URL in the page.
///
/// # Safety
///
/// `page` must be a valid pointer from `page_new()`. `url` must be a valid C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_open(page: *mut Page, url: *const std::ffi::c_char) -> i32 {
    if page.is_null() || url.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let url_str = match unsafe { std::ffi::CStr::from_ptr(url) }.to_str() {
        Ok(s) => s,
        Err(_) => return SCRAPER_ERR_LOAD,
    };
    match page.open(url_str) {
        Ok(()) => SCRAPER_OK,
        Err(e) => error_code(&e),
    }
}

// -- Capture --

/// Evaluate JavaScript and return the result as a JSON string.
///
/// On success, `*out_json` is set to a heap-allocated null-terminated string
/// and `*out_len` to its length. Free with `page_string_free()`.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_evaluate(
    page: *mut Page,
    script: *const std::ffi::c_char,
    out_json: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || script.is_null() || out_json.is_null() || out_len.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let script_str = match unsafe { std::ffi::CStr::from_ptr(script) }.to_str() {
        Ok(s) => s,
        Err(_) => return SCRAPER_ERR_JS,
    };
    match page.evaluate(script_str) {
        Ok(json) => match std::ffi::CString::new(json) {
            Ok(cstr) => {
                let len = cstr.as_bytes().len();
                let ptr = cstr.into_raw();
                unsafe {
                    *out_json = ptr;
                    *out_len = len;
                }
                SCRAPER_OK
            }
            Err(_) => SCRAPER_ERR_JS,
        },
        Err(e) => error_code(&e),
    }
}

/// Take a screenshot. Returns PNG bytes.
///
/// On success, `*out_data` and `*out_len` are set. Free with `page_buffer_free()`.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_screenshot(
    page: *mut Page,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_data.is_null() || out_len.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.screenshot() {
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

/// Take a full-page screenshot. Returns PNG bytes.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_screenshot_fullpage(
    page: *mut Page,
    out_data: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_data.is_null() || out_len.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.screenshot_fullpage() {
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

/// Capture the page HTML.
///
/// On success, `*out_html` and `*out_len` are set. Free with `page_string_free()`.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_html(
    page: *mut Page,
    out_html: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_html.is_null() || out_len.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.html() {
        Ok(html) => match std::ffi::CString::new(html) {
            Ok(cstr) => {
                let len = cstr.as_bytes().len();
                let ptr = cstr.into_raw();
                unsafe {
                    *out_html = ptr;
                    *out_len = len;
                }
                SCRAPER_OK
            }
            Err(_) => SCRAPER_ERR_JS,
        },
        Err(e) => error_code(&e),
    }
}

// -- Page info --

/// Get the current page URL.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_url(
    page: *mut Page,
    out_url: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_url.is_null() || out_len.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.url() {
        Some(url_str) => match std::ffi::CString::new(url_str) {
            Ok(cstr) => {
                let len = cstr.as_bytes().len();
                let ptr = cstr.into_raw();
                unsafe {
                    *out_url = ptr;
                    *out_len = len;
                }
                SCRAPER_OK
            }
            Err(_) => SCRAPER_ERR_JS,
        },
        None => SCRAPER_ERR_NO_PAGE,
    }
}

/// Get the current page title.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_title(
    page: *mut Page,
    out_title: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_title.is_null() || out_len.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.title() {
        Some(title_str) => match std::ffi::CString::new(title_str) {
            Ok(cstr) => {
                let len = cstr.as_bytes().len();
                let ptr = cstr.into_raw();
                unsafe {
                    *out_title = ptr;
                    *out_len = len;
                }
                SCRAPER_OK
            }
            Err(_) => SCRAPER_ERR_JS,
        },
        None => SCRAPER_ERR_NO_PAGE,
    }
}

// -- Events (JSON) --

/// Get console messages as a JSON array.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_console_messages(
    page: *mut Page,
    out_json: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_json.is_null() || out_len.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let msgs = page.console_messages();
    let json = serde_json::to_string(&msgs).unwrap_or_else(|_| "[]".to_string());
    match std::ffi::CString::new(json) {
        Ok(cstr) => {
            let len = cstr.as_bytes().len();
            let ptr = cstr.into_raw();
            unsafe {
                *out_json = ptr;
                *out_len = len;
            }
            SCRAPER_OK
        }
        Err(_) => SCRAPER_ERR_JS,
    }
}

/// Get network requests as a JSON array.
///
/// # Safety
///
/// All pointer arguments must be valid or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_network_requests(
    page: *mut Page,
    out_json: *mut *mut std::ffi::c_char,
    out_len: *mut usize,
) -> i32 {
    if page.is_null() || out_json.is_null() || out_len.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let reqs = page.network_requests();
    let json = serde_json::to_string(&reqs).unwrap_or_else(|_| "[]".to_string());
    match std::ffi::CString::new(json) {
        Ok(cstr) => {
            let len = cstr.as_bytes().len();
            let ptr = cstr.into_raw();
            unsafe {
                *out_json = ptr;
                *out_len = len;
            }
            SCRAPER_OK
        }
        Err(_) => SCRAPER_ERR_JS,
    }
}

// -- Wait FFI --

/// Wait for a CSS selector to match an element.
///
/// # Safety
///
/// `page` and `selector` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_wait_for_selector(
    page: *mut Page,
    selector: *const std::ffi::c_char,
    timeout_secs: u64,
) -> i32 {
    if page.is_null() || selector.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let sel = match unsafe { std::ffi::CStr::from_ptr(selector) }.to_str() {
        Ok(s) => s,
        Err(_) => return SCRAPER_ERR_JS,
    };
    match page.wait_for_selector(sel, timeout_secs) {
        Ok(()) => SCRAPER_OK,
        Err(e) => error_code(&e),
    }
}

/// Wait for a JS expression to evaluate to a truthy value.
///
/// # Safety
///
/// `page` and `js_expr` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_wait_for_condition(
    page: *mut Page,
    js_expr: *const std::ffi::c_char,
    timeout_secs: u64,
) -> i32 {
    if page.is_null() || js_expr.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let expr = match unsafe { std::ffi::CStr::from_ptr(js_expr) }.to_str() {
        Ok(s) => s,
        Err(_) => return SCRAPER_ERR_JS,
    };
    match page.wait_for_condition(expr, timeout_secs) {
        Ok(()) => SCRAPER_OK,
        Err(e) => error_code(&e),
    }
}

/// Wait for a fixed number of seconds.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_wait(page: *mut Page, seconds: f64) -> i32 {
    if page.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    page.wait(seconds);
    SCRAPER_OK
}

/// Wait for the next navigation to complete.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_wait_for_navigation(page: *mut Page, timeout_secs: u64) -> i32 {
    if page.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.wait_for_navigation(timeout_secs) {
        Ok(()) => SCRAPER_OK,
        Err(e) => error_code(&e),
    }
}

// -- Input FFI --

/// Click at the given coordinates.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_click(page: *mut Page, x: f32, y: f32) -> i32 {
    if page.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.click(x, y) {
        Ok(()) => SCRAPER_OK,
        Err(e) => error_code(&e),
    }
}

/// Click on an element matching a CSS selector.
///
/// # Safety
///
/// `page` and `selector` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_click_selector(
    page: *mut Page,
    selector: *const std::ffi::c_char,
) -> i32 {
    if page.is_null() || selector.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let sel = match unsafe { std::ffi::CStr::from_ptr(selector) }.to_str() {
        Ok(s) => s,
        Err(_) => return SCRAPER_ERR_JS,
    };
    match page.click_selector(sel) {
        Ok(()) => SCRAPER_OK,
        Err(e) => error_code(&e),
    }
}

/// Type text by sending individual key events.
///
/// # Safety
///
/// `page` and `text` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_type_text(page: *mut Page, text: *const std::ffi::c_char) -> i32 {
    if page.is_null() || text.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let text_str = match unsafe { std::ffi::CStr::from_ptr(text) }.to_str() {
        Ok(s) => s,
        Err(_) => return SCRAPER_ERR_JS,
    };
    match page.type_text(text_str) {
        Ok(()) => SCRAPER_OK,
        Err(e) => error_code(&e),
    }
}

/// Press a single key by name (e.g. "Enter", "Tab", "a").
///
/// # Safety
///
/// `page` and `key_name` must be valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_key_press(page: *mut Page, key_name: *const std::ffi::c_char) -> i32 {
    if page.is_null() || key_name.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    let name = match unsafe { std::ffi::CStr::from_ptr(key_name) }.to_str() {
        Ok(s) => s,
        Err(_) => return SCRAPER_ERR_JS,
    };
    match page.key_press(name) {
        Ok(()) => SCRAPER_OK,
        Err(e) => error_code(&e),
    }
}

/// Move the mouse to the given coordinates.
///
/// # Safety
///
/// `page` must be a valid pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_mouse_move(page: *mut Page, x: f32, y: f32) -> i32 {
    if page.is_null() {
        return SCRAPER_ERR_NULL_PTR;
    }
    let page = unsafe { &*page };
    match page.mouse_move(x, y) {
        Ok(()) => SCRAPER_OK,
        Err(e) => error_code(&e),
    }
}

// -- Memory --

/// Free a buffer returned by `page_screenshot()` or `page_screenshot_fullpage()`.
///
/// # Safety
///
/// `data` must be a pointer returned by a page screenshot function, or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_buffer_free(data: *mut u8, len: usize) {
    if !data.is_null() && len > 0 {
        unsafe {
            let slice = std::slice::from_raw_parts_mut(data, len);
            drop(Box::from_raw(slice));
        }
    }
}

/// Free a string returned by page FFI functions.
///
/// # Safety
///
/// `s` must be a pointer returned by a page string function, or NULL.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn page_string_free(s: *mut std::ffi::c_char) {
    if !s.is_null() {
        unsafe { drop(std::ffi::CString::from_raw(s)) };
    }
}
