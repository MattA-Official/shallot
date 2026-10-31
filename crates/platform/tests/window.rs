//! Opens a real window through the public API, on the main thread as AppKit requires. Skips when
//! there is no display or window server, as on a headless machine.

use shallot_platform::{Event, RawWindowHandle, Window, WindowConfig};

fn main() {
    let mut window = match Window::new(&WindowConfig::new("shallot window test", 320, 200)) {
        Ok(window) => window,
        Err(error) => {
            println!("window test skipped: {error}");
            return;
        }
    };
    let (width, height) = window.size();
    assert!(width >= 1 && height >= 1, "{width}x{height}");
    window.set_title("shallot window test, renamed");
    match window.raw_handle() {
        RawWindowHandle::Xlib { display, window } => assert!(!display.is_null() && window != 0),
        RawWindowHandle::Win32 { hwnd, .. } => assert!(!hwnd.is_null()),
        RawWindowHandle::AppKit {
            ns_view,
            ca_metal_layer,
        } => assert!(!ns_view.is_null() && !ca_metal_layer.is_null()),
    }
    let mut events = Vec::new();
    for _ in 0..10 {
        window.poll_events(&mut events);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!events.contains(&Event::CloseRequested), "{events:?}");
    println!(
        "window test passed: {width}x{height}, {} events",
        events.len()
    );
}
