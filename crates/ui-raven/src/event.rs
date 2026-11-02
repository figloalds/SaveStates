//! Input events of custom widgets.

use raven::*;

/// An input event, in coordinates local to the widget that receives it.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// `dx`, `dy`: movement since the previous mouse event.
    MouseMove {
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
    },
    /// `clicks`: 1 for a single click, 2 for a double click...
    MouseDown {
        x: f32,
        y: f32,
        button: MouseButton,
        clicks: i32,
    },
    MouseUp {
        x: f32,
        y: f32,
        button: MouseButton,
    },
    /// `wx`, `wy`: scroll amount in lines, positive = left / up.
    Wheel {
        x: f32,
        y: f32,
        wx: f32,
        wy: f32,
        ctrl: bool,
    },
    /// The pointer came over the widget.
    Enter,
    /// The pointer left the widget.
    Leave,
    KeyDown {
        key: Key,
        ctrl: bool,
        shift: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    /// Another button, by its Raven number.
    Other(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Escape,
    Enter,
    Tab,
    Space,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    /// A letter (upper case) or a digit.
    Char(char),
    /// F1 to F12.
    F(u8),
    /// Any other key (modifiers included), by its Raven code.
    Other(i32),
}

impl Event {
    /// The event a custom widget receives, or `None` for the kinds it is not told about
    /// (key up, typed text, keyboard focus).
    pub(crate) fn from_raw(e: &REvent) -> Option<Event> {
        let (x, y) = (e.local_x, e.local_y);
        let ctrl = e.modifiers & R_MOD_CTRL as u32 != 0;
        let shift = e.modifiers & R_MOD_SHIFT as u32 != 0;
        Some(match e.type_ {
            R_EVENT_MOUSE_MOVE => Event::MouseMove {
                x,
                y,
                dx: e.delta_x,
                dy: e.delta_y,
            },
            R_EVENT_MOUSE_DOWN => Event::MouseDown {
                x,
                y,
                button: MouseButton::from_raw(e.button),
                clicks: e.clicks,
            },
            R_EVENT_MOUSE_UP => Event::MouseUp {
                x,
                y,
                button: MouseButton::from_raw(e.button),
            },
            R_EVENT_MOUSE_WHEEL => Event::Wheel {
                x,
                y,
                wx: e.wheel_x,
                wy: e.wheel_y,
                ctrl,
            },
            R_EVENT_MOUSE_ENTER => Event::Enter,
            R_EVENT_MOUSE_LEAVE => Event::Leave,
            R_EVENT_KEY_DOWN => Event::KeyDown {
                key: Key::from_raw(e.key),
                ctrl,
                shift,
            },
            _ => return None,
        })
    }
}

impl MouseButton {
    fn from_raw(button: RMouseButton) -> MouseButton {
        match button {
            R_MOUSE_LEFT => MouseButton::Left,
            R_MOUSE_RIGHT => MouseButton::Right,
            R_MOUSE_MIDDLE => MouseButton::Middle,
            other => MouseButton::Other(other),
        }
    }
}

impl Key {
    fn from_raw(key: RKey) -> Key {
        match key {
            R_KEY_ESCAPE => Key::Escape,
            R_KEY_ENTER | R_KEY_KP_ENTER => Key::Enter,
            R_KEY_TAB => Key::Tab,
            R_KEY_SPACE => Key::Space,
            R_KEY_BACKSPACE => Key::Backspace,
            R_KEY_DELETE => Key::Delete,
            R_KEY_LEFT => Key::Left,
            R_KEY_RIGHT => Key::Right,
            R_KEY_UP => Key::Up,
            R_KEY_DOWN => Key::Down,
            R_KEY_HOME => Key::Home,
            R_KEY_END => Key::End,
            R_KEY_PAGE_UP => Key::PageUp,
            R_KEY_PAGE_DOWN => Key::PageDown,
            // Printable keys use their upper-case ASCII code.
            R_KEY_A..=R_KEY_Z | R_KEY_0..=R_KEY_9 => {
                char::from_u32(key as u32).map_or(Key::Other(key), Key::Char)
            }
            R_KEY_F1..=R_KEY_F12 => Key::F((key - R_KEY_F1 + 1) as u8),
            other => Key::Other(other),
        }
    }

    /// Raven's code of the key (`Key::Enter` is the main Enter key, not the keypad's).
    /// `None` for a character that is not an ASCII letter or digit, or an F key past F12.
    pub(crate) fn to_raw(self) -> Option<RKey> {
        Some(match self {
            Key::Escape => R_KEY_ESCAPE,
            Key::Enter => R_KEY_ENTER,
            Key::Tab => R_KEY_TAB,
            Key::Space => R_KEY_SPACE,
            Key::Backspace => R_KEY_BACKSPACE,
            Key::Delete => R_KEY_DELETE,
            Key::Left => R_KEY_LEFT,
            Key::Right => R_KEY_RIGHT,
            Key::Up => R_KEY_UP,
            Key::Down => R_KEY_DOWN,
            Key::Home => R_KEY_HOME,
            Key::End => R_KEY_END,
            Key::PageUp => R_KEY_PAGE_UP,
            Key::PageDown => R_KEY_PAGE_DOWN,
            // Printable keys use their upper-case ASCII code.
            Key::Char(c) if c.is_ascii_alphanumeric() => c.to_ascii_uppercase() as RKey,
            Key::Char(_) => return None,
            Key::F(n @ 1..=12) => R_KEY_F1 + RKey::from(n) - 1,
            Key::F(_) => return None,
            Key::Other(code) => code,
        })
    }
}

/// Raven's modifier bits for Ctrl and Shift.
pub(crate) fn modifiers(ctrl: bool, shift: bool) -> u32 {
    let mut bits = 0;
    if ctrl {
        bits |= R_MOD_CTRL as u32;
    }
    if shift {
        bits |= R_MOD_SHIFT as u32;
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(type_: REventType) -> REvent {
        // SAFETY: `REvent` is plain data for which all zeros is valid.
        let mut e: REvent = unsafe { std::mem::zeroed() };
        e.type_ = type_;
        e
    }

    #[test]
    fn keys_map_letters_digits_and_function_keys() {
        assert_eq!(Key::from_raw(R_KEY_S), Key::Char('S'));
        assert_eq!(Key::from_raw(R_KEY_7), Key::Char('7'));
        assert_eq!(Key::from_raw(R_KEY_F5), Key::F(5));
        assert_eq!(Key::from_raw(R_KEY_F12), Key::F(12));
        assert_eq!(Key::from_raw(R_KEY_KP_ENTER), Key::Enter);
        assert_eq!(Key::from_raw(R_KEY_LEFT_CTRL), Key::Other(R_KEY_LEFT_CTRL));
    }

    #[test]
    fn keys_go_back_to_their_raven_codes() {
        for raw in [
            R_KEY_S,
            R_KEY_7,
            R_KEY_F1,
            R_KEY_F12,
            R_KEY_ESCAPE,
            R_KEY_ENTER,
            R_KEY_PAGE_DOWN,
            R_KEY_LEFT_CTRL,
        ] {
            assert_eq!(Key::from_raw(raw).to_raw(), Some(raw));
        }
        assert_eq!(Key::Char('s').to_raw(), Some(R_KEY_S));
        assert_eq!(Key::Char('ç').to_raw(), None);
        assert_eq!(Key::F(13).to_raw(), None);
        assert_eq!(modifiers(true, false), R_MOD_CTRL as u32);
        assert_eq!(modifiers(true, true), (R_MOD_CTRL | R_MOD_SHIFT) as u32);
        assert_eq!(modifiers(false, false), 0);
    }

    #[test]
    fn events_use_local_coordinates_and_modifiers() {
        let mut e = raw(R_EVENT_KEY_DOWN);
        e.key = R_KEY_Z;
        e.modifiers = (R_MOD_CTRL | R_MOD_SHIFT) as u32;
        assert_eq!(
            Event::from_raw(&e),
            Some(Event::KeyDown {
                key: Key::Char('Z'),
                ctrl: true,
                shift: true
            })
        );

        let mut e = raw(R_EVENT_MOUSE_DOWN);
        (e.x, e.y, e.local_x, e.local_y) = (110.0, 220.0, 10.0, 20.0);
        e.button = R_MOUSE_RIGHT;
        e.clicks = 2;
        assert_eq!(
            Event::from_raw(&e),
            Some(Event::MouseDown {
                x: 10.0,
                y: 20.0,
                button: MouseButton::Right,
                clicks: 2
            })
        );

        assert_eq!(Event::from_raw(&raw(R_EVENT_KEY_UP)), None);
        assert_eq!(Event::from_raw(&raw(R_EVENT_FOCUS_GAINED)), None);
    }
}
