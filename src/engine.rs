/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Layer 1: `PageEngine` — single-threaded, zero-overhead core.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::fd::{AsRawFd, IntoRawFd};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use image::codecs::png::PngEncoder;
use image::{DynamicImage, ImageEncoder};
use servo::resources::{self, Resource, ResourceReaderMethods};
use servo::{
    ConsoleLogLevel, CreateNewWebViewRequest, DevicePoint, EmbedderControl, EventLoopWaker,
    InputEvent, JSValue, Key, KeyState, KeyboardEvent, LoadStatus, MouseButton, MouseButtonAction,
    MouseButtonEvent, MouseMoveEvent, NamedKey, Preferences, RenderingContext, Servo, ServoBuilder,
    SimpleDialog, SoftwareRenderingContext, WebResourceLoad, WebResourceResponse, WebView,
    WebViewBuilder, WebViewDelegate, WebViewPoint, WheelDelta, WheelEvent, WheelMode,
};
use url::Url;

use crate::types::{
    ConsoleMessage, ElementRect, InputFile, NetworkRequest, PageError, PageOptions,
};

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

/// Wait until no new network requests arrive for `idle_duration`, or `max_timeout` elapses.
/// Returns true if idle was achieved, false on timeout.
fn wait_for_network_idle_inner(
    servo: &Servo,
    event_loop: &ScraperEventLoop,
    delegate: &PageDelegate,
    idle_duration: Duration,
    max_timeout: Duration,
) -> bool {
    let max_deadline = Instant::now() + max_timeout;
    let mut idle_deadline = Instant::now() + idle_duration;
    let mut last_seen = delegate.last_request_time.get();
    loop {
        event_loop.sleep();
        servo.spin_event_loop();
        event_loop.clear();
        let now = Instant::now();
        let current = delegate.last_request_time.get();
        if current != last_seen {
            last_seen = current;
            idle_deadline = now + idle_duration;
        }
        if now >= idle_deadline {
            return true;
        }
        if now >= max_deadline {
            return false;
        }
    }
}

// ---------------------------------------------------------------------------
// Internal: PageDelegate — enhanced WebView delegate
// ---------------------------------------------------------------------------

/// A popup WebView buffered until the engine drains it via `popup_pages()`.
struct PendingPopup {
    webview: WebView,
    rendering_context: Rc<SoftwareRenderingContext>,
    delegate: Rc<PageDelegate>,
}

struct PageDelegate {
    load_complete: Cell<bool>,
    frame_count: Cell<u64>,
    last_request_time: Cell<Option<Instant>>,
    console_messages: RefCell<Vec<ConsoleMessage>>,
    network_requests: RefCell<Vec<NetworkRequest>>,
    blocked_url_patterns: RefCell<Vec<String>>,
    closed: Cell<bool>,
    popup_buffer: Rc<RefCell<Vec<PendingPopup>>>,
    popup_enabled: Rc<Cell<bool>>,
    default_width: Cell<u32>,
    default_height: Cell<u32>,
}

impl PageDelegate {
    fn new(
        popup_buffer: Rc<RefCell<Vec<PendingPopup>>>,
        popup_enabled: Rc<Cell<bool>>,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            load_complete: Cell::new(false),
            frame_count: Cell::new(0),
            last_request_time: Cell::new(None),
            console_messages: RefCell::new(Vec::new()),
            network_requests: RefCell::new(Vec::new()),
            blocked_url_patterns: RefCell::new(Vec::new()),
            closed: Cell::new(false),
            popup_buffer,
            popup_enabled,
            default_width: Cell::new(width),
            default_height: Cell::new(height),
        }
    }
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
        let url_str = request.url.to_string();
        self.network_requests.borrow_mut().push(NetworkRequest {
            method: request.method.to_string(),
            url: url_str.clone(),
            is_main_frame: request.is_for_main_frame,
        });
        self.last_request_time.set(Some(Instant::now()));

        // Check if URL matches any blocked pattern.
        let blocked = self
            .blocked_url_patterns
            .borrow()
            .iter()
            .any(|pattern| url_str.contains(pattern));

        if blocked {
            let response = WebResourceResponse::new(request.url.clone());
            load.intercept(response).cancel();
        }
        // Otherwise drop `load` to let it continue normally.
    }

    fn show_embedder_control(&self, _webview: WebView, embedder_control: EmbedderControl) {
        // Auto-dismiss dialogs.
        if let EmbedderControl::SimpleDialog(dialog) = embedder_control {
            match dialog {
                SimpleDialog::Alert(alert) => {
                    alert.confirm();
                }
                SimpleDialog::Confirm(confirm) => {
                    confirm.dismiss();
                }
                SimpleDialog::Prompt(prompt) => {
                    prompt.dismiss();
                }
            }
        }
    }

    fn notify_closed(&self, _webview: WebView) {
        self.closed.set(true);
    }

    fn request_create_new(&self, _parent: WebView, request: CreateNewWebViewRequest) {
        if !self.popup_enabled.get() {
            // Drop request to block popup.
            return;
        }

        let w = self.default_width.get();
        let h = self.default_height.get();

        let rendering_context = match SoftwareRenderingContext::new(PhysicalSize::new(w, h)) {
            Ok(ctx) => Rc::new(ctx),
            Err(_) => return, // Failed — drop request to block popup.
        };
        if rendering_context.make_current().is_err() {
            return;
        }

        let delegate = Rc::new(PageDelegate::new(
            self.popup_buffer.clone(),
            self.popup_enabled.clone(),
            w,
            h,
        ));

        let webview = request
            .builder(rendering_context.clone())
            .delegate(delegate.clone())
            .build();

        self.popup_buffer.borrow_mut().push(PendingPopup {
            webview,
            rendering_context,
            delegate,
        });
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
) -> Result<JSValue, PageError> {
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
        return Err(PageError::Timeout);
    }

    match result.borrow_mut().take() {
        Some(Ok(value)) => Ok(value),
        Some(Err(e)) => Err(PageError::JsError(format!("{e:?}"))),
        None => Err(PageError::Timeout),
    }
}

