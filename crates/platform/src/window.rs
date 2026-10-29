//! The cross-platform window type.

use crate::{Event, PlatformError};
use std::ffi::c_void;

/// How to create a [`Window`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowConfig {
    pub title: String,
    /// Initial drawable width in physical pixels.
    pub width: u32,
    /// Initial drawable height in physical pixels.
    pub height: u32,
    pub resizable: bool,
}

impl WindowConfig {
    pub fn new(title: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            title: title.into(),
            width,
            height,
            resizable: true,
        }
    }
}

/// The OS handles a GPU API needs to present to a window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawWindowHandle {
    /// An Xlib `Display*` and `Window`.
    Xlib { display: *mut c_void, window: u64 },
}

/// A top-level OS window.
pub struct Window {
    #[cfg(target_os = "linux")]
    inner: crate::x11::Window,
}

impl Window {
    /// Opens and shows a window.
    pub fn new(config: &WindowConfig) -> Result<Window, PlatformError> {
        #[cfg(target_os = "linux")]
        {
            Ok(Window {
                inner: crate::x11::Window::new(config)?,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = config;
            Err(PlatformError::new("no window backend for this OS yet"))
        }
    }

    /// Appends every pending event to `events` without blocking.
    pub fn poll_events(&mut self, events: &mut Vec<Event>) {
        #[cfg(target_os = "linux")]
        self.inner.poll_events(events);
        #[cfg(not(target_os = "linux"))]
        let _ = events;
    }

    /// The drawable size in physical pixels, as of the last processed event.
    pub fn size(&self) -> (u32, u32) {
        #[cfg(target_os = "linux")]
        return self.inner.size();
        #[cfg(not(target_os = "linux"))]
        (0, 0)
    }

    pub fn set_title(&mut self, title: &str) {
        #[cfg(target_os = "linux")]
        self.inner.set_title(title);
        #[cfg(not(target_os = "linux"))]
        let _ = title;
    }

    /// The OS handles for creating a GPU surface. Valid while the window lives.
    pub fn raw_handle(&self) -> RawWindowHandle {
        #[cfg(target_os = "linux")]
        return self.inner.raw_handle();
        #[cfg(not(target_os = "linux"))]
        unreachable!("no window backend for this OS yet")
    }
}
