# Platform layer (`shallot-platform`)

The platform crate replaces everything the browser gives the TypeScript engine for free: a canvas and its window, DOM input events, `requestAnimationFrame`, `performance.now`, Web Workers, `fetch` for local assets, and Web Audio output. It exposes one safe API with an implementation per OS.

## Scope

| Capability | TypeScript source of truth | Rust API |
|---|---|---|
| Window and surface | `<canvas>`, `devicePixelRatio`, resize observer | `Window`, `WindowConfig`, `RawSurface` handed to `shallot-gpu` |
| Event loop and frame clock | `requestAnimationFrame` timestamps | `EventLoop::run(app)`, `FrameEvent { present_time }` |
| Keyboard | `KeyboardEvent.code` | `Event::Key { code: KeyCode, pressed, repeat }`, `KeyCode` named after DOM codes |
| Text input | not used by examples today | `Event::Text(char)`; reserved for UI text fields |
| Pointer | `PointerEvent`, wheel | `Event::Pointer { id, kind: Mouse/Touch/Pen, position, buttons }`, `Event::Wheel` |
| Pointer lock | `requestPointerLock`, `movementX/Y` | `Window::set_pointer_lock(bool) -> Result`, raw relative motion events |
| Touch | `PointerEvent` with `pointerType = "touch"` | Same `Pointer` event with `kind: Touch` |
| Focus and visibility | `focus`, `blur`, `visibilitychange` | `Event::Focus(bool)`, `Event::Occluded(bool)` |
| Cursor | CSS `cursor` | `Window::set_cursor(CursorIcon)` |
| Time | `performance.now` | `Instant` from `std`; present timestamps from the OS where available |
| Threads | Web Workers, `SharedArrayBuffer`, `Atomics.wait` | `std::thread`, `std::sync::atomic`, a `Parker` over `std::thread::park` |
| Files | `fetch` of bundled assets | `assets::read(path) -> Result<Vec<u8>>` relative to the executable's asset root; bundled assets via `include_bytes!` |
| Audio output | `AudioContext` + `AudioWorklet` | `AudioDevice::open(config, callback)`; callback renders 128-frame blocks |
| Dynamic loading | — | `Library::open(name)`, `Library::symbol::<F>(name)` |
| Logging | `console` | `log` module writing to stderr and `OutputDebugStringW` on Windows |

Gamepads are not used by any example or by `core/input` today, so they are out of scope until upstream adds them.

## Event loop shape

```rust
pub trait Handler {
    fn event(&mut self, window: &Window, event: Event);
    /// Called once per display refresh with the predicted present time, the native analogue
    /// of a requestAnimationFrame callback. Returning Control::Exit ends the loop.
    fn frame(&mut self, window: &Window, frame: FrameEvent) -> Control;
}

pub fn run(config: WindowConfig, handler: impl Handler) -> Result<(), PlatformError>;
```

