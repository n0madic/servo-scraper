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
//! use servo_scraper::{PageEngine, PageOptions};
//!
//! let mut engine = PageEngine::new(PageOptions::default()).unwrap();
//! engine.open("https://example.com").unwrap();
//! let html = engine.html().unwrap();
//! let png = engine.screenshot().unwrap();
//! ```
//!
//! # Example (thread-safe / FFI)
//!
//! ```no_run
//! use servo_scraper::{Page, PageOptions};
//!
//! let page = Page::new(PageOptions::default()).unwrap();
//! page.open("https://example.com").unwrap();
//! let png = page.screenshot().unwrap();
//! ```

mod engine;
mod ffi;
mod page;
mod types;

pub use engine::PageEngine;
pub use page::Page;
pub use types::{ConsoleMessage, NetworkRequest, PageError, PageOptions};
