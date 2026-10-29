//! Window and input events.

/// Something that happened to a window.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The user asked to close the window, for example with its close button.
    CloseRequested,
    /// The drawable area changed size, in physical pixels.
    Resized { width: u32, height: u32 },
    /// The window gained (`true`) or lost (`false`) keyboard focus.
    Focused(bool),
    /// A physical key went down. `repeat` is true for auto-repeats while it is held.
    KeyDown { key: Key, repeat: bool },
    /// A physical key came up.
    KeyUp { key: Key },
    /// Text produced by typing, after the keyboard layout is applied.
    Text(char),
    /// The pointer moved, in physical pixels from the window's top-left corner.
    PointerMoved { x: f64, y: f64 },
    /// The pointer entered (`true`) or left (`false`) the window.
    PointerInside(bool),
    /// A mouse button changed state.
    MouseButton { button: MouseButton, pressed: bool },
    /// The wheel scrolled, in lines. Positive `y` scrolls down, as for DOM `WheelEvent.deltaY`.
    Wheel { x: f64, y: f64 },
}

/// A mouse button, numbered as DOM `MouseEvent.button`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Back,
    Forward,
    Other(u16),
}

impl MouseButton {
    /// The DOM `MouseEvent.button` number.
    pub fn index(self) -> u16 {
        match self {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::Back => 3,
            MouseButton::Forward => 4,
            MouseButton::Other(n) => n,
        }
    }
}

macro_rules! keys {
    ($($name:ident),* $(,)?) => {
        /// A physical key, named as DOM `KeyboardEvent.code`.
        ///
        /// Keys name positions on a US layout regardless of the active layout, which is what the
        /// engine's input resource keys on.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum Key {
            $($name,)*
        }

        impl Key {
            /// Every key, in declaration order.
            pub const ALL: &'static [Key] = &[$(Key::$name,)*];

            /// The DOM `KeyboardEvent.code` string, such as `"KeyW"` or `"ArrowLeft"`.
            pub fn code(self) -> &'static str {
                match self {
                    $(Key::$name => stringify!($name),)*
                }
            }

            /// Parses a DOM `KeyboardEvent.code` string.
            pub fn from_code(code: &str) -> Option<Key> {
                match code {
                    $(stringify!($name) => Some(Key::$name),)*
                    _ => None,
                }
            }
        }
    };
}

keys! {
    Escape, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9, Digit0,
    Minus, Equal, Backspace, Tab,
    KeyQ, KeyW, KeyE, KeyR, KeyT, KeyY, KeyU, KeyI, KeyO, KeyP, BracketLeft, BracketRight, Enter,
    ControlLeft, KeyA, KeyS, KeyD, KeyF, KeyG, KeyH, KeyJ, KeyK, KeyL, Semicolon, Quote, Backquote,
    ShiftLeft, Backslash, KeyZ, KeyX, KeyC, KeyV, KeyB, KeyN, KeyM, Comma, Period, Slash,
    ShiftRight, NumpadMultiply, AltLeft, Space, CapsLock,
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,
    NumLock, ScrollLock, Numpad7, Numpad8, Numpad9, NumpadSubtract, Numpad4, Numpad5, Numpad6,
    NumpadAdd, Numpad1, Numpad2, Numpad3, Numpad0, NumpadDecimal, IntlBackslash, NumpadEnter,
    ControlRight, NumpadDivide, PrintScreen, AltRight, Home, ArrowUp, PageUp, ArrowLeft,
    ArrowRight, End, ArrowDown, PageDown, Insert, Delete, Pause, MetaLeft, MetaRight,
    ContextMenu,
}

impl Key {
    /// Maps a Linux evdev scancode (`KEY_*` in `input-event-codes.h`) to a key.
    ///
    /// X11 and Wayland keycodes are evdev codes offset by 8. Codes 1 to 88 also match PC
    /// scan-code set 1, which Win32 reports.
    pub fn from_evdev(code: u32) -> Option<Key> {
        use Key::*;
        Some(match code {
            1 => Escape,
            2 => Digit1,
            3 => Digit2,
            4 => Digit3,
            5 => Digit4,
            6 => Digit5,
            7 => Digit6,
            8 => Digit7,
            9 => Digit8,
            10 => Digit9,
            11 => Digit0,
            12 => Minus,
            13 => Equal,
            14 => Backspace,
            15 => Tab,
            16 => KeyQ,
            17 => KeyW,
            18 => KeyE,
            19 => KeyR,
            20 => KeyT,
            21 => KeyY,
            22 => KeyU,
            23 => KeyI,
            24 => KeyO,
            25 => KeyP,
            26 => BracketLeft,
            27 => BracketRight,
            28 => Enter,
            29 => ControlLeft,
            30 => KeyA,
            31 => KeyS,
            32 => KeyD,
            33 => KeyF,
            34 => KeyG,
            35 => KeyH,
            36 => KeyJ,
            37 => KeyK,
            38 => KeyL,
            39 => Semicolon,
            40 => Quote,
            41 => Backquote,
            42 => ShiftLeft,
            43 => Backslash,
            44 => KeyZ,
            45 => KeyX,
            46 => KeyC,
            47 => KeyV,
            48 => KeyB,
            49 => KeyN,
            50 => KeyM,
            51 => Comma,
            52 => Period,
            53 => Slash,
            54 => ShiftRight,
            55 => NumpadMultiply,
            56 => AltLeft,
            57 => Space,
            58 => CapsLock,
            59 => F1,
            60 => F2,
            61 => F3,
            62 => F4,
            63 => F5,
            64 => F6,
            65 => F7,
            66 => F8,
            67 => F9,
            68 => F10,
            69 => NumLock,
            70 => ScrollLock,
            71 => Numpad7,
            72 => Numpad8,
            73 => Numpad9,
            74 => NumpadSubtract,
            75 => Numpad4,
            76 => Numpad5,
            77 => Numpad6,
            78 => NumpadAdd,
            79 => Numpad1,
            80 => Numpad2,
            81 => Numpad3,
            82 => Numpad0,
            83 => NumpadDecimal,
            86 => IntlBackslash,
            87 => F11,
            88 => F12,
            96 => NumpadEnter,
            97 => ControlRight,
            98 => NumpadDivide,
            99 => PrintScreen,
            100 => AltRight,
            102 => Home,
            103 => ArrowUp,
            104 => PageUp,
            105 => ArrowLeft,
            106 => ArrowRight,
            107 => End,
            108 => ArrowDown,
            109 => PageDown,
            110 => Insert,
            111 => Delete,
            119 => Pause,
            125 => MetaLeft,
            126 => MetaRight,
            127 => ContextMenu,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for &key in Key::ALL {
            assert_eq!(Key::from_code(key.code()), Some(key));
        }
        assert_eq!(Key::KeyW.code(), "KeyW");
        assert_eq!(Key::from_code("Nope"), None);
    }

    #[test]
    fn evdev_maps_common_keys() {
        assert_eq!(Key::from_evdev(17), Some(Key::KeyW));
        assert_eq!(Key::from_evdev(57), Some(Key::Space));
        assert_eq!(Key::from_evdev(103), Some(Key::ArrowUp));
        assert_eq!(Key::from_evdev(0), None);
    }

    #[test]
    fn evdev_table_is_injective() {
        let mut seen = std::collections::HashSet::new();
        for code in 0..256 {
            if let Some(key) = Key::from_evdev(code) {
                assert!(seen.insert(key), "{key:?} mapped twice");
            }
        }
        assert_eq!(seen.len(), Key::ALL.len());
    }
}
