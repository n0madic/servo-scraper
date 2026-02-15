/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Shared public types used across all layers.

use std::fmt;

use serde::Serialize;

/// Options for configuring a page session.
#[derive(Debug, Clone)]
pub struct PageOptions {
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

impl Default for PageOptions {
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
pub enum PageError {
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

impl fmt::Display for PageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PageError::InitFailed(msg) => write!(f, "initialization failed: {msg}"),
            PageError::LoadFailed(msg) => write!(f, "page load failed: {msg}"),
            PageError::Timeout => write!(f, "timed out"),
            PageError::JsError(msg) => write!(f, "JavaScript error: {msg}"),
            PageError::ScreenshotFailed(msg) => write!(f, "screenshot failed: {msg}"),
            PageError::ChannelClosed => write!(f, "internal channel closed"),
            PageError::NoPage => write!(f, "no page open"),
            PageError::SelectorNotFound(sel) => write!(f, "selector not found: {sel}"),
        }
    }
}

impl std::error::Error for PageError {}