fn take_screenshot_bytes(
    servo: &Servo,
    event_loop: &ScraperEventLoop,
    webview: &WebView,
    timeout_secs: u64,
) -> Result<Vec<u8>, PageError> {
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
        return Err(PageError::Timeout);
    }

    match result.borrow_mut().take() {
        Some(Ok(image)) => {
            let dynamic = DynamicImage::ImageRgba8(image);
            let rgba8 = dynamic.to_rgba8();
            let (w, h) = (rgba8.width(), rgba8.height());
            let mut png_buf = Vec::new();
            PngEncoder::new(&mut png_buf)
                .write_image(&rgba8, w, h, image::ExtendedColorType::Rgba8)
                .map_err(|e| PageError::ScreenshotFailed(format!("PNG encoding failed: {e}")))?;
            Ok(png_buf)
        }
        Some(Err(e)) => Err(PageError::ScreenshotFailed(format!("{e:?}"))),
        None => Err(PageError::Timeout),
    }
}

fn capture_html(
    servo: &Servo,
    event_loop: &ScraperEventLoop,
    webview: &WebView,
    timeout_secs: u64,
) -> Result<String, PageError> {
    match eval_js(
        servo,
        event_loop,
        webview,
        "document.documentElement.outerHTML",
        timeout_secs,
    )? {
        JSValue::String(html) => Ok(html),
        other => Err(PageError::JsError(format!(
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

/// Produce a properly escaped, double-quoted JS string literal using serde_json.
/// Handles backslashes, quotes, newlines, tabs, null bytes, and all Unicode control chars.
fn js_string_literal(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{}\"", s))
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

// ---------------------------------------------------------------------------
// Internal: Per-page state
// ---------------------------------------------------------------------------

/// Internal state for a single page/tab.
struct PageState {
    webview: Option<WebView>,
    rendering_context: Rc<SoftwareRenderingContext>,
    delegate: Rc<PageDelegate>,
    width: u32,
    height: u32,
}

// ===========================================================================
// Layer 1: PageEngine (single-threaded, zero overhead)
// ===========================================================================

/// Single-threaded page engine. **Not** `Send` or `Sync`.
///
/// Use this directly from Rust when you control the thread (e.g. from a CLI
/// binary). For FFI or multi-threaded use, see [`Page`](crate::Page).
pub struct PageEngine {
    servo: Servo,
    event_loop: ScraperEventLoop,
    pages: HashMap<u32, PageState>,
    active_page_id: Option<u32>,
    next_page_id: u32,
    popup_buffer: Rc<RefCell<Vec<PendingPopup>>>,
    popup_enabled: Rc<Cell<bool>>,
    options: PageOptions,
}

impl PageEngine {
    /// Create a new page engine with the given options.
    pub fn new(options: PageOptions) -> Result<Self, PageError> {
        resources::set(Box::new(EmbeddedResourceReader));

        rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .ok();

        let event_loop = ScraperEventLoop::default();
        let waker = event_loop.create_waker();

        let mut builder = ServoBuilder::default().event_loop_waker(waker);
        if let Some(ref ua) = options.user_agent {
            builder = builder.preferences(Preferences {
                user_agent: ua.clone(),
                ..Default::default()
            });
        }
        let servo = builder.build();
        servo.setup_logging();

        Ok(Self {
            servo,
            event_loop,
            pages: HashMap::new(),
            active_page_id: None,
            next_page_id: 0,
            popup_buffer: Rc::new(RefCell::new(Vec::new())),
            popup_enabled: Rc::new(Cell::new(false)),
            options,
        })
    }

    // -- Active-page helpers --

    fn active_page(&self) -> Result<&PageState, PageError> {
        let id = self.active_page_id.ok_or(PageError::NoPage)?;
        self.pages.get(&id).ok_or(PageError::NoPage)
    }

    fn webview(&self) -> Result<&WebView, PageError> {
        self.active_page()?
            .webview
            .as_ref()
            .ok_or(PageError::NoPage)
    }

    fn active_delegate(&self) -> Result<&PageDelegate, PageError> {
        Ok(&self.active_page()?.delegate)
    }

    // -- Internal page creation --

    fn create_page_internal(&mut self, width: u32, height: u32) -> Result<u32, PageError> {
        let rendering_context = Rc::new(
            SoftwareRenderingContext::new(PhysicalSize::new(width, height))
                .map_err(|e| PageError::InitFailed(format!("rendering context: {e:?}")))?,
        );
        rendering_context
            .make_current()
            .map_err(|e| PageError::InitFailed(format!("make_current: {e:?}")))?;

        let delegate = Rc::new(PageDelegate::new(
            self.popup_buffer.clone(),
            self.popup_enabled.clone(),
            width,
            height,
        ));

        let id = self.next_page_id;
        self.next_page_id += 1;

        self.pages.insert(
            id,
            PageState {
                webview: None,
                rendering_context,
                delegate,
                width,
                height,
            },
        );

        Ok(id)
    }

    /// Wait for the current load to complete (spin until `load_complete` + idle wait).
    fn wait_for_load(&self) -> Result<(), PageError> {
        let page = self.active_page()?;
        let delegate_rc = page.delegate.clone();
        let delegate_rc2 = delegate_rc.clone();
        let loaded = with_stderr_suppressed(|| {
            let loaded = spin_until(
                &self.servo,
                &self.event_loop,
                move || delegate_rc2.load_complete.get(),
                self.options.timeout,
            );

            if loaded && self.options.wait > 0.0 {
                wait_for_idle(
                    &self.servo,
                    &self.event_loop,
                    &delegate_rc,
                    Duration::from_secs_f64(self.options.wait),
                    Duration::from_secs(self.options.timeout),
                );
            }

            loaded
        });

        if !loaded {
            return Err(PageError::Timeout);
        }

        Ok(())
    }

    /// Open a URL. Creates a new WebView or navigates the existing one.
    /// If no pages exist, auto-creates page 0 and makes it active (backward compat).
    pub fn open(&mut self, url: &str) -> Result<(), PageError> {
        let parsed_url =
            Url::parse(url).map_err(|e| PageError::LoadFailed(format!("invalid URL: {e}")))?;

        // Auto-create page 0 if no pages exist (backward compatibility).
        if self.pages.is_empty() {
            let id = self.create_page_internal(self.options.width, self.options.height)?;
            self.active_page_id = Some(id);
        }

        let page = self
            .pages
            .get_mut(&self.active_page_id.ok_or(PageError::NoPage)?)
            .ok_or(PageError::NoPage)?;

        page.delegate.load_complete.set(false);

        if let Some(ref webview) = page.webview {
            webview.load(parsed_url);
        } else {
            let webview = WebViewBuilder::new(&self.servo, page.rendering_context.clone())
                .delegate(page.delegate.clone())
                .url(parsed_url)
                .build();
            page.webview = Some(webview);
        }

        self.wait_for_load()
    }

    /// Evaluate JavaScript and return the result as a JSON string.
    pub fn evaluate(&self, script: &str) -> Result<String, PageError> {
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
    pub fn screenshot(&self) -> Result<Vec<u8>, PageError> {
        let webview = self.webview()?;
        take_screenshot_bytes(&self.servo, &self.event_loop, webview, self.options.timeout)
    }

    /// Take a full-page screenshot (PNG bytes).
    pub fn screenshot_fullpage(&self) -> Result<Vec<u8>, PageError> {
        let webview = self.webview()?;
        let page = self.active_page()?;
        let js = "Math.max(document.documentElement.scrollHeight, document.body.scrollHeight)";
        if let Ok(JSValue::Number(doc_height)) = eval_js(
            &self.servo,
            &self.event_loop,
            webview,
            js,
            self.options.timeout,
        ) {
            let mut doc_height = doc_height as u32;
            if doc_height > page.height {
                let new_size = PhysicalSize::new(page.width, doc_height);
                webview.resize(new_size);
                let got_frame = wait_for_frame(
                    &self.servo,
                    &self.event_loop,
                    &page.delegate,
                    Duration::from_secs(self.options.timeout),
                );
                if !got_frame {
                    return Err(PageError::ScreenshotFailed(
                        "timed out waiting for repaint after resize".to_string(),
                    ));
                }
                // Wait for layout to stabilize at the new size.
                wait_for_idle(
                    &self.servo,
                    &self.event_loop,
                    &page.delegate,
                    Duration::from_secs_f64(self.options.wait),
                    Duration::from_secs(self.options.timeout),
                );

                // Re-check height — DOM may have changed during the wait.
                // One retry to handle the common case without risking infinite loops.
                if let Ok(JSValue::Number(new_height)) = eval_js(
                    &self.servo,
                    &self.event_loop,
                    webview,
                    js,
                    self.options.timeout,
                ) {
                    let new_height = new_height as u32;
                    if new_height != doc_height && new_height > page.height {
                        doc_height = new_height;
                        webview.resize(PhysicalSize::new(page.width, doc_height));
                        wait_for_frame(
                            &self.servo,
                            &self.event_loop,
                            &page.delegate,
                            Duration::from_secs(self.options.timeout),
                        );
                        wait_for_idle(
                            &self.servo,
                            &self.event_loop,
                            &page.delegate,
                            Duration::from_secs_f64(self.options.wait),
                            Duration::from_secs(self.options.timeout),
                        );
                    }
                }
            }
        }
        take_screenshot_bytes(&self.servo, &self.event_loop, webview, self.options.timeout)
    }

    /// Capture the page's HTML.
    pub fn html(&self) -> Result<String, PageError> {
        let webview = self.webview()?;
        capture_html(&self.servo, &self.event_loop, webview, self.options.timeout)
    }

    /// Get the current page URL.
    pub fn url(&self) -> Option<String> {
        self.webview()
            .ok()
            .and_then(|wv| wv.url().map(|u| u.to_string()))
    }

    /// Get the current page title.
    pub fn title(&self) -> Option<String> {
        self.webview().ok().and_then(|wv| wv.page_title())
    }

    /// Drain and return captured console messages.
    pub fn console_messages(&self) -> Vec<ConsoleMessage> {
        match self.active_delegate() {
            Ok(delegate) => delegate.console_messages.borrow_mut().drain(..).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Drain and return captured network requests.
    pub fn network_requests(&self) -> Vec<NetworkRequest> {
        match self.active_delegate() {
            Ok(delegate) => delegate.network_requests.borrow_mut().drain(..).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Close the active page (drop the WebView, remove from map).
    pub fn close(&mut self) {
        if let Some(id) = self.active_page_id.take() {
            self.pages.remove(&id);
        }
    }

    /// Reset all state: drop all pages, clear popup buffer, reset ID counter.
    pub fn reset(&mut self) {
        self.pages.clear();
        self.active_page_id = None;
        self.next_page_id = 0;
        self.popup_buffer.borrow_mut().clear();
    }

    // -- Phase 2: Wait mechanisms --

    /// Wait until a CSS selector matches an element on the page.
    pub fn wait_for_selector(&self, selector: &str, timeout_secs: u64) -> Result<(), PageError> {
        let webview = self.webview()?;
        let delegate = self.active_delegate()?;
        let escaped = js_string_literal(selector);
        let js = format!("document.querySelector({escaped}) !== null");

        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        loop {
            if let Ok(JSValue::Boolean(true)) =
                eval_js(&self.servo, &self.event_loop, webview, &js, timeout_secs)
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(PageError::Timeout);
            }
            wait_for_frame(
                &self.servo,
                &self.event_loop,
                delegate,
                Duration::from_millis(200),
            );
        }
    }

    /// Wait until a JS expression evaluates to a truthy value.
    pub fn wait_for_condition(&self, js_expr: &str, timeout_secs: u64) -> Result<(), PageError> {
        let webview = self.webview()?;
        let delegate = self.active_delegate()?;
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
                return Err(PageError::Timeout);
            }
            wait_for_frame(
                &self.servo,
                &self.event_loop,
                delegate,
                Duration::from_millis(200),
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
    pub fn wait_for_navigation(&self, timeout_secs: u64) -> Result<(), PageError> {
        self.webview()?;
        let delegate = self.active_delegate()?;
        delegate.load_complete.set(false);
        let delegate_rc = self.active_page()?.delegate.clone();
        let loaded = spin_until(
            &self.servo,
            &self.event_loop,
            move || delegate_rc.load_complete.get(),
            timeout_secs,
        );
        if !loaded {
            return Err(PageError::Timeout);
        }
        Ok(())
    }

    /// Wait until no new network requests arrive for `idle_ms` milliseconds.
    pub fn wait_for_network_idle(&self, idle_ms: u64, timeout_secs: u64) -> Result<(), PageError> {
        self.webview()?;
        let delegate = self.active_delegate()?;
        if delegate.last_request_time.get().is_none() {
            return Ok(());
        }
        let settled = wait_for_network_idle_inner(
            &self.servo,
            &self.event_loop,
            delegate,
            Duration::from_millis(idle_ms),
            Duration::from_secs(timeout_secs),
        );
        if settled {
            Ok(())
        } else {
            Err(PageError::Timeout)
        }
    }

    // -- Phase 3: Input events --

    /// Click at the given device coordinates.
    pub fn click(&self, x: f32, y: f32) -> Result<(), PageError> {
        let webview = self.webview()?;
        let delegate = self.active_delegate()?;
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
            delegate,
            Duration::from_secs(2),
        );

        Ok(())
    }

    /// Click on an element matching a CSS selector.
    pub fn click_selector(&self, selector: &str) -> Result<(), PageError> {
        let webview = self.webview()?;
        let escaped = js_string_literal(selector);
        let js = format!(
            "(function() {{ \
                var el = document.querySelector({escaped}); \
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
                    _ => return Err(PageError::JsError("invalid coordinate".into())),
                };
                let y = match &coords[1] {
                    JSValue::Number(n) => *n as f32,
                    _ => return Err(PageError::JsError("invalid coordinate".into())),
                };
                self.click(x, y)
            }
            JSValue::Null | JSValue::Undefined => {
                Err(PageError::SelectorNotFound(selector.to_string()))
            }
            other => Err(PageError::JsError(format!(
                "unexpected getBoundingClientRect result: {other:?}"
            ))),
        }
    }

    /// Type text by sending individual key events.
    pub fn type_text(&self, text: &str) -> Result<(), PageError> {
        let webview = self.webview()?;
        let delegate = self.active_delegate()?;
        for ch in text.chars() {
            let key = Key::Character(ch.to_string());

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
                delegate,
                Duration::from_secs(2),
            );
        }
        Ok(())
    }

    /// Press a single key by name (e.g. "Enter", "Tab", "a").
    pub fn key_press(&self, key_name: &str) -> Result<(), PageError> {
        let webview = self.webview()?;
        let delegate = self.active_delegate()?;
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
            delegate,
            Duration::from_secs(2),
        );

        Ok(())
    }

    /// Move the mouse to the given device coordinates.
    pub fn mouse_move(&self, x: f32, y: f32) -> Result<(), PageError> {
        let webview = self.webview()?;
        let delegate = self.active_delegate()?;
        let point = WebViewPoint::from(DevicePoint::new(x, y));
        webview.notify_input_event(InputEvent::MouseMove(MouseMoveEvent::new(point)));
        wait_for_frame(
            &self.servo,
            &self.event_loop,
            delegate,
            Duration::from_secs(2),
        );
        Ok(())
    }

    // -- Scroll --

    /// Scroll the viewport by the given pixel deltas using a native wheel event.
    pub fn scroll(&self, delta_x: f64, delta_y: f64) -> Result<(), PageError> {
        let webview = self.webview()?;
        let page = self.active_page()?;
        let center = WebViewPoint::from(DevicePoint::new(
            page.width as f32 / 2.0,
            page.height as f32 / 2.0,
        ));
        // Servo's WheelDelta convention: positive y = scroll up (content moves down).
        // We negate so our API uses the intuitive convention: positive y = scroll down.
        let delta = WheelDelta {
            x: -delta_x,
            y: -delta_y,
            z: 0.0,
            mode: WheelMode::DeltaPixel,
        };
        webview.notify_input_event(InputEvent::Wheel(WheelEvent::new(delta, center)));
        wait_for_frame(
            &self.servo,
            &self.event_loop,
            &page.delegate,
            Duration::from_secs(2),
        );
        wait_for_idle(
            &self.servo,
            &self.event_loop,
            &page.delegate,
            Duration::from_millis(200),
            Duration::from_secs(2),
        );
        Ok(())
    }

    /// Scroll the element matching a CSS selector into view.
    pub fn scroll_to_selector(&self, selector: &str) -> Result<(), PageError> {
        let webview = self.webview()?;
        let delegate = self.active_delegate()?;
        let escaped = js_string_literal(selector);
        let js = format!(
            "(function() {{ \
                var el = document.querySelector({escaped}); \
                if (!el) return null; \
                el.scrollIntoView({{behavior: 'instant', block: 'center'}}); \
                return true; \
            }})()"
        );
        match eval_js(
            &self.servo,
            &self.event_loop,
            webview,
            &js,
            self.options.timeout,
        )? {
            JSValue::Boolean(true) => {
                wait_for_frame(
                    &self.servo,
                    &self.event_loop,
                    delegate,
                    Duration::from_secs(2),
                );
                Ok(())
            }
            JSValue::Null | JSValue::Undefined => Err(PageError::SelectorNotFound(selector.into())),
            other => Err(PageError::JsError(format!(
                "unexpected scrollIntoView result: {other:?}"
            ))),
        }
    }

    // -- Select --

    /// Select an option in a `<select>` element by value.
    pub fn select_option(&self, selector: &str, value: &str) -> Result<(), PageError> {
        let webview = self.webview()?;
        let esc_sel = js_string_literal(selector);
        let esc_val = js_string_literal(value);
        let js = format!(
            "(function() {{ \
                var el = document.querySelector({esc_sel}); \
                if (!el) return 'not_found'; \
                if (el.tagName !== 'SELECT') return 'not_select'; \
                var opt = Array.from(el.options).find(function(o) {{ return o.value === {esc_val}; }}); \
                if (!opt) return 'no_option'; \
                el.value = {esc_val}; \
                el.dispatchEvent(new Event('input', {{bubbles: true}})); \
                el.dispatchEvent(new Event('change', {{bubbles: true}})); \
                return 'ok'; \
            }})()"
        );
        match eval_js(
            &self.servo,
            &self.event_loop,
            webview,
            &js,
            self.options.timeout,
        )? {
            JSValue::String(s) if s == "ok" => Ok(()),
            JSValue::String(s) if s == "not_found" => {
                Err(PageError::SelectorNotFound(selector.into()))
            }
            JSValue::String(s) if s == "not_select" => Err(PageError::JsError(format!(
                "element '{selector}' is not a <select>"
            ))),
            JSValue::String(s) if s == "no_option" => Err(PageError::JsError(format!(
                "no option with value '{value}' in '{selector}'"
            ))),
            other => Err(PageError::JsError(format!(
                "unexpected select result: {other:?}"
            ))),
        }
    }

    // -- File upload --

    /// Set files on an `<input type="file">` element using the DataTransfer API.
    pub fn set_input_files(&self, selector: &str, files: &[InputFile]) -> Result<(), PageError> {
        let webview = self.webview()?;
        let esc_sel = js_string_literal(selector);

        use base64::Engine as _;
        let file_entries: Vec<String> = files
            .iter()
            .map(|f| {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&f.data);
                let esc_name = js_string_literal(&f.name);
                let esc_mime = js_string_literal(&f.mime_type);
                format!("{{name:{esc_name},mime:{esc_mime},b64:'{b64}'}}")
            })
            .collect();
        let files_js = file_entries.join(",");

        let js = format!(
            "(function() {{ \
                var input = document.querySelector({esc_sel}); \
                if (!input) return 'not_found'; \
                if (input.type !== 'file') return 'not_file'; \
                var dt = new DataTransfer(); \
                var files = [{files_js}]; \
                for (var i = 0; i < files.length; i++) {{ \
                    var f = files[i]; \
                    var bytes = Uint8Array.from(atob(f.b64), function(c) {{ return c.charCodeAt(0); }}); \
                    dt.items.add(new File([bytes], f.name, {{type: f.mime}})); \
                }} \
                input.files = dt.files; \
                input.dispatchEvent(new Event('change', {{bubbles: true}})); \
                return 'ok'; \
            }})()"
        );
        match eval_js(
            &self.servo,
            &self.event_loop,
            webview,
            &js,
            self.options.timeout,
        )? {
            JSValue::String(s) if s == "ok" => Ok(()),
            JSValue::String(s) if s == "not_found" => {
                Err(PageError::SelectorNotFound(selector.into()))
            }
            JSValue::String(s) if s == "not_file" => Err(PageError::JsError(format!(
                "element '{selector}' is not an <input type=\"file\">"
            ))),
            other => Err(PageError::JsError(format!(
                "unexpected file input result: {other:?}"
            ))),
        }
    }

    // -- Cookies (JS-based) --

    /// Get cookies for the current page via `document.cookie`.
    pub fn get_cookies(&self) -> Result<String, PageError> {
        let webview = self.webview()?;
        match eval_js(
            &self.servo,
            &self.event_loop,
            webview,
            "document.cookie",
            self.options.timeout,
        )? {
            JSValue::String(s) => Ok(s),
            other => Err(PageError::JsError(format!(
                "unexpected cookie result: {other:?}"
            ))),
        }
    }

    /// Set a cookie via `document.cookie = '...'`.
    pub fn set_cookie(&self, cookie: &str) -> Result<(), PageError> {
        let webview = self.webview()?;
        let escaped = js_string_literal(cookie);
        let js = format!("document.cookie = {escaped}");
        eval_js(
            &self.servo,
            &self.event_loop,
            webview,
            &js,
            self.options.timeout,
        )?;
        Ok(())
    }

    /// Clear all cookies by expiring each one.
    pub fn clear_cookies(&self) -> Result<(), PageError> {
        let webview = self.webview()?;
        let js = r#"(function() {
            var cookies = document.cookie.split(';');
            for (var i = 0; i < cookies.length; i++) {
                var name = cookies[i].split('=')[0].trim();
                if (name) {
                    document.cookie = name + '=;expires=Thu, 01 Jan 1970 00:00:00 GMT;path=/';
                }
            }
        })()"#;
        eval_js(
            &self.servo,
            &self.event_loop,
            webview,
            js,
            self.options.timeout,
        )?;
        Ok(())
    }

    // -- Request interception --

    /// Set URL patterns to block. Any request whose URL contains a pattern is cancelled.
    /// Requires an active page.
    pub fn block_urls(&mut self, patterns: Vec<String>) {
        if let Ok(delegate) = self.active_delegate() {
            *delegate.blocked_url_patterns.borrow_mut() = patterns;
        }
    }

    /// Clear all blocked URL patterns.
    pub fn clear_blocked_urls(&mut self) {
        if let Ok(delegate) = self.active_delegate() {
            delegate.blocked_url_patterns.borrow_mut().clear();
        }
    }

    // -- Navigation --

    /// Reload the current page.
    pub fn reload(&self) -> Result<(), PageError> {
        let webview = self.webview()?;
        let delegate = self.active_delegate()?;
        delegate.load_complete.set(false);
        webview.reload();
        self.wait_for_load()
    }

    /// Navigate back in history. Returns `false` if there is no history to go back to.
    pub fn go_back(&self) -> Result<bool, PageError> {
        let webview = self.webview()?;
        if !webview.can_go_back() {
            return Ok(false);
        }
        let delegate = self.active_delegate()?;
        delegate.load_complete.set(false);
        webview.go_back(1);
        self.wait_for_load()?;
        Ok(true)
    }

    /// Navigate forward in history. Returns `false` if there is no forward history.
    pub fn go_forward(&self) -> Result<bool, PageError> {
        let webview = self.webview()?;
        if !webview.can_go_forward() {
            return Ok(false);
        }
        let delegate = self.active_delegate()?;
        delegate.load_complete.set(false);
        webview.go_forward(1);
        self.wait_for_load()?;
        Ok(true)
    }

    // -- Element info (JS-based) --

    /// Get the bounding rectangle of the first element matching a CSS selector.
    pub fn element_rect(&self, selector: &str) -> Result<ElementRect, PageError> {
        let webview = self.webview()?;
        let escaped = js_string_literal(selector);
        let js = format!(
            "(function() {{ \
                var el = document.querySelector({escaped}); \
                if (!el) return null; \
                var r = el.getBoundingClientRect(); \
                return [r.x, r.y, r.width, r.height]; \
            }})()"
        );

        match eval_js(
            &self.servo,
            &self.event_loop,
            webview,
            &js,
            self.options.timeout,
        )? {
            JSValue::Array(arr) if arr.len() == 4 => {
                let nums: Vec<f64> = arr
                    .iter()
                    .map(|v| match v {
                        JSValue::Number(n) => Ok(*n),
                        _ => Err(PageError::JsError("invalid rect value".into())),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(ElementRect {
                    x: nums[0],
                    y: nums[1],
                    width: nums[2],
                    height: nums[3],
                })
            }
            JSValue::Null | JSValue::Undefined => {
                Err(PageError::SelectorNotFound(selector.to_string()))
            }
            other => Err(PageError::JsError(format!(
                "unexpected rect result: {other:?}"
            ))),
        }
    }

    /// Get the text content of the first element matching a CSS selector.
    pub fn element_text(&self, selector: &str) -> Result<String, PageError> {
        let webview = self.webview()?;
        let escaped = js_string_literal(selector);
        let js = format!(
            "(function() {{ \
                var el = document.querySelector({escaped}); \
                if (!el) return null; \
                return el.textContent; \
            }})()"
        );

        match eval_js(
            &self.servo,
            &self.event_loop,
            webview,
            &js,
            self.options.timeout,
        )? {
            JSValue::String(s) => Ok(s),
            JSValue::Null | JSValue::Undefined => {
                Err(PageError::SelectorNotFound(selector.to_string()))
            }
            other => Err(PageError::JsError(format!(
                "unexpected text result: {other:?}"
            ))),
        }
    }

    /// Get an attribute value of the first element matching a CSS selector.
    /// Returns `Ok(None)` if the element exists but the attribute does not.
    pub fn element_attribute(
        &self,
        selector: &str,
        attribute: &str,
    ) -> Result<Option<String>, PageError> {
        let webview = self.webview()?;
        let esc_sel = js_string_literal(selector);
        let esc_attr = js_string_literal(attribute);
        let js = format!(
            "(function() {{ \
                var el = document.querySelector({esc_sel}); \
                if (!el) return undefined; \
                return el.getAttribute({esc_attr}); \
            }})()"
        );

        match eval_js(
            &self.servo,
            &self.event_loop,
            webview,
            &js,
            self.options.timeout,
        )? {
            JSValue::String(s) => Ok(Some(s)),
            JSValue::Null => Ok(None),
            JSValue::Undefined => Err(PageError::SelectorNotFound(selector.to_string())),
            other => Err(PageError::JsError(format!(
                "unexpected attribute result: {other:?}"
            ))),
        }
    }

    /// Get the outer HTML of the first element matching a CSS selector.
    pub fn element_html(&self, selector: &str) -> Result<String, PageError> {
        let webview = self.webview()?;
        let escaped = js_string_literal(selector);
        let js = format!(
            "(function() {{ \
                var el = document.querySelector({escaped}); \
                if (!el) return null; \
                return el.outerHTML; \
            }})()"
        );

        match eval_js(
            &self.servo,
            &self.event_loop,
            webview,
            &js,
            self.options.timeout,
        )? {
            JSValue::String(s) => Ok(s),
            JSValue::Null | JSValue::Undefined => {
                Err(PageError::SelectorNotFound(selector.to_string()))
            }
            other => Err(PageError::JsError(format!(
                "unexpected html result: {other:?}"
            ))),
        }
    }

    // =====================================================================
    // Multi-page methods
    // =====================================================================

    /// Create a new page with the default viewport size. Returns the page ID.
    pub fn new_page(&mut self) -> Result<u32, PageError> {
        self.create_page_internal(self.options.width, self.options.height)
    }

    /// Create a new page with a custom viewport size. Returns the page ID.
    pub fn new_page_with_size(&mut self, width: u32, height: u32) -> Result<u32, PageError> {
        self.create_page_internal(width, height)
    }

    /// Switch the active page to the given ID.
    pub fn switch_to(&mut self, page_id: u32) -> Result<(), PageError> {
        if !self.pages.contains_key(&page_id) {
            return Err(PageError::NoPage);
        }
        self.active_page_id = Some(page_id);
        Ok(())
    }

    /// Close a specific page by ID (removes it from the map).
    /// If the closed page is the active page, `active_page_id` becomes `None`.
    pub fn close_page(&mut self, page_id: u32) -> Result<(), PageError> {
        if self.pages.remove(&page_id).is_none() {
            return Err(PageError::NoPage);
        }
        if self.active_page_id == Some(page_id) {
            self.active_page_id = None;
        }
        Ok(())
    }

    /// Get the active page ID, or `None` if no page is active.
    pub fn active_page_id(&self) -> Option<u32> {
        self.active_page_id
    }

    /// List all open page IDs (sorted).
    pub fn page_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self.pages.keys().copied().collect();
        ids.sort();
        ids
    }

    /// Get the number of open pages.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Enable or disable popup capture. When disabled (default), popups are blocked.
    pub fn set_popup_handling(&mut self, enabled: bool) {
        self.popup_enabled.set(enabled);
    }

    /// Drain pending popup WebViews, assign page IDs, and return them.
    pub fn popup_pages(&mut self) -> Vec<u32> {
        let popups: Vec<PendingPopup> = self.popup_buffer.borrow_mut().drain(..).collect();
        let mut ids = Vec::with_capacity(popups.len());
        for popup in popups {
            let id = self.next_page_id;
            self.next_page_id += 1;
            let width = popup.delegate.default_width.get();
            let height = popup.delegate.default_height.get();
            self.pages.insert(
                id,
                PageState {
                    webview: Some(popup.webview),
                    rendering_context: popup.rendering_context,
                    delegate: popup.delegate,
                    width,
                    height,
                },
            );
            ids.push(id);
        }
        ids
    }

    /// Get the URL of a specific page by ID (without switching).
    pub fn page_url(&self, page_id: u32) -> Option<String> {
        self.pages
            .get(&page_id)
            .and_then(|p| p.webview.as_ref())
            .and_then(|wv| wv.url().map(|u| u.to_string()))
    }

    /// Get the title of a specific page by ID (without switching).
    pub fn page_title(&self, page_id: u32) -> Option<String> {
        self.pages
            .get(&page_id)
            .and_then(|p| p.webview.as_ref())
            .and_then(|wv| wv.page_title())
    }
}
