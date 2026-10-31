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
    /// A Win32 `HINSTANCE` and `HWND`.
    Win32 {
        hinstance: *mut c_void,
        hwnd: *mut c_void,
    },
    /// An AppKit `NSView *` and the `CAMetalLayer *` backing it.
    AppKit {
        ns_view: *mut c_void,
        ca_metal_layer: *mut c_void,
    },
}

#[cfg(target_os = "macos")]
use crate::cocoa as backend;
#[cfg(windows)]
use crate::win32 as backend;
#[cfg(target_os = "linux")]
use crate::x11 as backend;

/// A top-level OS window.
pub struct Window {
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    inner: backend::Window,
}

impl Window {
    /// Opens and shows a window.
    pub fn new(config: &WindowConfig) -> Result<Window, PlatformError> {
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        {
            Ok(Window {
                inner: backend::Window::new(config)?,
            })
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            let _ = config;
            Err(PlatformError::new("no window backend for this OS yet"))
        }
    }

    /// Appends every pending event to `events` without blocking.
    pub fn poll_events(&mut self, events: &mut Vec<Event>) {
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        self.inner.poll_events(events);
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        let _ = events;
    }

    /// The drawable size in physical pixels, as of the last processed event.
    pub fn size(&self) -> (u32, u32) {
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        return self.inner.size();
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        (0, 0)
    }

    pub fn set_title(&mut self, title: &str) {
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        self.inner.set_title(title);
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        let _ = title;
    }

    /// The OS handles for creating a GPU surface. Valid while the window lives.
    pub fn raw_handle(&self) -> RawWindowHandle {
        #[cfg(any(target_os = "linux", target_os = "macos", windows))]
        return self.inner.raw_handle();
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        unreachable!("no window backend for this OS yet")
    }
}