- Events are delivered between frames, then `frame` runs once. The engine samples input once per callback into tick-addressed records exactly as `core/input` does today.
- `frame` is paced by the swapchain, not by a timer: `shallot-gpu` blocks in `acquire_next_image` or the Metal drawable, which is the native equivalent of the browser's rAF pacing. A frame-latency limit of two replaces the "skip while six frames are pending" rule.
- One window per process is required for the examples. The API allows several, for tools and the embedded-scene example ([examples.md](examples.md#wave-1-engine-bring-up)).
- Headless mode (`WindowConfig::headless(width, height)`) creates no OS window and drives `frame` from a fixed clock, for tests and screenshot capture.

## Per-OS implementation

### Windows

- Win32 through `user32`/`kernel32`/`gdi32`: `RegisterClassExW`, `CreateWindowExW`, a message pump with `PeekMessageW`.
- Per-monitor DPI v2 (`SetProcessDpiAwarenessContext`), `WM_DPICHANGED` drives the device-pixel ratio.
- Keyboard from `WM_KEYDOWN`/`WM_KEYUP` scan codes mapped to DOM codes (the scan-code table is fixed and documented in the UI Events spec).
- Pointer from `WM_POINTER*` for mouse, touch and pen. Raw relative motion for pointer lock from Raw Input (`RegisterRawInputDevices`, `WM_INPUT`), cursor confined with `ClipCursor`.
- Timing: `QueryPerformanceCounter` underlies `std::time::Instant`; `timeBeginPeriod(1)` only while running.
- Audio: WASAPI shared mode in event-driven mode through COM vtables declared by hand (`IMMDeviceEnumerator`, `IAudioClient`, `IAudioRenderClient`).
- Surface: `HWND` + `HINSTANCE` for `VK_KHR_win32_surface`.

Status: window, keyboard (scan codes, text with surrogate pairs), mouse buttons with capture, wheel, enter/leave, focus, resize, DPI awareness and the Vulkan surface are in `crates/platform/src/win32.rs`, with `user32.dll` loaded at run time. Mouse input still comes from the classic `WM_*BUTTON*` messages; `WM_POINTER*`, Raw Input, timing, audio and the device-pixel-ratio event follow with step 6 below. CI checks it twice: natively on `windows-latest` (lints and tests, including a real window fed posted messages) and as a cross-compiled build under Wine on Linux, where `winevulkan` forwards to lavapipe so the GPU tests and the windowed triangle run against the Win32 surface.

### macOS

- Cocoa through the Objective-C runtime only: `objc_getClass`, `sel_registerName`, `objc_msgSend` with typed casts, and `objc_allocateClassPair` to define the `NSApplicationDelegate`, `NSWindowDelegate` and `NSView` subclasses at run time. No Objective-C source.
- The view is layer-backed with a `CAMetalLayer`. Frame pacing from `CVDisplayLink` or, on macOS 14+, `CADisplayLink` from the view.
- Keyboard from `keyDown:`/`keyUp:`/`flagsChanged:` key codes mapped to DOM codes.
- Pointer lock: `CGAssociateMouseAndMouseCursorPosition(false)` plus hidden cursor; deltas from `deltaX`/`deltaY`.
- Touch: trackpad touches are not pointer touches on macOS, so `Touch` stays empty, as it does in desktop browsers.
- Audio: Core Audio `AudioUnit` (`kAudioUnitSubType_DefaultOutput`) render callback.
- Surface: `CAMetalLayer` for the Metal backend.

Status: window, keyboard (virtual key codes, modifiers from `FlagsChanged`, text), mouse buttons, wheel (precise trackpad deltas scaled to lines), enter/leave, focus, resize and the layer-backed view are in `crates/platform/src/cocoa.rs`, with `libobjc`, AppKit, Foundation and QuartzCore loaded at run time. Rather than subclassing `NSApplication` or `NSView`, it drains the event queue with `nextEventMatchingMask:untilDate:inMode:dequeue:` inside `poll_events`, translates the events for its window and forwards all but key events to `sendEvent:`; a runtime-defined delegate class answers `windowShouldClose:`, and size and focus are compared on each poll. Windows must be created on the main thread, so the window test is a `harness = false` test with its own `main`. Until the Metal backend lands (gpu.md F8), the GPU layer presents through MoltenVK: `VK_EXT_metal_surface` on the view's `CAMetalLayer`, with `VK_KHR_portability_enumeration` so the loader lists MoltenVK. CI runs lints, tests and the triangle on `macos-15` with Homebrew's `vulkan-loader` and `molten-vk`. Pointer lock, `CADisplayLink` pacing, the device-pixel-ratio event and audio follow with step 6 below.

### Linux

- X11 first (works everywhere, including XWayland), then native Wayland. Both loaded with `dlopen` so a binary runs on either.
- X11: `libX11` for windows and events, `XInput2` (`libXi`) for raw motion and touch, `XkbKeycodeToKeysym`-free mapping from evdev keycodes to DOM codes (evdev codes are `keycode - 8`), `libXcursor` for cursors, `_NET_WM_SYNC_REQUEST` for smooth resize.
- Wayland: `libwayland-client` with protocol interfaces transcribed by hand from `xdg-shell`, `relative-pointer-unstable-v1`, `pointer-constraints-unstable-v1`, `fractional-scale-v1` and `viewporter`; keyboard via `libxkbcommon` only for keymap-independent evdev codes. Client-side decorations: a minimal title bar drawn by the engine, or `xdg-decoration` where the compositor offers it.
- Audio: ALSA (`libasound`) as the baseline, PulseAudio's simple API when present. PipeWire serves both.
- Surface: `Display*` + `Window` for `VK_KHR_xlib_surface`, or `wl_display` + `wl_surface` for `VK_KHR_wayland_surface`.

## Bindings conventions

- One module per system library (`sys/user32.rs`, `sys/objc.rs`, `sys/x11.rs` …), each opening with the header name and version it was transcribed from.
- Structs are `#[repr(C)]` with layout assertions (`const _: () = assert!(size_of::<T>() == N)`) checked against the documented sizes for each target.
- Callbacks into Rust from the OS catch panics and abort with a message, since unwinding into foreign frames is undefined.
- Every `dlopen`ed library reports a clear error naming the missing library and the package that provides it.

## Delivery order

1. X11 window, events, timing and a Vulkan surface (the foundations milestone: a WGSL triangle).
2. Win32 with Vulkan.
3. Cocoa with Metal (needs the Metal backend in [gpu.md](gpu.md)).
4. Audio output on all three.
5. Wayland.
6. Pointer lock and touch on all three, then cursors, DPI changes and multiple windows.

## Tests

- Unit tests for key-code tables and DPI arithmetic run everywhere.
- Window tests run under `Xvfb` on Linux CI and natively on Windows and macOS runners; they create a window, pump events, inject synthetic input (`XTest`, `SendInput`, `CGEventPost`) and check the delivered events.
- Headless mode is what every engine-level test uses, so the platform is not on the critical path of the parity suite.
