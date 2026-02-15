/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Layer 1: `PageEngine` — single-threaded, zero-overhead core.

use std::cell::{Cell, RefCell};
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
    ConsoleLogLevel, DevicePoint, EmbedderControl, EventLoopWaker, InputEvent, JSValue, Key,
    KeyState, KeyboardEvent, LoadStatus, MouseButton, MouseButtonAction, MouseButtonEvent,
    MouseMoveEvent, NamedKey, RenderingContext, Servo, ServoBuilder, SimpleDialog,
    SoftwareRenderingContext, WebResourceLoad, WebView, WebViewBuilder, WebViewDelegate,
    WebViewPoint,
};
use url::Url;

use crate::types::{ConsoleMessage, NetworkRequest, ScraperError, ScraperOptions};

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
/// binary). For FFI or multi-threaded use, see [`Page`](crate::Page).
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
