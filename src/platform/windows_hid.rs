use std::sync::{atomic::{AtomicU8, Ordering}, Mutex};

use base::message_proto::{key_event, ControlKey, KeyEvent, KeyboardMode, MouseEvent};
use hbb_common::log;

extern "C" {
    fn rustdesk_hid_submit(device: u8, data: *const u8, length: u8) -> i32;
}

const KEYBOARD_DEVICE: u8 = 1;
const MOUSE_DEVICE: u8 = 2;

#[derive(Default)]
struct State {
    keyboard: [u8; 8],
    mouse_buttons: u8,
}

lazy_static::lazy_static! {
    static ref STATE: Mutex<State> = Mutex::new(State::default());
}
static AVAILABILITY: AtomicU8 = AtomicU8::new(0);

pub fn available() -> bool {
    match AVAILABILITY.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let available = unsafe { rustdesk_hid_submit(0, std::ptr::null(), 0) == 0 };
            AVAILABILITY.store(if available { 1 } else { 2 }, Ordering::Relaxed);
            available
        }
    }
}

pub fn keyboard_usage(usage: u8, pressed: bool) -> bool {
    if usage == 0 {
        return false;
    }
    let mut state = STATE.lock().unwrap();
    let slot = state.keyboard[2..8]
        .iter()
        .position(|value| *value == usage);
    if pressed {
        if slot.is_some() {
            return submit_keyboard(&state.keyboard);
        }
        if let Some(empty) = state.keyboard[2..8].iter_mut().find(|value| **value == 0) {
            *empty = usage;
        } else {
            return false;
        }
    } else if let Some(index) = slot {
        state.keyboard[index + 2] = 0;
    } else {
        return true;
    }
    submit_keyboard(&state.keyboard)
}

pub fn keyboard_modifier(mask: u8, pressed: bool) -> bool {
    let mut state = STATE.lock().unwrap();
    if pressed {
        state.keyboard[0] |= mask;
    } else {
        state.keyboard[0] &= !mask;
    }
    submit_keyboard(&state.keyboard)
}

fn submit_keyboard(report: &[u8; 8]) -> bool {
    unsafe { rustdesk_hid_submit(KEYBOARD_DEVICE, report.as_ptr(), report.len() as u8) == 0 }
}

pub fn mouse_move(dx: i32, dy: i32) -> bool {
    let mut report = [0u8; 7];
    let state = STATE.lock().unwrap();
    report[0] = state.mouse_buttons;
    drop(state);
    report[2..4].copy_from_slice(&(dx.clamp(-32768, 32767) as i16).to_le_bytes());
    report[4..6].copy_from_slice(&(dy.clamp(-32768, 32767) as i16).to_le_bytes());
    unsafe { rustdesk_hid_submit(MOUSE_DEVICE, report.as_ptr(), report.len() as u8) == 0 }
}

pub fn mouse_button(button: i32, pressed: bool) -> bool {
    let mask = match button {
        1 => 0x01,
        2 => 0x02,
        4 => 0x04,
        8 => 0x08,
        16 => 0x10,
        _ => return false,
    };
    let mut state = STATE.lock().unwrap();
    if pressed {
        state.mouse_buttons |= mask;
    } else {
        state.mouse_buttons &= !mask;
    }
    let report = [state.mouse_buttons, 0, 0, 0, 0, 0, 0];
    unsafe { rustdesk_hid_submit(MOUSE_DEVICE, report.as_ptr(), report.len() as u8) == 0 }
}

pub fn mouse_wheel(vertical: i32, horizontal: i32) -> bool {
    let state = STATE.lock().unwrap();
    let report = [
        state.mouse_buttons,
        0,
        0,
        0,
        0,
        vertical.clamp(-127, 127) as i8 as u8,
        horizontal.clamp(-127, 127) as i8 as u8,
    ];
    drop(state);
    unsafe { rustdesk_hid_submit(MOUSE_DEVICE, report.as_ptr(), report.len() as u8) == 0 }
}

pub fn reset() {
    if let Ok(mut state) = STATE.lock() {
        state.keyboard = [0; 8];
        state.mouse_buttons = 0;
        let _ = submit_keyboard(&state.keyboard);
        let mouse = [0, 0, 0, 0, 0, 0, 0];
        unsafe {
            let _ = rustdesk_hid_submit(MOUSE_DEVICE, mouse.as_ptr(), mouse.len() as u8);
        }
    } else {
        log::warn!("Failed to reset RustDesk virtual HID state");
    }
}

