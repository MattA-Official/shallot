//! Win32 window backend, through `user32.dll` loaded at run time.
//!
//! The window procedure turns messages into [`Event`]s queued on the window's state, which
//! [`Window::poll_events`] drains after pumping the thread's message queue. The process is made
//! per-monitor DPI aware (v2), so sizes and pointer positions are physical pixels.

use crate::{Event, Key, Library, MouseButton, PlatformError, RawWindowHandle, WindowConfig};
use std::ffi::c_void;
use std::sync::OnceLock;

type Hwnd = *mut c_void;
type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

const WM_DESTROY: u32 = 0x0002;
const WM_SIZE: u32 = 0x0005;
const WM_SETFOCUS: u32 = 0x0007;
const WM_KILLFOCUS: u32 = 0x0008;
const WM_CLOSE: u32 = 0x0010;
const WM_ERASEBKGND: u32 = 0x0014;
const WM_NCCREATE: u32 = 0x0081;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_CHAR: u32 = 0x0102;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_MOUSEMOVE: u32 = 0x0200;
const WM_LBUTTONDOWN: u32 = 0x0201;
const WM_LBUTTONUP: u32 = 0x0202;
const WM_RBUTTONDOWN: u32 = 0x0204;
const WM_RBUTTONUP: u32 = 0x0205;
const WM_MBUTTONDOWN: u32 = 0x0207;
const WM_MBUTTONUP: u32 = 0x0208;
const WM_MOUSEWHEEL: u32 = 0x020A;
const WM_XBUTTONDOWN: u32 = 0x020B;
const WM_XBUTTONUP: u32 = 0x020C;
const WM_MOUSEHWHEEL: u32 = 0x020E;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_DPICHANGED: u32 = 0x02E0;

const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const WS_THICKFRAME: u32 = 0x0004_0000;
const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
const CS_HREDRAW: u32 = 0x0002;
const CS_VREDRAW: u32 = 0x0001;
const CW_USEDEFAULT: i32 = 0x8000_0000_u32 as i32;
const SW_SHOW: i32 = 5;
const SIZE_MINIMIZED: usize = 1;
const PM_REMOVE: u32 = 1;
const IDC_ARROW: usize = 32512;
const TME_LEAVE: u32 = 2;
const GWLP_USERDATA: i32 = -21;
const SWP_NOZORDER: u32 = 0x0004;
const SWP_NOMOVE: u32 = 0x0002;
const SWP_NOACTIVATE: u32 = 0x0010;
const ERROR_CLASS_ALREADY_EXISTS: u32 = 1410;
const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
const VK_PAUSE: usize = 0x13;

