//! Windows, input events and timing over raw OS bindings.
//!
//! This crate uses nothing beyond `std`. Each backend declares the handful of OS functions it
//! needs itself and loads the system libraries at run time, so a build never needs headers,
//! import libraries or a C toolchain.
//!
//! | OS      | Backend                         | Status      |
//! |---------|---------------------------------|-------------|
//! | Linux   | X11 (`libX11.so.6`, loaded)     | implemented |
//! | Linux   | Wayland                         | planned     |
//! | Windows | Win32 (`user32.dll`, loaded)    | implemented |
//! | macOS   | Cocoa through the objc runtime  | planned     |
//!
//! ```no_run
//! use shallot_platform::{Event, Window, WindowConfig};
//!
//! let mut window = Window::new(&WindowConfig::new("Hello", 800, 600)).unwrap();
//! let mut events = Vec::new();
//! 'run: loop {
//!     window.poll_events(&mut events);
//!     for event in events.drain(..) {
//!         if let Event::CloseRequested = event {
//!             break 'run;
//!         }
//!     }
//! }
//! ```

mod event;
pub mod library;
pub mod time;
mod window;

#[cfg(windows)]
mod win32;
#[cfg(target_os = "linux")]
mod x11;

pub use event::{Event, Key, MouseButton};
pub use library::Library;
pub use window::{RawWindowHandle, Window, WindowConfig};

use std::fmt;

/// An error from the platform layer, such as a missing system library or a refused window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformError(pub String);

impl PlatformError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PlatformError {}