fn scan_code_usage(code: u32) -> Option<(u8, u8)> {
    let extended = code & 0xFF00 == 0xE000;
    let scan = (code & 0xFF) as u8;
    let modifier = match (extended, scan) {
        (false, 0x1d) => 0x01,
        (true, 0x1d) => 0x10,
        (false, 0x2a) => 0x02,
        (false, 0x36) => 0x20,
        (false, 0x38) => 0x04,
        (true, 0x38) => 0x40,
        (true, 0x5b) => 0x08,
        (true, 0x5c) => 0x80,
        _ => 0,
    };
    if modifier != 0 {
        return Some((0, modifier));
    }
    let usage = match (extended, scan) {
        (false, 0x01) => 0x29,
        (false, 0x0e) => 0x2a,
        (false, 0x0f) => 0x2b,
        (false, 0x1c) => 0x28,
        (false, 0x39) => 0x2c,
        (false, 0x3a) => 0x39,
        (false, 0x3b..=0x44) => scan - 0x3b + 0x3a,
        (false, 0x57) => 0x44,
        (false, 0x58) => 0x45,
        (false, 0x02..=0x0a) => scan + 0x1c,
        (false, 0x0b) => 0x27,
        (false, 0x0c) => 0x2d,
        (false, 0x0d) => 0x2e,
        (false, 0x1a) => 0x2f,
        (false, 0x1b) => 0x30,
        (false, 0x2b) => 0x31,
        (false, 0x27) => 0x33,
        (false, 0x28) => 0x34,
        (false, 0x29) => 0x35,
        (false, 0x33) => 0x36,
        (false, 0x34) => 0x37,
        (false, 0x35) => 0x38,
        (false, 0x10..=0x19) => scan - 0x10 + 0x14,
        (false, 0x1e..=0x26) => scan - 0x1e + 0x04,
        (false, 0x2c..=0x32) => scan - 0x2c + 0x1d,
        (true, 0x47) => 0x4a,
        (true, 0x4f) => 0x4d,
        (true, 0x49) => 0x4b,
        (true, 0x51) => 0x4e,
        (true, 0x52) => 0x49,
        (true, 0x53) => 0x4c,
        (true, 0x48) => 0x52,
        (true, 0x50) => 0x51,
        (true, 0x4b) => 0x50,
        (true, 0x4d) => 0x4f,
        (true, 0x35) => 0x54,
        (true, 0x1c) => 0x58,
        _ => return None,
    };
    Some((usage, 0))
}

fn control_key_usage(key: ControlKey) -> Option<(u8, u8)> {
    let value = match key {
        ControlKey::Alt | ControlKey::Option | ControlKey::Menu => return Some((0, 0x04)),
        ControlKey::RAlt => return Some((0, 0x40)),
        ControlKey::Control => return Some((0, 0x01)),
        ControlKey::RControl => return Some((0, 0x10)),
        ControlKey::Shift => return Some((0, 0x02)),
        ControlKey::RShift => return Some((0, 0x20)),
        ControlKey::Meta => return Some((0, 0x08)),
        ControlKey::RWin => return Some((0, 0x80)),
        ControlKey::Backspace => 0x2a,
        ControlKey::CapsLock => 0x39,
        ControlKey::Delete => 0x4c,
        ControlKey::DownArrow => 0x51,
        ControlKey::End => 0x4d,
        ControlKey::Escape => 0x29,
        ControlKey::Home => 0x4a,
        ControlKey::LeftArrow => 0x50,
        ControlKey::PageDown => 0x4e,
        ControlKey::PageUp => 0x4b,
        ControlKey::Return | ControlKey::NumpadEnter => 0x28,
        ControlKey::RightArrow => 0x4f,
        ControlKey::Space => 0x2c,
        ControlKey::Tab => 0x2b,
        ControlKey::UpArrow => 0x52,
        ControlKey::Insert => 0x49,
        ControlKey::NumLock => 0x53,
        ControlKey::Scroll => 0x47,
        ControlKey::Apps => 0x65,
        ControlKey::Multiply => 0x55,
        ControlKey::Add => 0x57,
        ControlKey::Subtract => 0x56,
        ControlKey::Decimal => 0x63,
        ControlKey::Divide => 0x54,
        ControlKey::Equals => 0x67,
        ControlKey::F1 => 0x3a,
        ControlKey::F2 => 0x3b,
        ControlKey::F3 => 0x3c,
        ControlKey::F4 => 0x3d,
        ControlKey::F5 => 0x3e,
        ControlKey::F6 => 0x3f,
        ControlKey::F7 => 0x40,
        ControlKey::F8 => 0x41,
        ControlKey::F9 => 0x42,
        ControlKey::F10 => 0x43,
        ControlKey::F11 => 0x44,
        ControlKey::F12 => 0x45,
        ControlKey::Numpad0 => 0x62,
        ControlKey::Numpad1 => 0x59,
        ControlKey::Numpad2 => 0x5a,
        ControlKey::Numpad3 => 0x5b,
        ControlKey::Numpad4 => 0x5c,
        ControlKey::Numpad5 => 0x5d,
        ControlKey::Numpad6 => 0x5e,
        ControlKey::Numpad7 => 0x5f,
        ControlKey::Numpad8 => 0x60,
        ControlKey::Numpad9 => 0x61,
        _ => return None,
    };
    Some((value, 0))
}

pub fn key_event(event: &KeyEvent) -> bool {
    let pressed = event.down;
    match event.union.as_ref() {
        Some(key_event::Union::Chr(code))
            if event.mode.enum_value_or(KeyboardMode::Legacy) != KeyboardMode::Legacy =>
        {
            let Some((usage, modifier)) = scan_code_usage(*code) else {
                return false;
            };
            if modifier != 0 {
                keyboard_modifier(modifier, pressed)
            } else {
                keyboard_usage(usage, pressed)
            }
        }
        Some(key_event::Union::ControlKey(control)) => {
            let Some((usage, modifier)) = control_key_usage(control.enum_value_or_default()) else {
                return false;
            };
            if modifier != 0 {
                keyboard_modifier(modifier, pressed)
            } else {
                keyboard_usage(usage, pressed)
            }
        }
        _ => false,
    }
}

pub fn mouse_event(event: &MouseEvent) -> bool {
    use crate::common::input::*;
    let event_type = event.mask & MOUSE_TYPE_MASK;
    let buttons = event.mask >> 3;
    match event_type {
        MOUSE_TYPE_MOVE_RELATIVE => mouse_move(event.x, event.y),
        MOUSE_TYPE_DOWN | MOUSE_TYPE_UP => mouse_button(buttons, event_type == MOUSE_TYPE_DOWN),
        MOUSE_TYPE_WHEEL | MOUSE_TYPE_TRACKPAD => mouse_wheel(event.y, event.x),
        _ => false,
    }
}
