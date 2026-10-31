//! Cocoa window backend, through the Objective-C runtime (`libobjc`) and AppKit loaded at run
//! time.
//!
//! Messages are sent with `objc_msgSend` cast to each method's signature. The window's content
//! view is backed by a `CAMetalLayer`, which is what both Metal and Vulkan (through MoltenVK)
//! present to. [`Window::poll_events`] drains the application's event queue, translating events
//! for this window before handing them on to AppKit, and reports size and focus changes by
//! comparing against the last values it saw.

use crate::{Event, Key, Library, MouseButton, PlatformError, RawWindowHandle, WindowConfig};
use std::collections::HashSet;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::OnceLock;

type Id = *mut c_void;
type Sel = *mut c_void;
type Class = *mut c_void;
type Imp = *const c_void;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Point {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Size {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Rect {
    origin: Point,
    size: Size,
}

// NSWindowStyleMask.
const STYLE_TITLED: u64 = 1 << 0;
const STYLE_CLOSABLE: u64 = 1 << 1;
const STYLE_MINIATURIZABLE: u64 = 1 << 2;
const STYLE_RESIZABLE: u64 = 1 << 3;
const BACKING_BUFFERED: u64 = 2;
const ACTIVATION_POLICY_REGULAR: i64 = 0;

// NSEventType.
const LEFT_MOUSE_DOWN: u64 = 1;
const LEFT_MOUSE_UP: u64 = 2;
const RIGHT_MOUSE_DOWN: u64 = 3;
const RIGHT_MOUSE_UP: u64 = 4;
const MOUSE_MOVED: u64 = 5;
const LEFT_MOUSE_DRAGGED: u64 = 6;
const RIGHT_MOUSE_DRAGGED: u64 = 7;
const KEY_DOWN: u64 = 10;
const KEY_UP: u64 = 11;
const FLAGS_CHANGED: u64 = 12;
const SCROLL_WHEEL: u64 = 22;
const OTHER_MOUSE_DOWN: u64 = 25;
const OTHER_MOUSE_UP: u64 = 26;
const OTHER_MOUSE_DRAGGED: u64 = 27;
const EVENT_MASK_ANY: u64 = u64::MAX;

/// Lines per pixel of precise (trackpad) scrolling, to report the wheel in lines everywhere.
const PIXELS_PER_LINE: f64 = 10.0;

/// The Objective-C runtime entry points, loaded once per process with AppKit and QuartzCore.
struct Runtime {
    get_class: unsafe extern "C" fn(*const c_char) -> Class,
    register_name: unsafe extern "C" fn(*const c_char) -> Sel,
    allocate_class_pair: unsafe extern "C" fn(Class, *const c_char, usize) -> Class,
    register_class_pair: unsafe extern "C" fn(Class),
    add_method: unsafe extern "C" fn(Class, Sel, Imp, *const c_char) -> bool,
    add_ivar: unsafe extern "C" fn(Class, *const c_char, usize, u8, *const c_char) -> bool,
    set_ivar: unsafe extern "C" fn(Id, *const c_char, *mut c_void) -> Id,
    get_ivar: unsafe extern "C" fn(Id, *const c_char, *mut *mut c_void) -> Id,
    pool_push: unsafe extern "C" fn() -> *mut c_void,
    pool_pop: unsafe extern "C" fn(*mut c_void),
    msg_send: Imp,
    #[cfg(target_arch = "x86_64")]
    msg_send_stret: Imp,
    /// `NSDefaultRunLoopMode`, an `NSString *` exported by Foundation.
    default_mode: Id,
}

// The function pointers and the constant string are process-wide and immutable.
unsafe impl Send for Runtime {}
unsafe impl Sync for Runtime {}

impl Runtime {
    fn load() -> Result<Runtime, PlatformError> {
        let objc = Library::open(&["/usr/lib/libobjc.A.dylib"])?;
        let appkit = Library::open(&["/System/Library/Frameworks/AppKit.framework/AppKit"])?;
        let quartz =
            Library::open(&["/System/Library/Frameworks/QuartzCore.framework/QuartzCore"])?;
        let foundation =
            Library::open(&["/System/Library/Frameworks/Foundation.framework/Foundation"])?;
        unsafe {
            let mode = foundation.symbol_ptr("NSDefaultRunLoopMode") as *const Id;
            if mode.is_null() {
                return Err(PlatformError::new("missing symbol NSDefaultRunLoopMode"));
            }
            let runtime = Runtime {
                get_class: objc.require("objc_getClass")?,
                register_name: objc.require("sel_registerName")?,
                allocate_class_pair: objc.require("objc_allocateClassPair")?,
                register_class_pair: objc.require("objc_registerClassPair")?,
                add_method: objc.require("class_addMethod")?,
                add_ivar: objc.require("class_addIvar")?,
                set_ivar: objc.require("object_setInstanceVariable")?,
                get_ivar: objc.require("object_getInstanceVariable")?,
                pool_push: objc.require("objc_autoreleasePoolPush")?,
                pool_pop: objc.require("objc_autoreleasePoolPop")?,
                msg_send: objc.symbol_ptr("objc_msgSend"),
                #[cfg(target_arch = "x86_64")]
                msg_send_stret: objc.symbol_ptr("objc_msgSend_stret"),
                default_mode: *mode,
            };
            // The frameworks register their classes when loaded and stay loaded with the
            // process, as in any Cocoa program.
            std::mem::forget((objc, appkit, quartz, foundation));
            Ok(runtime)
        }
    }

    fn class(&self, name: &CStr) -> Class {
        unsafe { (self.get_class)(name.as_ptr()) }
    }

    fn sel(&self, name: &CStr) -> Sel {
        unsafe { (self.register_name)(name.as_ptr()) }
    }

    /// `[receiver selector]`.
    unsafe fn send<R>(&self, receiver: Id, selector: &CStr) -> R {
        let f: unsafe extern "C" fn(Id, Sel) -> R = std::mem::transmute(self.msg_send);
        f(receiver, self.sel(selector))
    }

    /// `[receiver selector: a]`.
    unsafe fn send1<R, A>(&self, receiver: Id, selector: &CStr, a: A) -> R {
        let f: unsafe extern "C" fn(Id, Sel, A) -> R = std::mem::transmute(self.msg_send);
        f(receiver, self.sel(selector), a)
    }

    unsafe fn send2<R, A, B>(&self, receiver: Id, selector: &CStr, a: A, b: B) -> R {
        let f: unsafe extern "C" fn(Id, Sel, A, B) -> R = std::mem::transmute(self.msg_send);
        f(receiver, self.sel(selector), a, b)
    }

    unsafe fn send4<R, A, B, C, D>(
        &self,
        receiver: Id,
        selector: &CStr,
        a: A,
        b: B,
        c: C,
        d: D,
    ) -> R {
        let f: unsafe extern "C" fn(Id, Sel, A, B, C, D) -> R = std::mem::transmute(self.msg_send);
        f(receiver, self.sel(selector), a, b, c, d)
    }

    /// A message returning an `NSRect`, which x86-64 returns through memory.
    unsafe fn send_rect(&self, receiver: Id, selector: &CStr) -> Rect {
        #[cfg(target_arch = "x86_64")]
        {
            let f: unsafe extern "C" fn(*mut Rect, Id, Sel) =
                std::mem::transmute(self.msg_send_stret);
            let mut rect = Rect::default();
            f(&mut rect, receiver, self.sel(selector));
            rect
        }
        #[cfg(not(target_arch = "x86_64"))]
        self.send(receiver, selector)
    }

    unsafe fn string(&self, text: &str) -> Id {
        let text = CString::new(text.replace('\0', "")).unwrap();
        self.send1(
            self.class(c"NSString"),
            c"stringWithUTF8String:",
            text.as_ptr(),
        )
    }
}

static RUNTIME: OnceLock<Result<Runtime, PlatformError>> = OnceLock::new();

fn runtime() -> Result<&'static Runtime, PlatformError> {
    RUNTIME
        .get_or_init(Runtime::load)
        .as_ref()
        .map_err(Clone::clone)
}

const CLOSE_FLAG: &CStr = c"shallotCloseRequested";

/// `windowShouldClose:`: records the request and leaves closing to the application.
unsafe extern "C" fn window_should_close(this: Id, _sel: Sel, _sender: Id) -> bool {
    if let Ok(rt) = runtime() {
        (rt.set_ivar)(
            this,
            CLOSE_FLAG.as_ptr(),
            std::ptr::dangling_mut::<c_void>(),
        );
    }
    false
}

/// The window delegate class, registered once.
fn delegate_class(rt: &Runtime) -> Class {
    static CLASS: OnceLock<usize> = OnceLock::new();
    *CLASS.get_or_init(|| unsafe {
        let class =
            (rt.allocate_class_pair)(rt.class(c"NSObject"), c"ShallotWindowDelegate".as_ptr(), 0);
        let size = std::mem::size_of::<*mut c_void>();
        (rt.add_ivar)(
            class,
            CLOSE_FLAG.as_ptr(),
            size,
            size.trailing_zeros() as u8,
            c"^v".as_ptr(),
        );
        (rt.add_method)(
            class,
            rt.sel(c"windowShouldClose:"),
            window_should_close as Imp,
            c"c@:@".as_ptr(),
        );
        (rt.register_class_pair)(class);
        class as usize
    }) as Class
}

pub struct Window {
    rt: &'static Runtime,
    app: Id,
    window: Id,
    view: Id,
    layer: Id,
    delegate: Id,
    width: u32,
    height: u32,
    focused: bool,
    pointer_inside: bool,
    held: HashSet<u16>,
}

impl Window {
    pub fn new(config: &WindowConfig) -> Result<Window, PlatformError> {
        let rt = runtime()?;
        unsafe {
            let pool = (rt.pool_push)();
            let result = Self::create(rt, config);
            (rt.pool_pop)(pool);
            result
        }
    }

    unsafe fn create(rt: &'static Runtime, config: &WindowConfig) -> Result<Window, PlatformError> {
        let main: bool = rt.send(rt.class(c"NSThread"), c"isMainThread");
        if !main {
            return Err(PlatformError::new(
                "AppKit windows must be created on the main thread",
            ));
        }
        let app: Id = rt.send(rt.class(c"NSApplication"), c"sharedApplication");
        if app.is_null() {
            return Err(PlatformError::new("could not create the NSApplication"));
        }
        let _: bool = rt.send1(app, c"setActivationPolicy:", ACTIVATION_POLICY_REGULAR);
        let _: () = rt.send(app, c"finishLaunching");

        // The configured size is in pixels; AppKit sizes windows in points.
        let screen: Id = rt.send(rt.class(c"NSScreen"), c"mainScreen");
        let scale: f64 = if screen.is_null() {
            1.0
        } else {
            rt.send(screen, c"backingScaleFactor")
        };
        let width = config.width.max(1);
        let height = config.height.max(1);
        let content = Rect {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size {
                width: width as f64 / scale,
                height: height as f64 / scale,
            },
        };
        let mut style = STYLE_TITLED | STYLE_CLOSABLE | STYLE_MINIATURIZABLE;
        if config.resizable {
            style |= STYLE_RESIZABLE;
        }
        let window: Id = rt.send(rt.class(c"NSWindow"), c"alloc");
        let window: Id = rt.send4(
            window,
            c"initWithContentRect:styleMask:backing:defer:",
            content,
            style,
            BACKING_BUFFERED,
            false,
        );
        if window.is_null() {
            return Err(PlatformError::new("could not create the NSWindow"));
        }
        // The window is released by `Drop`, not when it closes.
        let _: () = rt.send1(window, c"setReleasedWhenClosed:", false);
        let _: () = rt.send1(window, c"setAcceptsMouseMovedEvents:", true);

        let delegate: Id = rt.send(delegate_class(rt), c"new");
        let _: () = rt.send1(window, c"setDelegate:", delegate);

        let view: Id = rt.send(window, c"contentView");
        let layer: Id = rt.send(rt.class(c"CAMetalLayer"), c"layer");
        if layer.is_null() {
            return Err(PlatformError::new("could not create a CAMetalLayer"));
        }
        let _: Id = rt.send(layer, c"retain");
        let _: () = rt.send1(layer, c"setContentsScale:", scale);
        let _: () = rt.send1(view, c"setWantsLayer:", true);
        let _: () = rt.send1(view, c"setLayer:", layer);

        let _: () = rt.send(window, c"center");
        let mut this = Window {
            rt,
            app,
            window,
            view,
            layer,
            delegate,
            width,
            height,
            focused: false,
            pointer_inside: false,
            held: HashSet::new(),
        };
        this.set_title(&config.title);
        let _: () = rt.send1(
            window,
            c"makeKeyAndOrderFront:",
            std::ptr::null_mut::<c_void>(),
        );
        let _: () = rt.send1(app, c"activateIgnoringOtherApps:", true);
        let (w, h) = this.pixel_size();
        this.width = w;
        this.height = h;
        Ok(this)
    }

    unsafe fn scale(&self) -> f64 {
        self.rt.send(self.window, c"backingScaleFactor")
    }

    /// The content view's size in pixels.
    unsafe fn pixel_size(&self) -> (u32, u32) {
        let bounds = self.rt.send_rect(self.view, c"bounds");
        let scale = self.scale();
        (
            ((bounds.size.width * scale).round() as u32).max(1),
            ((bounds.size.height * scale).round() as u32).max(1),
        )
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn raw_handle(&self) -> RawWindowHandle {
        RawWindowHandle::AppKit {
            ns_view: self.view,
            ca_metal_layer: self.layer,
        }
    }

    pub fn set_title(&mut self, title: &str) {
        unsafe {
            let pool = (self.rt.pool_push)();
            let title = self.rt.string(title);
            let _: () = self.rt.send1(self.window, c"setTitle:", title);
            (self.rt.pool_pop)(pool);
        }
    }

    pub fn poll_events(&mut self, events: &mut Vec<Event>) {
        let rt = self.rt;
        unsafe {
            let pool = (rt.pool_push)();
            let distant_past: Id = rt.send(rt.class(c"NSDate"), c"distantPast");
            loop {
                let event: Id = rt.send4(
                    self.app,
                    c"nextEventMatchingMask:untilDate:inMode:dequeue:",
                    EVENT_MASK_ANY,
                    distant_past,
                    rt.default_mode,
                    true,
                );
                if event.is_null() {
                    break;
                }
                let target: Id = rt.send(event, c"window");
                let forward = if target == self.window {
                    self.translate(event, events)
                } else {
                    true
                };
                if forward {
                    let _: () = rt.send1(self.app, c"sendEvent:", event);
                }
            }
            let _: () = rt.send(self.app, c"updateWindows");

            let mut flag: *mut c_void = std::ptr::null_mut();
            (rt.get_ivar)(self.delegate, CLOSE_FLAG.as_ptr(), &mut flag);
            if !flag.is_null() {
                (rt.set_ivar)(self.delegate, CLOSE_FLAG.as_ptr(), std::ptr::null_mut());
                events.push(Event::CloseRequested);
            }
            let (width, height) = self.pixel_size();
            if (width, height) != (self.width, self.height) {
                self.width = width;
                self.height = height;
                let _: () = rt.send1(self.layer, c"setContentsScale:", self.scale());
                events.push(Event::Resized { width, height });
            }
            let focused: bool = rt.send(self.window, c"isKeyWindow");
            if focused != self.focused {
                self.focused = focused;
                if !focused {
                    self.held.clear();
                }
                events.push(Event::Focused(focused));
            }
            (rt.pool_pop)(pool);
        }
    }

    /// Translates an event for this window. Returns whether AppKit should also see it: key
    /// events stay here, since no responder handles them and AppKit would beep.
    unsafe fn translate(&mut self, event: Id, events: &mut Vec<Event>) -> bool {
        let rt = self.rt;
        let kind: u64 = rt.send(event, c"type");
        match kind {
            KEY_DOWN | KEY_UP | FLAGS_CHANGED => {
                let code: u16 = rt.send(event, c"keyCode");
                let repeat = kind == KEY_DOWN && rt.send::<bool>(event, c"isARepeat");
                let mut text = None;
                if kind == KEY_DOWN {
                    let characters: Id = rt.send(event, c"characters");
                    if !characters.is_null() {
                        let utf8: *const c_char = rt.send(characters, c"UTF8String");
                        if !utf8.is_null() {
                            text = Some(CStr::from_ptr(utf8).to_string_lossy());
                        }
                    }
                }
                key_event(&mut self.held, kind, code, repeat, text.as_deref(), events);
                false
            }
            MOUSE_MOVED | LEFT_MOUSE_DRAGGED | RIGHT_MOUSE_DRAGGED | OTHER_MOUSE_DRAGGED => {
                let (x, y) = self.pointer(event);
                let inside =
                    x >= 0.0 && y >= 0.0 && x < self.width as f64 && y < self.height as f64;
                if inside != self.pointer_inside {
                    self.pointer_inside = inside;
                    events.push(Event::PointerInside(inside));
                }
                events.push(Event::PointerMoved { x, y });
                true
            }
            LEFT_MOUSE_DOWN | LEFT_MOUSE_UP | RIGHT_MOUSE_DOWN | RIGHT_MOUSE_UP
            | OTHER_MOUSE_DOWN | OTHER_MOUSE_UP => {
                let button = match kind {
                    LEFT_MOUSE_DOWN | LEFT_MOUSE_UP => MouseButton::Left,
                    RIGHT_MOUSE_DOWN | RIGHT_MOUSE_UP => MouseButton::Right,
                    _ => {
                        let number: i64 = rt.send(event, c"buttonNumber");
                        match number {
                            2 => MouseButton::Middle,
                            3 => MouseButton::Back,
                            4 => MouseButton::Forward,
                            n => MouseButton::Other(n.clamp(0, u16::MAX as i64) as u16),
                        }
                    }
                };
                let pressed = matches!(kind, LEFT_MOUSE_DOWN | RIGHT_MOUSE_DOWN | OTHER_MOUSE_DOWN);
                events.push(Event::MouseButton { button, pressed });
                true
            }
            SCROLL_WHEEL => {
                let dx: f64 = rt.send(event, c"scrollingDeltaX");
                let dy: f64 = rt.send(event, c"scrollingDeltaY");
                let precise: bool = rt.send(event, c"hasPreciseScrollingDeltas");
                let lines = if precise { 1.0 / PIXELS_PER_LINE } else { 1.0 };
                // AppKit's deltas are positive towards the top and left: the DOM's opposite.
                events.push(Event::Wheel {
                    x: -dx * lines,
                    y: -dy * lines,
                });
                true
            }
            _ => true,
        }
    }

    /// The event's position in pixels from the content view's top-left corner.
    unsafe fn pointer(&self, event: Id) -> (f64, f64) {
        let rt = self.rt;
        let location: Point = rt.send(event, c"locationInWindow");
        let local: Point = rt.send2(
            self.view,
            c"convertPoint:fromView:",
            location,
            std::ptr::null_mut::<c_void>(),
        );
        let bounds = rt.send_rect(self.view, c"bounds");
        let scale = self.scale();
        (local.x * scale, (bounds.size.height - local.y) * scale)
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let rt = self.rt;
        unsafe {
            let _: () = rt.send1(self.window, c"setDelegate:", std::ptr::null_mut::<c_void>());
            let _: () = rt.send(self.window, c"close");
            let _: () = rt.send(self.window, c"release");
            let _: () = rt.send(self.delegate, c"release");
            let _: () = rt.send(self.layer, c"release");
        }
    }
}

/// Translates a key event's fields. `held` tracks the keys down, which is how a modifier's
/// `FlagsChanged` tells a press from a release.
fn key_event(
    held: &mut HashSet<u16>,
    kind: u64,
    code: u16,
    repeat: bool,
    text: Option<&str>,
    events: &mut Vec<Event>,
) {
    let key = key_from_virtual_code(code);
    match kind {
        KEY_DOWN => {
            held.insert(code);
            if let Some(key) = key {
                events.push(Event::KeyDown { key, repeat });
            }
            for ch in text.unwrap_or_default().chars() {
                // Function and arrow keys arrive as private-use characters.
                if !ch.is_control() && !('\u{F700}'..='\u{F8FF}').contains(&ch) {
                    events.push(Event::Text(ch));
                }
            }
        }
        KEY_UP => {
            held.remove(&code);
            if let Some(key) = key {
                events.push(Event::KeyUp { key });
            }
        }
        FLAGS_CHANGED => {
            let Some(key) = key else { return };
            if held.remove(&code) {
                events.push(Event::KeyUp { key });
            } else {
                held.insert(code);
                events.push(Event::KeyDown { key, repeat: false });
            }
        }
        _ => {}
    }
}

/// Maps a macOS virtual key code (`kVK_*` in `Events.h`, which name positions on an ANSI
/// keyboard) to a key. Keys a Mac keyboard lacks (Print Screen, Scroll Lock, Pause) never
/// arrive.
pub(crate) fn key_from_virtual_code(code: u16) -> Option<Key> {
    use Key::*;
    Some(match code {
        0x00 => KeyA,
        0x01 => KeyS,
        0x02 => KeyD,
        0x03 => KeyF,
        0x04 => KeyH,
        0x05 => KeyG,
        0x06 => KeyZ,
        0x07 => KeyX,
        0x08 => KeyC,
        0x09 => KeyV,
        0x0A => IntlBackslash,
        0x0B => KeyB,
        0x0C => KeyQ,
        0x0D => KeyW,
        0x0E => KeyE,
        0x0F => KeyR,
        0x10 => KeyY,
        0x11 => KeyT,
        0x12 => Digit1,
        0x13 => Digit2,
        0x14 => Digit3,
        0x15 => Digit4,
        0x16 => Digit6,
        0x17 => Digit5,
        0x18 => Equal,
        0x19 => Digit9,
        0x1A => Digit7,
        0x1B => Minus,
        0x1C => Digit8,
        0x1D => Digit0,
        0x1E => BracketRight,
        0x1F => KeyO,
        0x20 => KeyU,
        0x21 => BracketLeft,
        0x22 => KeyI,
        0x23 => KeyP,
        0x24 => Enter,
        0x25 => KeyL,
        0x26 => KeyJ,
        0x27 => Quote,
        0x28 => KeyK,
        0x29 => Semicolon,
        0x2A => Backslash,
        0x2B => Comma,
        0x2C => Slash,
        0x2D => KeyN,
        0x2E => KeyM,
        0x2F => Period,
        0x30 => Tab,
        0x31 => Space,
        0x32 => Backquote,
        0x33 => Backspace,
        0x35 => Escape,
        0x36 => MetaRight,
        0x37 => MetaLeft,
        0x38 => ShiftLeft,
        0x39 => CapsLock,
        0x3A => AltLeft,
        0x3B => ControlLeft,
        0x3C => ShiftRight,
        0x3D => AltRight,
        0x3E => ControlRight,
        0x41 => NumpadDecimal,
        0x43 => NumpadMultiply,
        0x45 => NumpadAdd,
        // The keypad's Clear key sits where NumLock is on a PC keypad.
        0x47 => NumLock,
        0x4B => NumpadDivide,
        0x4C => NumpadEnter,
        0x4E => NumpadSubtract,
        0x52 => Numpad0,
        0x53 => Numpad1,
        0x54 => Numpad2,
        0x55 => Numpad3,
        0x56 => Numpad4,
        0x57 => Numpad5,
        0x58 => Numpad6,
        0x59 => Numpad7,
        0x5B => Numpad8,
        0x5C => Numpad9,
        0x60 => F5,
        0x61 => F6,
        0x62 => F7,
        0x63 => F3,
        0x64 => F8,
        0x65 => F9,
        0x67 => F11,
        0x6D => F10,
        0x6E => ContextMenu,
        0x6F => F12,
        // Help sits where Insert is on a PC keyboard.
        0x72 => Insert,
        0x73 => Home,
        0x74 => PageUp,
        0x75 => Delete,
        0x76 => F4,
        0x77 => End,
        0x78 => F2,
        0x79 => PageDown,
        0x7A => F1,
        0x7B => ArrowLeft,
        0x7C => ArrowRight,
        0x7D => ArrowDown,
        0x7E => ArrowUp,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_codes_map_to_dom_codes() {
        assert_eq!(key_from_virtual_code(0x0D), Some(Key::KeyW));
        assert_eq!(key_from_virtual_code(0x31), Some(Key::Space));
        assert_eq!(key_from_virtual_code(0x7E), Some(Key::ArrowUp));
        assert_eq!(key_from_virtual_code(0x37), Some(Key::MetaLeft));
        assert_eq!(key_from_virtual_code(0x34), None);
    }

    #[test]
    fn every_key_a_mac_has_is_mapped_once() {
        let mut seen = HashSet::new();
        for code in 0..=0x7F {
            if let Some(key) = key_from_virtual_code(code) {
                assert!(seen.insert(key), "{key:?} mapped twice");
            }
        }
        let absent = [Key::PrintScreen, Key::ScrollLock, Key::Pause];
        for key in Key::ALL {
            assert_eq!(seen.contains(key), !absent.contains(key), "{key:?}");
        }
    }

    #[test]
    fn key_events_translate() {
        let mut held = HashSet::new();
        let mut events = Vec::new();
        key_event(&mut held, KEY_DOWN, 0x0D, false, Some("w"), &mut events);
        key_event(&mut held, KEY_DOWN, 0x0D, true, Some("w"), &mut events);
        key_event(&mut held, KEY_UP, 0x0D, false, None, &mut events);
        // The up arrow's text is a private-use character, and is not text.
        key_event(
            &mut held,
            KEY_DOWN,
            0x7E,
            false,
            Some("\u{F700}"),
            &mut events,
        );
        assert_eq!(
            events,
            [
                Event::KeyDown {
                    key: Key::KeyW,
                    repeat: false
                },
                Event::Text('w'),
                Event::KeyDown {
                    key: Key::KeyW,
                    repeat: true
                },
                Event::Text('w'),
                Event::KeyUp { key: Key::KeyW },
                Event::KeyDown {
                    key: Key::ArrowUp,
                    repeat: false
                },
            ]
        );
    }

    #[test]
    fn modifier_changes_toggle() {
        let mut held = HashSet::new();
        let mut events = Vec::new();
        for _ in 0..2 {
            key_event(&mut held, FLAGS_CHANGED, 0x38, false, None, &mut events);
        }
        assert_eq!(
            events,
            [
                Event::KeyDown {
                    key: Key::ShiftLeft,
                    repeat: false
                },
                Event::KeyUp {
                    key: Key::ShiftLeft
                },
            ]
        );
        assert!(held.is_empty());
    }
}
