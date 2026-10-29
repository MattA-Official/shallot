//! X11 window backend, through `libX11.so.6` loaded at run time.

use crate::{Event, Key, Library, MouseButton, PlatformError, RawWindowHandle, WindowConfig};
use std::collections::HashSet;
use std::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void, CString};

type Display = c_void;
type XWindow = c_ulong;
type Atom = c_ulong;
type Bool = c_int;

const KEY_PRESS: c_int = 2;
const KEY_RELEASE: c_int = 3;
const BUTTON_PRESS: c_int = 4;
const BUTTON_RELEASE: c_int = 5;
const MOTION_NOTIFY: c_int = 6;
const ENTER_NOTIFY: c_int = 7;
const LEAVE_NOTIFY: c_int = 8;
const FOCUS_IN: c_int = 9;
const FOCUS_OUT: c_int = 10;
const CONFIGURE_NOTIFY: c_int = 22;
const CLIENT_MESSAGE: c_int = 33;

const KEY_PRESS_MASK: c_long = 1 << 0;
const KEY_RELEASE_MASK: c_long = 1 << 1;
const BUTTON_PRESS_MASK: c_long = 1 << 2;
const BUTTON_RELEASE_MASK: c_long = 1 << 3;
const ENTER_WINDOW_MASK: c_long = 1 << 4;
const LEAVE_WINDOW_MASK: c_long = 1 << 5;
const POINTER_MOTION_MASK: c_long = 1 << 6;
const EXPOSURE_MASK: c_long = 1 << 15;
const STRUCTURE_NOTIFY_MASK: c_long = 1 << 17;
const FOCUS_CHANGE_MASK: c_long = 1 << 21;

const P_MIN_SIZE: c_long = 1 << 4;
const P_MAX_SIZE: c_long = 1 << 5;
const PROP_MODE_REPLACE: c_int = 0;