#[repr(C)]
struct WndClassExW {
    size: u32,
    style: u32,
    wnd_proc: WndProc,
    cls_extra: i32,
    wnd_extra: i32,
    instance: *mut c_void,
    icon: *mut c_void,
    cursor: *mut c_void,
    background: *mut c_void,
    menu_name: *const u16,
    class_name: *const u16,
    icon_small: *mut c_void,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt: Point,
    private: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
struct TrackMouseEvent {
    size: u32,
    flags: u32,
    track: Hwnd,
    hover_time: u32,
}

/// The first member of `CREATESTRUCTW`, the only one read.
#[repr(C)]
struct CreateStruct {
    create_params: *mut c_void,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    fn GetLastError() -> u32;
}

// `SetWindowLongPtrW` is a macro over `SetWindowLongW` in 32-bit headers.
#[cfg(target_pointer_width = "64")]
const WINDOW_LONG: [&str; 2] = ["SetWindowLongPtrW", "GetWindowLongPtrW"];
#[cfg(not(target_pointer_width = "64"))]
const WINDOW_LONG: [&str; 2] = ["SetWindowLongW", "GetWindowLongW"];

macro_rules! user32 {
    ($($field:ident = $name:expr => fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        /// The `user32.dll` entry points used, loaded once per process. The library stays
        /// loaded for the life of the process, as it does for any GUI program.
        struct User32 {
            $($field: unsafe extern "system" fn($($arg),*) $(-> $ret)?,)*
            set_dpi_awareness_context: Option<unsafe extern "system" fn(isize) -> i32>,
        }

        impl User32 {
            fn load() -> Result<User32, PlatformError> {
                let library = Library::open(&["user32.dll"])?;
                let user32 = unsafe {
                    User32 {
                        $($field: library.require($name)?,)*
                        set_dpi_awareness_context: library.get("SetProcessDpiAwarenessContext"),
                    }
                };
                std::mem::forget(library);
                Ok(user32)
            }
        }
    };
}

user32! {
    register_class = "RegisterClassExW" => fn(*const WndClassExW) -> u16;
    create_window = "CreateWindowExW" => fn(
        u32, *const u16, *const u16, u32, i32, i32, i32, i32, Hwnd, *mut c_void, *mut c_void,
        *mut c_void
    ) -> Hwnd;
    destroy_window = "DestroyWindow" => fn(Hwnd) -> i32;
    show_window = "ShowWindow" => fn(Hwnd, i32) -> i32;
    def_window_proc = "DefWindowProcW" => fn(Hwnd, u32, usize, isize) -> isize;
    peek_message = "PeekMessageW" => fn(*mut Msg, Hwnd, u32, u32, u32) -> i32;
    translate_message = "TranslateMessage" => fn(*const Msg) -> i32;
    dispatch_message = "DispatchMessageW" => fn(*const Msg) -> isize;
    set_window_text = "SetWindowTextW" => fn(Hwnd, *const u16) -> i32;
    get_client_rect = "GetClientRect" => fn(Hwnd, *mut Rect) -> i32;
    get_window_rect = "GetWindowRect" => fn(Hwnd, *mut Rect) -> i32;
    set_window_pos = "SetWindowPos" => fn(Hwnd, Hwnd, i32, i32, i32, i32, u32) -> i32;
    load_cursor = "LoadCursorW" => fn(*mut c_void, usize) -> *mut c_void;
    track_mouse_event = "TrackMouseEvent" => fn(*mut TrackMouseEvent) -> i32;
    set_capture = "SetCapture" => fn(Hwnd) -> Hwnd;
    release_capture = "ReleaseCapture" => fn() -> i32;
    set_window_long = WINDOW_LONG[0] => fn(Hwnd, i32, isize) -> isize;
    get_window_long = WINDOW_LONG[1] => fn(Hwnd, i32) -> isize;
}

static USER32: OnceLock<Result<User32, PlatformError>> = OnceLock::new();

fn user32() -> Result<&'static User32, PlatformError> {
    USER32
        .get_or_init(User32::load)
        .as_ref()
        .map_err(Clone::clone)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16()
        .filter(|&c| c != 0)
        .chain(Some(0))
        .collect()
}

/// What the window procedure updates. Boxed, so its address is stable while the window lives.
struct State {
    user32: &'static User32,
    events: Vec<Event>,
    width: u32,
    height: u32,
    pointer_inside: bool,
    /// Mouse buttons held, which keep the pointer captured.
    buttons: u32,
    /// A UTF-16 high surrogate waiting for its low half.
    surrogate: Option<u16>,
}

pub struct Window {
    hwnd: Hwnd,
    instance: *mut c_void,
    state: Box<State>,
}

const CLASS_NAME: &str = "shallot-window";

impl Window {
    pub fn new(config: &WindowConfig) -> Result<Window, PlatformError> {
        let user32 = user32()?;
        unsafe {
            if let Some(set_awareness) = user32.set_dpi_awareness_context {
                // Fails harmlessly when the process's awareness is already set.
                set_awareness(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            }
            let instance = GetModuleHandleW(std::ptr::null());
            let class_name = wide(CLASS_NAME);
            let class = WndClassExW {
                size: std::mem::size_of::<WndClassExW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                wnd_proc: window_proc,
                cls_extra: 0,
                wnd_extra: 0,
                instance,
                icon: std::ptr::null_mut(),
                cursor: (user32.load_cursor)(std::ptr::null_mut(), IDC_ARROW),
                background: std::ptr::null_mut(),
                menu_name: std::ptr::null(),
                class_name: class_name.as_ptr(),
                icon_small: std::ptr::null_mut(),
            };
            if (user32.register_class)(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS
            {
                return Err(PlatformError::new("RegisterClassExW failed"));
            }

            let width = config.width.max(1);
            let height = config.height.max(1);
            let mut state = Box::new(State {
                user32,
                events: Vec::new(),
                width,
                height,
                pointer_inside: false,
                buttons: 0,
                surrogate: None,
            });
            let mut style = WS_OVERLAPPEDWINDOW;
            if !config.resizable {
                style &= !(WS_THICKFRAME | WS_MAXIMIZEBOX);
            }
            let title = wide(&config.title);
            let hwnd = (state.user32.create_window)(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                width as i32,
                height as i32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                &mut *state as *mut State as *mut c_void,
            );
            if hwnd.is_null() {
                return Err(PlatformError::new("CreateWindowExW failed"));
            }
            let window = Window {
                hwnd,
                instance,
                state,
            };
            // The size given covers the frame; grow the window until the client area is the
            // requested size, at whatever DPI the window landed on.
            window.set_client_size(width, height);
            (window.state.user32.show_window)(hwnd, SW_SHOW);
            Ok(window)
        }
    }

    unsafe fn set_client_size(&self, width: u32, height: u32) {
        let user32 = &self.state.user32;
        let (mut client, mut frame) = (Rect::default(), Rect::default());
        (user32.get_client_rect)(self.hwnd, &mut client);
        (user32.get_window_rect)(self.hwnd, &mut frame);
        let extra_w = (frame.right - frame.left) - (client.right - client.left);
        let extra_h = (frame.bottom - frame.top) - (client.bottom - client.top);
        (user32.set_window_pos)(
            self.hwnd,
            std::ptr::null_mut(),
            0,
            0,
            width as i32 + extra_w,
            height as i32 + extra_h,
            SWP_NOZORDER | SWP_NOMOVE | SWP_NOACTIVATE,
        );
    }

    pub fn size(&self) -> (u32, u32) {
        (self.state.width, self.state.height)
    }

    pub fn raw_handle(&self) -> RawWindowHandle {
        RawWindowHandle::Win32 {
            hinstance: self.instance,
            hwnd: self.hwnd,
        }
    }

    pub fn set_title(&mut self, title: &str) {
        let title = wide(title);
        unsafe { (self.state.user32.set_window_text)(self.hwnd, title.as_ptr()) };
    }

    pub fn poll_events(&mut self, events: &mut Vec<Event>) {
        unsafe {
            let mut msg: Msg = std::mem::zeroed();
            while (self.state.user32.peek_message)(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE)
                != 0
            {
                (self.state.user32.translate_message)(&msg);
                (self.state.user32.dispatch_message)(&msg);
            }
        }
        events.append(&mut self.state.events);
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            (self.state.user32.destroy_window)(self.hwnd);
        }
    }
}

fn low_word(value: isize) -> i16 {
    value as u16 as i16
}

fn high_word(value: isize) -> i16 {
    (value >> 16) as u16 as i16
}

/// Maps a key message's scan code (`lParam` bits 16 to 23) and extended flag (bit 24) to a
/// key. Set-1 scan codes without the extended prefix match evdev codes 1 to 88.
pub(crate) fn key_from_scan_code(scan: u32, extended: bool, virtual_key: usize) -> Option<Key> {
    use Key::*;
    // Pause sends the scan code of NumLock without the prefix, and NumLock sends it with it.
    if virtual_key == VK_PAUSE {
        return Some(Pause);
    }
    if !extended {
        return match scan {
            0x45 => Some(NumLock),
            0x54 => Some(PrintScreen),
            1..=88 => Key::from_evdev(scan),
            _ => None,
        };
    }
    Some(match scan {
        0x1C => NumpadEnter,
        0x1D => ControlRight,
        0x35 => NumpadDivide,
        0x37 => PrintScreen,
        0x38 => AltRight,
        0x45 => NumLock,
        0x47 => Home,
        0x48 => ArrowUp,
        0x49 => PageUp,
        0x4B => ArrowLeft,
        0x4D => ArrowRight,
        0x4F => End,
        0x50 => ArrowDown,
        0x51 => PageDown,
        0x52 => Insert,
        0x53 => Delete,
        0x5B => MetaLeft,
        0x5C => MetaRight,
        0x5D => ContextMenu,
        _ => return None,
    })
}

unsafe extern "system" fn window_proc(
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> isize {
    // The window exists only after `user32()` loaded, so this cannot fail.
    let Ok(user32) = user32() else { return 0 };
    if message == WM_NCCREATE {
        let create = &*(lparam as *const CreateStruct);
        (user32.set_window_long)(hwnd, GWLP_USERDATA, create.create_params as isize);
    }
    let state = (user32.get_window_long)(hwnd, GWLP_USERDATA) as *mut State;
    if !state.is_null() {
        if let Some(result) = handle(&mut *state, hwnd, message, wparam, lparam) {
            return result;
        }
    }
    (user32.def_window_proc)(hwnd, message, wparam, lparam)
}

/// Handles a message, or returns `None` to leave it to `DefWindowProcW`.
unsafe fn handle(
    state: &mut State,
    hwnd: Hwnd,
    message: u32,
    wparam: usize,
    lparam: isize,
) -> Option<isize> {
    let events = &mut state.events;
    match message {
        WM_CLOSE => {
            // Closing is the application's decision: the window stays until it is dropped.
            events.push(Event::CloseRequested);
            Some(0)
        }
        WM_DESTROY => {
            (state.user32.set_window_long)(hwnd, GWLP_USERDATA, 0);
            Some(0)
        }
        WM_ERASEBKGND => Some(1),
        WM_SIZE => {
            if wparam != SIZE_MINIMIZED {
                let width = (lparam as u32 & 0xFFFF).max(1);
                let height = ((lparam as u32 >> 16) & 0xFFFF).max(1);
                if (width, height) != (state.width, state.height) {
                    state.width = width;
                    state.height = height;
                    events.push(Event::Resized { width, height });
                }
            }
            Some(0)
        }
        WM_DPICHANGED => {
            // Move to the rectangle Windows suggests for the new scale.
            let rect = &*(lparam as *const Rect);
            (state.user32.set_window_pos)(
                hwnd,
                std::ptr::null_mut(),
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
            Some(0)
        }
        WM_SETFOCUS => {
            events.push(Event::Focused(true));
            Some(0)
        }
        WM_KILLFOCUS => {
            events.push(Event::Focused(false));
            Some(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP => {
            let scan = (lparam as u32 >> 16) & 0xFF;
            let extended = (lparam as u32 >> 24) & 1 != 0;
            if let Some(key) = key_from_scan_code(scan, extended, wparam) {
                if matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN) {
                    let repeat = (lparam as u32 >> 30) & 1 != 0;
                    events.push(Event::KeyDown { key, repeat });
                } else {
                    events.push(Event::KeyUp { key });
                }
            }
            // System keys go on to the default handler, which turns Alt+F4 into WM_CLOSE.
            if matches!(message, WM_SYSKEYDOWN | WM_SYSKEYUP) {
                None
            } else {
                Some(0)
            }
        }
        WM_CHAR => {
            let unit = wparam as u16;
            let ch = match unit {
                0xD800..=0xDBFF => {
                    state.surrogate = Some(unit);
                    None
                }
                0xDC00..=0xDFFF => state
                    .surrogate
                    .take()
                    .and_then(|high| char::decode_utf16([high, unit]).next().and_then(Result::ok)),
                _ => char::from_u32(unit as u32),
            };
            if let Some(ch) = ch.filter(|c| !c.is_control()) {
                events.push(Event::Text(ch));
            }
            Some(0)
        }
        WM_MOUSEMOVE => {
            if !state.pointer_inside {
                state.pointer_inside = true;
                let mut track = TrackMouseEvent {
                    size: std::mem::size_of::<TrackMouseEvent>() as u32,
                    flags: TME_LEAVE,
                    track: hwnd,
                    hover_time: 0,
                };
                (state.user32.track_mouse_event)(&mut track);
                events.push(Event::PointerInside(true));
            }
            events.push(Event::PointerMoved {
                x: low_word(lparam) as f64,
                y: high_word(lparam) as f64,
            });
            Some(0)
        }
        WM_MOUSELEAVE => {
            state.pointer_inside = false;
            events.push(Event::PointerInside(false));
            Some(0)
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONDOWN | WM_RBUTTONUP | WM_MBUTTONDOWN
        | WM_MBUTTONUP | WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let (button, pressed) = match message {
                WM_LBUTTONDOWN => (MouseButton::Left, true),
                WM_LBUTTONUP => (MouseButton::Left, false),
                WM_RBUTTONDOWN => (MouseButton::Right, true),
                WM_RBUTTONUP => (MouseButton::Right, false),
                WM_MBUTTONDOWN => (MouseButton::Middle, true),
                WM_MBUTTONUP => (MouseButton::Middle, false),
                _ => {
                    let button = match (wparam >> 16) & 0xFFFF {
                        1 => MouseButton::Back,
                        2 => MouseButton::Forward,
                        n => MouseButton::Other(n as u16 + 2),
                    };
                    (button, message == WM_XBUTTONDOWN)
                }
            };
            // Capture while any button is held, so drags outside the window still release.
            let bit = 1 << button.index().min(31);
            if pressed {
                if state.buttons == 0 {
                    (state.user32.set_capture)(hwnd);
                }
                state.buttons |= bit;
            } else {
                state.buttons &= !bit;
                if state.buttons == 0 {
                    (state.user32.release_capture)();
                }
            }
            events.push(Event::MouseButton { button, pressed });
            // X buttons report having been handled by returning TRUE.
            Some(matches!(message, WM_XBUTTONDOWN | WM_XBUTTONUP) as isize)
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            // One notch is 120 units; vertical deltas are positive away from the user, the
            // opposite of DOM `deltaY`.
            let delta = (wparam >> 16) as u16 as i16 as f64 / 120.0;
            events.push(if message == WM_MOUSEWHEEL {
                Event::Wheel { x: 0.0, y: -delta }
            } else {
                Event::Wheel { x: delta, y: 0.0 }
            });
            Some(0)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_codes_map_to_dom_codes() {
        assert_eq!(key_from_scan_code(0x11, false, 0), Some(Key::KeyW));
        assert_eq!(key_from_scan_code(0x1D, false, 0), Some(Key::ControlLeft));
        assert_eq!(key_from_scan_code(0x1D, true, 0), Some(Key::ControlRight));
        assert_eq!(key_from_scan_code(0x48, false, 0), Some(Key::Numpad8));
        assert_eq!(key_from_scan_code(0x48, true, 0), Some(Key::ArrowUp));
        assert_eq!(key_from_scan_code(0x45, false, VK_PAUSE), Some(Key::Pause));
        assert_eq!(key_from_scan_code(0x45, true, 0x90), Some(Key::NumLock));
        assert_eq!(key_from_scan_code(0x5B, true, 0), Some(Key::MetaLeft));
    }

    #[test]
    fn every_key_has_a_scan_code() {
        let mut seen = std::collections::HashSet::new();
        for extended in [false, true] {
            for scan in 0..256 {
                if let Some(key) = key_from_scan_code(scan, extended, 0) {
                    seen.insert(key);
                }
            }
        }
        seen.insert(Key::Pause);
        assert_eq!(seen.len(), Key::ALL.len());
    }

    /// Posts messages as the OS would and checks what comes out. Skips when no window can
    /// be created, as on a desktop-less session.
    #[test]
    fn messages_become_events() {
        let Ok(mut window) = Window::new(&WindowConfig::new("test", 320, 200)) else {
            eprintln!("no desktop; skipping");
            return;
        };
        assert_eq!(window.size(), (320, 200));
        window.set_title("renamed");
        let library = Library::open(&["user32.dll"]).unwrap();
        let post: unsafe extern "system" fn(Hwnd, u32, usize, isize) -> i32 =
            unsafe { library.require("PostMessageW").unwrap() };
        let key = |scan: isize, extended: bool, up: bool| {
            scan << 16 | (extended as isize) << 24 | (up as isize) << 30 | (up as isize) << 31 | 1
        };
        let mut events = Vec::new();
        window.poll_events(&mut events);
        events.clear();
        unsafe {
            post(window.hwnd, WM_KEYDOWN, 0x57, key(0x11, false, false));
            post(window.hwnd, WM_KEYUP, 0x57, key(0x11, false, true));
            post(window.hwnd, WM_KEYDOWN, 0x26, key(0x48, true, false));
            post(window.hwnd, WM_MOUSEMOVE, 0, 30 << 16 | 20);
            post(window.hwnd, WM_LBUTTONDOWN, 1, 30 << 16 | 20);
            post(window.hwnd, WM_LBUTTONUP, 0, 30 << 16 | 20);
            post(
                window.hwnd,
                WM_MOUSEWHEEL,
                (-120i16 as u16 as usize) << 16,
                0,
            );
            post(window.hwnd, WM_CLOSE, 0, 0);
        }
        window.poll_events(&mut events);
        events.retain(|e| !matches!(e, Event::Focused(_) | Event::PointerInside(_)));
        assert_eq!(
            events,
            [
                Event::KeyDown {
                    key: Key::KeyW,
                    repeat: false
                },
                Event::KeyUp { key: Key::KeyW },
                Event::KeyDown {
                    key: Key::ArrowUp,
                    repeat: false
                },
                Event::PointerMoved { x: 20.0, y: 30.0 },
                Event::MouseButton {
                    button: MouseButton::Left,
                    pressed: true
                },
                Event::MouseButton {
                    button: MouseButton::Left,
                    pressed: false
                },
                Event::Wheel { x: 0.0, y: 1.0 },
                Event::CloseRequested,
                // TranslateMessage queues the character behind what was already posted.
                Event::Text('w'),
            ]
        );
    }

    #[test]
    fn layouts_match_the_windows_headers() {
        use std::mem::size_of;
        assert_eq!(size_of::<WndClassExW>(), 80);
        assert_eq!(size_of::<Msg>(), 48);
        assert_eq!(size_of::<TrackMouseEvent>(), 24);
    }
}
