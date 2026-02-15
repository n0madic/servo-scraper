/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Layer 2: `Page` — thread-safe wrapper (`Send + Sync`).

use std::sync::Mutex;
use std::sync::mpsc;
use std::thread;

use crate::engine::PageEngine;
use crate::types::{
    ConsoleMessage, ElementRect, InputFile, NetworkRequest, PageError, PageOptions,
};

/// Commands sent from the `Page` handle to the background thread.
enum Command {
    Open {
        url: String,
        response: mpsc::Sender<Result<(), PageError>>,
    },
    Evaluate {
        script: String,
        response: mpsc::Sender<Result<String, PageError>>,
    },
    Screenshot {
        response: mpsc::Sender<Result<Vec<u8>, PageError>>,
    },
    ScreenshotFullpage {
        response: mpsc::Sender<Result<Vec<u8>, PageError>>,
    },
    Html {
        response: mpsc::Sender<Result<String, PageError>>,
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
    Reset {
        response: mpsc::Sender<()>,
    },
    // Phase 2: Wait commands
    WaitForSelector {
        selector: String,
        timeout: u64,
        response: mpsc::Sender<Result<(), PageError>>,
    },
    WaitForCondition {
        js_expr: String,
        timeout: u64,
        response: mpsc::Sender<Result<(), PageError>>,
    },
    Wait {
        seconds: f64,
        response: mpsc::Sender<()>,
    },
    WaitForNavigation {
        timeout: u64,
        response: mpsc::Sender<Result<(), PageError>>,
    },
    // Phase 3: Input commands
    Click {
        x: f32,
        y: f32,
        response: mpsc::Sender<Result<(), PageError>>,
    },
    ClickSelector {
        selector: String,
        response: mpsc::Sender<Result<(), PageError>>,
    },
    TypeText {
        text: String,
        response: mpsc::Sender<Result<(), PageError>>,
    },
    KeyPress {
        key: String,
        response: mpsc::Sender<Result<(), PageError>>,
    },
    MouseMove {
        x: f32,
        y: f32,
        response: mpsc::Sender<Result<(), PageError>>,
    },
    // Scroll
    Scroll {
        delta_x: f64,
        delta_y: f64,
        response: mpsc::Sender<Result<(), PageError>>,
    },
    ScrollToSelector {
        selector: String,
        response: mpsc::Sender<Result<(), PageError>>,
    },
    // Select
    SelectOption {
        selector: String,
        value: String,
        response: mpsc::Sender<Result<(), PageError>>,
    },
    // File upload
    SetInputFiles {
        selector: String,
        files: Vec<InputFile>,
        response: mpsc::Sender<Result<(), PageError>>,
    },
    // Cookies
    GetCookies {
        response: mpsc::Sender<Result<String, PageError>>,
    },
    SetCookie {
        cookie: String,
        response: mpsc::Sender<Result<(), PageError>>,
    },
    ClearCookies {
        response: mpsc::Sender<Result<(), PageError>>,
    },
    // Request interception
    BlockUrls {
        patterns: Vec<String>,
        response: mpsc::Sender<()>,
    },
    ClearBlockedUrls {
        response: mpsc::Sender<()>,
    },
    // Navigation
    Reload {
        response: mpsc::Sender<Result<(), PageError>>,
    },
    GoBack {
        response: mpsc::Sender<Result<bool, PageError>>,
    },
    GoForward {
        response: mpsc::Sender<Result<bool, PageError>>,
    },
    // Element info
    ElementRect {
        selector: String,
        response: mpsc::Sender<Result<ElementRect, PageError>>,
    },
    ElementText {
        selector: String,
        response: mpsc::Sender<Result<String, PageError>>,
    },
    ElementAttribute {
        selector: String,
        attribute: String,
        response: mpsc::Sender<Result<Option<String>, PageError>>,
    },
    ElementHtml {
        selector: String,
        response: mpsc::Sender<Result<String, PageError>>,
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
    pub fn new(options: PageOptions) -> Result<Self, PageError> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
        let (init_tx, init_rx) = mpsc::channel::<Result<(), PageError>>();

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
                    Command::Reset { response } => {
                        engine.reset();
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
                    Command::Scroll {
                        delta_x,
                        delta_y,
                        response,
                    } => {
                        let _ = response.send(engine.scroll(delta_x, delta_y));
                    }
                    Command::ScrollToSelector { selector, response } => {
                        let _ = response.send(engine.scroll_to_selector(&selector));
                    }
                    Command::SelectOption {
                        selector,
                        value,
                        response,
                    } => {
                        let _ = response.send(engine.select_option(&selector, &value));
                    }
                    Command::SetInputFiles {
                        selector,
                        files,
                        response,
                    } => {
                        let _ = response.send(engine.set_input_files(&selector, &files));
                    }
                    Command::GetCookies { response } => {
                        let _ = response.send(engine.get_cookies());
                    }
                    Command::SetCookie { cookie, response } => {
                        let _ = response.send(engine.set_cookie(&cookie));
                    }
                    Command::ClearCookies { response } => {
                        let _ = response.send(engine.clear_cookies());
                    }
                    Command::BlockUrls { patterns, response } => {
                        engine.block_urls(patterns);
                        let _ = response.send(());
                    }
                    Command::ClearBlockedUrls { response } => {
                        engine.clear_blocked_urls();
                        let _ = response.send(());
                    }
                    Command::Reload { response } => {
                        let _ = response.send(engine.reload());
                    }
                    Command::GoBack { response } => {
                        let _ = response.send(engine.go_back());
                    }
                    Command::GoForward { response } => {
                        let _ = response.send(engine.go_forward());
                    }
                    Command::ElementRect { selector, response } => {
                        let _ = response.send(engine.element_rect(&selector));
                    }
                    Command::ElementText { selector, response } => {
                        let _ = response.send(engine.element_text(&selector));
                    }
                    Command::ElementAttribute {
                        selector,
                        attribute,
                        response,
                    } => {
                        let _ = response.send(engine.element_attribute(&selector, &attribute));
                    }
                    Command::ElementHtml { selector, response } => {
                        let _ = response.send(engine.element_html(&selector));
                    }
                    Command::Shutdown => break,
                }
            }
        });

        init_rx
            .recv()
            .map_err(|_| PageError::InitFailed("background thread panicked".into()))??;

        Ok(Self {
            sender: Mutex::new(cmd_tx),
            thread: Mutex::new(Some(thread)),
        })
    }

    fn send_cmd<T>(
        &self,
        make_cmd: impl FnOnce(mpsc::Sender<T>) -> Command,
    ) -> Result<T, PageError> {
        let (resp_tx, resp_rx) = mpsc::channel();
        let sender = self.sender.lock().map_err(|_| PageError::ChannelClosed)?;
        sender
            .send(make_cmd(resp_tx))
            .map_err(|_| PageError::ChannelClosed)?;
        drop(sender);
        resp_rx.recv().map_err(|_| PageError::ChannelClosed)
    }

    pub fn open(&self, url: &str) -> Result<(), PageError> {
        self.send_cmd(|response| Command::Open {
            url: url.to_string(),
            response,
        })?
    }

    pub fn evaluate(&self, script: &str) -> Result<String, PageError> {
        self.send_cmd(|response| Command::Evaluate {
            script: script.to_string(),
            response,
        })?
    }

    pub fn screenshot(&self) -> Result<Vec<u8>, PageError> {
        self.send_cmd(|response| Command::Screenshot { response })?
    }

    pub fn screenshot_fullpage(&self) -> Result<Vec<u8>, PageError> {
        self.send_cmd(|response| Command::ScreenshotFullpage { response })?
    }

    pub fn html(&self) -> Result<String, PageError> {
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

    pub fn reset(&self) {
        let _ = self.send_cmd(|response| Command::Reset { response });
    }

    pub fn wait_for_selector(&self, selector: &str, timeout: u64) -> Result<(), PageError> {
        self.send_cmd(|response| Command::WaitForSelector {
            selector: selector.to_string(),
            timeout,
            response,
        })?
    }

    pub fn wait_for_condition(&self, js_expr: &str, timeout: u64) -> Result<(), PageError> {
        self.send_cmd(|response| Command::WaitForCondition {
            js_expr: js_expr.to_string(),
            timeout,
            response,
        })?
    }

    pub fn wait(&self, seconds: f64) {
        let _ = self.send_cmd(|response| Command::Wait { seconds, response });
    }

    pub fn wait_for_navigation(&self, timeout: u64) -> Result<(), PageError> {
        self.send_cmd(|response| Command::WaitForNavigation { timeout, response })?
    }

    pub fn click(&self, x: f32, y: f32) -> Result<(), PageError> {
        self.send_cmd(|response| Command::Click { x, y, response })?
    }

    pub fn click_selector(&self, selector: &str) -> Result<(), PageError> {
        self.send_cmd(|response| Command::ClickSelector {
            selector: selector.to_string(),
            response,
        })?
    }

    pub fn type_text(&self, text: &str) -> Result<(), PageError> {
        self.send_cmd(|response| Command::TypeText {
            text: text.to_string(),
            response,
        })?
    }

    pub fn key_press(&self, key: &str) -> Result<(), PageError> {
        self.send_cmd(|response| Command::KeyPress {
            key: key.to_string(),
            response,
        })?
    }

    pub fn mouse_move(&self, x: f32, y: f32) -> Result<(), PageError> {
        self.send_cmd(|response| Command::MouseMove { x, y, response })?
    }

    pub fn scroll(&self, delta_x: f64, delta_y: f64) -> Result<(), PageError> {
        self.send_cmd(|response| Command::Scroll {
            delta_x,
            delta_y,
            response,
        })?
    }

    pub fn scroll_to_selector(&self, selector: &str) -> Result<(), PageError> {
        self.send_cmd(|response| Command::ScrollToSelector {
            selector: selector.to_string(),
            response,
        })?
    }

    pub fn select_option(&self, selector: &str, value: &str) -> Result<(), PageError> {
        self.send_cmd(|response| Command::SelectOption {
            selector: selector.to_string(),
            value: value.to_string(),
            response,
        })?
    }

    pub fn set_input_files(&self, selector: &str, files: Vec<InputFile>) -> Result<(), PageError> {
        self.send_cmd(|response| Command::SetInputFiles {
            selector: selector.to_string(),
            files,
            response,
        })?
    }

    pub fn get_cookies(&self) -> Result<String, PageError> {
        self.send_cmd(|response| Command::GetCookies { response })?
    }

    pub fn set_cookie(&self, cookie: &str) -> Result<(), PageError> {
        self.send_cmd(|response| Command::SetCookie {
            cookie: cookie.to_string(),
            response,
        })?
    }

    pub fn clear_cookies(&self) -> Result<(), PageError> {
        self.send_cmd(|response| Command::ClearCookies { response })?
    }

    pub fn block_urls(&self, patterns: Vec<String>) {
        let _ = self.send_cmd(|response| Command::BlockUrls { patterns, response });
    }

    pub fn clear_blocked_urls(&self) {
        let _ = self.send_cmd(|response| Command::ClearBlockedUrls { response });
    }

    pub fn reload(&self) -> Result<(), PageError> {
        self.send_cmd(|response| Command::Reload { response })?
    }

    pub fn go_back(&self) -> Result<bool, PageError> {
        self.send_cmd(|response| Command::GoBack { response })?
    }

    pub fn go_forward(&self) -> Result<bool, PageError> {
        self.send_cmd(|response| Command::GoForward { response })?
    }

    pub fn element_rect(&self, selector: &str) -> Result<ElementRect, PageError> {
        self.send_cmd(|response| Command::ElementRect {
            selector: selector.to_string(),
            response,
        })?
    }

    pub fn element_text(&self, selector: &str) -> Result<String, PageError> {
        self.send_cmd(|response| Command::ElementText {
            selector: selector.to_string(),
            response,
        })?
    }

    pub fn element_attribute(
        &self,
        selector: &str,
        attribute: &str,
    ) -> Result<Option<String>, PageError> {
        self.send_cmd(|response| Command::ElementAttribute {
            selector: selector.to_string(),
            attribute: attribute.to_string(),
            response,
        })?
    }

    pub fn element_html(&self, selector: &str) -> Result<String, PageError> {
        self.send_cmd(|response| Command::ElementHtml {
            selector: selector.to_string(),
            response,
        })?
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