#[repr(C)]
#[derive(Clone, Copy)]
struct XAnyEvent {
    kind: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    window: XWindow,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct XKeyEvent {
    kind: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    window: XWindow,
    root: XWindow,
    subwindow: XWindow,
    time: c_ulong,
    x: c_int,
    y: c_int,
    x_root: c_int,
    y_root: c_int,
    state: c_uint,
    keycode: c_uint,
    same_screen: Bool,
}

/// `XButtonEvent` and `XMotionEvent` share this prefix; motion events never read `button`.
#[repr(C)]
#[derive(Clone, Copy)]
struct XPointerEvent {
    kind: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    window: XWindow,
    root: XWindow,
    subwindow: XWindow,
    time: c_ulong,
    x: c_int,
    y: c_int,
    x_root: c_int,
    y_root: c_int,
    state: c_uint,
    button: c_uint,
    same_screen: Bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct XConfigureEvent {
    kind: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    event: XWindow,
    window: XWindow,
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
    border_width: c_int,
    above: XWindow,
    override_redirect: Bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct XClientMessageEvent {
    kind: c_int,
    serial: c_ulong,
    send_event: Bool,
    display: *mut Display,
    window: XWindow,
    message_type: Atom,
    format: c_int,
    data: [c_long; 5],
}

#[repr(C)]
union XEvent {
    kind: c_int,
    any: XAnyEvent,
    key: XKeyEvent,
    pointer: XPointerEvent,
    configure: XConfigureEvent,
    client: XClientMessageEvent,
    pad: [c_long; 24],
}

#[repr(C)]
struct XSizeHints {
    flags: c_long,
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
    min_width: c_int,
    min_height: c_int,
    max_width: c_int,
    max_height: c_int,
    width_inc: c_int,
    height_inc: c_int,
    min_aspect: [c_int; 2],
    max_aspect: [c_int; 2],
    base_width: c_int,
    base_height: c_int,
    win_gravity: c_int,
}

struct Xlib {
    _lib: Library,
    open_display: unsafe extern "C" fn(*const c_char) -> *mut Display,
    close_display: unsafe extern "C" fn(*mut Display) -> c_int,
    default_screen: unsafe extern "C" fn(*mut Display) -> c_int,
    root_window: unsafe extern "C" fn(*mut Display, c_int) -> XWindow,
    black_pixel: unsafe extern "C" fn(*mut Display, c_int) -> c_ulong,
    create_simple_window: unsafe extern "C" fn(
        *mut Display,
        XWindow,
        c_int,
        c_int,
        c_uint,
        c_uint,
        c_uint,
        c_ulong,
        c_ulong,
    ) -> XWindow,
    destroy_window: unsafe extern "C" fn(*mut Display, XWindow) -> c_int,
    select_input: unsafe extern "C" fn(*mut Display, XWindow, c_long) -> c_int,
    store_name: unsafe extern "C" fn(*mut Display, XWindow, *const c_char) -> c_int,
    change_property: unsafe extern "C" fn(
        *mut Display,
        XWindow,
        Atom,
        Atom,
        c_int,
        c_int,
        *const u8,
        c_int,
    ) -> c_int,
    map_window: unsafe extern "C" fn(*mut Display, XWindow) -> c_int,
    intern_atom: unsafe extern "C" fn(*mut Display, *const c_char, Bool) -> Atom,
    set_wm_protocols: unsafe extern "C" fn(*mut Display, XWindow, *mut Atom, c_int) -> c_int,
    set_wm_normal_hints: unsafe extern "C" fn(*mut Display, XWindow, *mut XSizeHints),
    pending: unsafe extern "C" fn(*mut Display) -> c_int,
    next_event: unsafe extern "C" fn(*mut Display, *mut XEvent) -> c_int,
    peek_event: unsafe extern "C" fn(*mut Display, *mut XEvent) -> c_int,
    lookup_string: unsafe extern "C" fn(
        *mut XKeyEvent,
        *mut c_char,
        c_int,
        *mut c_ulong,
        *mut c_void,
    ) -> c_int,
    flush: unsafe extern "C" fn(*mut Display) -> c_int,
    xkb_set_detectable_auto_repeat:
        Option<unsafe extern "C" fn(*mut Display, Bool, *mut Bool) -> Bool>,
}

impl Xlib {
    fn load() -> Result<Xlib, PlatformError> {
        let lib = Library::open(&["libX11.so.6", "libX11.so"])?;
        unsafe {
            Ok(Xlib {
                open_display: lib.require("XOpenDisplay")?,
                close_display: lib.require("XCloseDisplay")?,
                default_screen: lib.require("XDefaultScreen")?,
                root_window: lib.require("XRootWindow")?,
                black_pixel: lib.require("XBlackPixel")?,
                create_simple_window: lib.require("XCreateSimpleWindow")?,
                destroy_window: lib.require("XDestroyWindow")?,
                select_input: lib.require("XSelectInput")?,
                store_name: lib.require("XStoreName")?,
                change_property: lib.require("XChangeProperty")?,
                map_window: lib.require("XMapWindow")?,
                intern_atom: lib.require("XInternAtom")?,
                set_wm_protocols: lib.require("XSetWMProtocols")?,
                set_wm_normal_hints: lib.require("XSetWMNormalHints")?,
                pending: lib.require("XPending")?,
                next_event: lib.require("XNextEvent")?,
                peek_event: lib.require("XPeekEvent")?,
                lookup_string: lib.require("XLookupString")?,
                flush: lib.require("XFlush")?,
                xkb_set_detectable_auto_repeat: lib.get("XkbSetDetectableAutoRepeat"),
                _lib: lib,
            })
        }
    }
}

pub struct Window {
    xlib: Xlib,
    display: *mut Display,
    window: XWindow,
    wm_delete_window: Atom,
    net_wm_name: Atom,
    utf8_string: Atom,
    width: u32,
    height: u32,
    held: HashSet<u32>,
    /// Without detectable auto-repeat, X sends a release and press pair per repeat.
    detectable_repeat: bool,
}

impl Window {
    pub fn new(config: &WindowConfig) -> Result<Window, PlatformError> {
        let xlib = Xlib::load()?;
        unsafe {
            let display = (xlib.open_display)(std::ptr::null());
            if display.is_null() {
                return Err(PlatformError::new(
                    "could not open the X display (is DISPLAY set?)",
                ));
            }
            let screen = (xlib.default_screen)(display);
            let root = (xlib.root_window)(display, screen);
            let black = (xlib.black_pixel)(display, screen);
            let width = config.width.max(1);
            let height = config.height.max(1);
            let window =
                (xlib.create_simple_window)(display, root, 0, 0, width, height, 0, black, black);
            if window == 0 {
                (xlib.close_display)(display);
                return Err(PlatformError::new("XCreateSimpleWindow failed"));
            }
            (xlib.select_input)(
                display,
                window,
                KEY_PRESS_MASK
                    | KEY_RELEASE_MASK
                    | BUTTON_PRESS_MASK
                    | BUTTON_RELEASE_MASK
                    | ENTER_WINDOW_MASK
                    | LEAVE_WINDOW_MASK
                    | POINTER_MOTION_MASK
                    | EXPOSURE_MASK
                    | STRUCTURE_NOTIFY_MASK
                    | FOCUS_CHANGE_MASK,
            );

            let atom = |name: &str| {
                let name = CString::new(name).unwrap();
                (xlib.intern_atom)(display, name.as_ptr(), 0)
            };
            let mut wm_delete_window = atom("WM_DELETE_WINDOW");
            (xlib.set_wm_protocols)(display, window, &mut wm_delete_window, 1);
            let net_wm_name = atom("_NET_WM_NAME");
            let utf8_string = atom("UTF8_STRING");

            if !config.resizable {
                let mut hints: XSizeHints = std::mem::zeroed();
                hints.flags = P_MIN_SIZE | P_MAX_SIZE;
                hints.min_width = width as c_int;
                hints.max_width = width as c_int;
                hints.min_height = height as c_int;
                hints.max_height = height as c_int;
                (xlib.set_wm_normal_hints)(display, window, &mut hints);
            }

            let mut detectable_repeat = false;
            if let Some(set_repeat) = xlib.xkb_set_detectable_auto_repeat {
                let mut supported = 0;
                detectable_repeat = set_repeat(display, 1, &mut supported) != 0 && supported != 0;
            }

            let mut this = Window {
                xlib,
                display,
                window,
                wm_delete_window,
                net_wm_name,
                utf8_string,
                width,
                height,
                held: HashSet::new(),
                detectable_repeat,
            };
            this.set_title(&config.title);
            (this.xlib.map_window)(display, window);
            (this.xlib.flush)(display);
            Ok(this)
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn raw_handle(&self) -> RawWindowHandle {
        RawWindowHandle::Xlib {
            display: self.display,
            window: self.window,
        }
    }

    pub fn set_title(&mut self, title: &str) {
        let title = CString::new(title.replace('\0', "")).unwrap();
        unsafe {
            (self.xlib.store_name)(self.display, self.window, title.as_ptr());
            let bytes = title.as_bytes();
            (self.xlib.change_property)(
                self.display,
                self.window,
                self.net_wm_name,
                self.utf8_string,
                8,
                PROP_MODE_REPLACE,
                bytes.as_ptr(),
                bytes.len() as c_int,
            );
            (self.xlib.flush)(self.display);
        }
    }

    pub fn poll_events(&mut self, events: &mut Vec<Event>) {
        unsafe {
            while (self.xlib.pending)(self.display) > 0 {
                let mut event: XEvent = std::mem::zeroed();
                (self.xlib.next_event)(self.display, &mut event);
                if event.any.window != self.window {
                    continue;
                }
                self.translate(&mut event, events);
            }
        }
    }

    unsafe fn translate(&mut self, event: &mut XEvent, events: &mut Vec<Event>) {
        match event.kind {
            KEY_PRESS => {
                let code = event.key.keycode;
                let repeat = !self.held.insert(code);
                if let Some(key) = Key::from_evdev(code.saturating_sub(8)) {
                    events.push(Event::KeyDown { key, repeat });
                }
                let mut buffer = [0 as c_char; 32];
                let mut keysym = 0;
                let len = (self.xlib.lookup_string)(
                    &mut event.key,
                    buffer.as_mut_ptr(),
                    buffer.len() as c_int,
                    &mut keysym,
                    std::ptr::null_mut(),
                );
                // XLookupString yields Latin-1; control characters are not text.
                for &byte in &buffer[..len.max(0) as usize] {
                    let ch = char::from(byte as u8);
                    if !ch.is_control() {
                        events.push(Event::Text(ch));
                    }
                }
            }
            KEY_RELEASE => {
                let code = event.key.keycode;
                if !self.detectable_repeat && self.next_is_repeat_of(&event.key) {
                    return;
                }
                self.held.remove(&code);
                if let Some(key) = Key::from_evdev(code.saturating_sub(8)) {
                    events.push(Event::KeyUp { key });
                }
            }
            BUTTON_PRESS | BUTTON_RELEASE => {
                let pressed = event.kind == BUTTON_PRESS;
                match event.pointer.button {
                    // Buttons 4 to 7 are wheel steps and only press.
                    4..=7 if pressed => {
                        let (x, y) = match event.pointer.button {
                            4 => (0.0, -1.0),
                            5 => (0.0, 1.0),
                            6 => (-1.0, 0.0),
                            _ => (1.0, 0.0),
                        };
                        events.push(Event::Wheel { x, y });
                    }
                    4..=7 => {}
                    n => {
                        let button = match n {
                            1 => MouseButton::Left,
                            2 => MouseButton::Middle,
                            3 => MouseButton::Right,
                            8 => MouseButton::Back,
                            9 => MouseButton::Forward,
                            n => MouseButton::Other(n as u16),
                        };
                        events.push(Event::MouseButton { button, pressed });
                    }
                }
            }
            MOTION_NOTIFY => {
                events.push(Event::PointerMoved {
                    x: event.pointer.x as f64,
                    y: event.pointer.y as f64,
                });
            }
            ENTER_NOTIFY => events.push(Event::PointerInside(true)),
            LEAVE_NOTIFY => events.push(Event::PointerInside(false)),
            FOCUS_IN => events.push(Event::Focused(true)),
            FOCUS_OUT => {
                self.held.clear();
                events.push(Event::Focused(false));
            }
            CONFIGURE_NOTIFY => {
                let width = event.configure.width.max(1) as u32;
                let height = event.configure.height.max(1) as u32;
                if (width, height) != (self.width, self.height) {
                    self.width = width;
                    self.height = height;
                    events.push(Event::Resized { width, height });
                }
            }
            CLIENT_MESSAGE if event.client.data[0] as Atom == self.wm_delete_window => {
                events.push(Event::CloseRequested);
            }
            _ => {}
        }
    }

    /// Whether a release is half of a synthetic auto-repeat: a press of the same key at the same
    /// instant is already queued behind it.
    unsafe fn next_is_repeat_of(&mut self, release: &XKeyEvent) -> bool {
        if (self.xlib.pending)(self.display) == 0 {
            return false;
        }
        let mut next: XEvent = std::mem::zeroed();
        (self.xlib.peek_event)(self.display, &mut next);
        next.kind == KEY_PRESS
            && next.key.keycode == release.keycode
            && next.key.time == release.time
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            (self.xlib.destroy_window)(self.display, self.window);
            (self.xlib.close_display)(self.display);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// Sizes and offsets from Xlib.h on x86_64 and aarch64 Linux.
    #[test]
    fn layouts_match_xlib() {
        assert_eq!(size_of::<XEvent>(), 192);
        assert_eq!(size_of::<XKeyEvent>(), 96);
        assert_eq!(size_of::<XPointerEvent>(), 96);
        assert_eq!(size_of::<XConfigureEvent>(), 88);
        assert_eq!(offset_of!(XConfigureEvent, width), 56);
        assert_eq!(size_of::<XClientMessageEvent>(), 96);
        assert_eq!(offset_of!(XClientMessageEvent, data), 56);
        assert_eq!(size_of::<XSizeHints>(), 80);
    }
}
