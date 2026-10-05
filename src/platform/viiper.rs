#![cfg(windows)]

use std::{
    io::{Read, Write},
    net::{Shutdown, TcpStream, ToSocketAddrs},
    os::windows::process::CommandExt,
    path::PathBuf,
    process::{Child, Command},
    sync::{atomic::{AtomicU8, Ordering}, Mutex},
    time::Duration,
};

use base::message_proto::{key_event, KeyEvent, KeyboardMode, MouseEvent};
use hbb_common::log;
use serde_json::{json, Value};

use crate::platform::windows_hid::{control_key_usage, scan_code_usage};

const API_ADDR: &str = "127.0.0.1:3242";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const INIT_TIMEOUT: Duration = Duration::from_secs(5);
const DEVICE_TIMEOUT: Duration = Duration::from_secs(2);
const VIIPER_PATH_ENV: &str = "RUSTDESK_VIIPER_PATH";
const VIIPER_ENABLE_ENV: &str = "RUSTDESK_VIIPER";

const MOUSE_BUTTON_LEFT: i32 = 1;
const MOUSE_BUTTON_RIGHT: i32 = 2;
const MOUSE_BUTTON_MIDDLE: i32 = 4;
const MOUSE_BUTTON_BACK: i32 = 8;
const MOUSE_BUTTON_FORWARD: i32 = 16;

fn mouse_button_mask(button: i32) -> Option<u8> {
    match button {
        MOUSE_BUTTON_LEFT => Some(0x01),
        MOUSE_BUTTON_RIGHT => Some(0x02),
        MOUSE_BUTTON_MIDDLE => Some(0x04),
        MOUSE_BUTTON_BACK => Some(0x08),
        MOUSE_BUTTON_FORWARD => Some(0x10),
        _ => None,
    }
}

struct KeyboardState {
    modifiers: u8,
    keys: [bool; 256],
}

impl Default for KeyboardState {
    fn default() -> Self {
        Self {
            modifiers: 0,
            keys: [false; 256],
        }
    }
}

struct Backend {
    _process: Option<Child>,
    bus_id: u64,
    mouse_id: String,
    keyboard_id: String,
    mouse: TcpStream,
    keyboard: TcpStream,
    mouse_buttons: u8,
    keyboard_state: KeyboardState,
}

impl Backend {
    fn connect() -> Result<Self, String> {
        let usbip = usbip_path().ok_or_else(|| {
            "usbip-win2 is not installed; expected usbip.exe in PATH or C:\\Program Files\\USBip".to_owned()
        })?;
        let mut process = None;
        if api_request("ping", None).is_err() {
            let path = viiper_path().ok_or_else(|| {
                "VIIPER is not running and viiper.exe was not found".to_owned()
            })?;
            let mut command = Command::new(path);
            command
                .arg("server")
                .creation_flags(CREATE_NO_WINDOW)
                // VIIPER invokes usbip.exe for local attachment. The USBip
                // installer does not always add its directory to PATH.
                .env("PATH", path_with_usbip(&usbip));
            let child = command
                .spawn()
                .map_err(|e| format!("failed to start VIIPER: {e}"))?;
            process = Some(child);
            let deadline = std::time::Instant::now() + INIT_TIMEOUT;
            while std::time::Instant::now() < deadline {
                if api_request("ping", None).is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if api_request("ping", None).is_err() {
                return Err("VIIPER API did not become ready".to_owned());
            }
        }

        let buses = api_request("bus/list", None)?;
        let bus_id = if let Some(bus_id) = buses["buses"]
            .as_array()
            .and_then(|values| values.first())
            .and_then(Value::as_u64)
        {
            bus_id
        } else {
            api_request("bus/create", None)?
                .get("busId")
                .and_then(Value::as_u64)
                .ok_or_else(|| "VIIPER bus/create returned no busId".to_owned())?
        };

        let mouse_info = api_request(
            &format!("bus/{bus_id}/add"),
            Some(json!({"type": "mouse"})),
        )?;
        let keyboard_info = api_request(
            &format!("bus/{bus_id}/add"),
            Some(json!({"type": "keyboard"})),
        )?;
        let mouse_id = device_id(&mouse_info)?;
        let keyboard_id = device_id(&keyboard_info)?;
        let mouse = connect_device(bus_id, &mouse_id)?;
        let keyboard = connect_device(bus_id, &keyboard_id)?;

        Ok(Self {
            _process: process,
            bus_id,
            mouse_id,
            keyboard_id,
            mouse,
            keyboard,
            mouse_buttons: 0,
            keyboard_state: KeyboardState::default(),
        })
    }

    fn mouse_event(&mut self, event: &MouseEvent) -> bool {
        use crate::common::input::*;
        let event_type = event.mask & MOUSE_TYPE_MASK;
        let buttons = event.mask >> 3;
        match event_type {
            MOUSE_TYPE_MOVE_RELATIVE => self.send_mouse(event.x, event.y, 0, 0),
            MOUSE_TYPE_DOWN | MOUSE_TYPE_UP => {
                let Some(mask) = mouse_button_mask(buttons) else {
                    return false;
                };
                if event_type == MOUSE_TYPE_DOWN {
                    self.mouse_buttons |= mask;
                } else {
                    self.mouse_buttons &= !mask;
                }
                self.send_mouse(0, 0, 0, 0)
            }
            MOUSE_TYPE_WHEEL | MOUSE_TYPE_TRACKPAD => {
                self.send_mouse(0, 0, event.y, event.x)
            }
            _ => false,
        }
    }

    fn send_mouse(&mut self, dx: i32, dy: i32, wheel: i32, pan: i32) -> bool {
        let mut report = [0u8; 9];
        report[0] = self.mouse_buttons;
        report[1..3].copy_from_slice(&(dx.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes());
        report[3..5].copy_from_slice(&(dy.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes());
        report[5..7].copy_from_slice(&(wheel.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes());
        report[7..9].copy_from_slice(&(pan.clamp(i16::MIN as i32, i16::MAX as i32) as i16).to_le_bytes());
        self.mouse.write_all(&report).is_ok()
    }

    fn key_event(&mut self, event: &KeyEvent) -> bool {
        let pressed = event.down;
        let mapped = match event.union.as_ref() {
            Some(key_event::Union::Chr(code))
                if event.mode.enum_value_or(KeyboardMode::Legacy) != KeyboardMode::Legacy =>
            {
                scan_code_usage(*code)
            }
            Some(key_event::Union::ControlKey(control)) => {
                control_key_usage(control.enum_value_or_default())
            }
            _ => None,
        };
        let Some((usage, modifier)) = mapped else {
            return false;
        };
        if modifier != 0 {
            if pressed {
                self.keyboard_state.modifiers |= modifier;
            } else {
                self.keyboard_state.modifiers &= !modifier;
            }
        } else if usage != 0 {
            self.keyboard_state.keys[usage as usize] = pressed;
        }

        let pressed_keys: Vec<u8> = self
            .keyboard_state
            .keys
            .iter()
            .enumerate()
            .filter_map(|(usage, pressed)| pressed.then_some(usage as u8))
            .collect();
        let mut report = Vec::with_capacity(2 + pressed_keys.len());
        report.push(self.keyboard_state.modifiers);
        report.push(pressed_keys.len() as u8);
        report.extend_from_slice(&pressed_keys);
        self.keyboard.write_all(&report).is_ok()
    }

    fn reset(&mut self) {
        self.mouse_buttons = 0;
        let _ = self.send_mouse(0, 0, 0, 0);
        self.keyboard_state = KeyboardState::default();
        let _ = self.keyboard.write_all(&[0u8; 2]);
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        let _ = self.mouse.shutdown(Shutdown::Both);
        let _ = self.keyboard.shutdown(Shutdown::Both);
        api_remove_device(self.bus_id, &self.mouse_id);
        api_remove_device(self.bus_id, &self.keyboard_id);
        if let Some(mut process) = self._process.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

static AVAILABILITY: AtomicU8 = AtomicU8::new(0);
lazy_static::lazy_static! {
    static ref BACKEND: Mutex<Option<Backend>> = Mutex::new(None);
}

pub fn available() -> bool {
    if std::env::var(VIIPER_ENABLE_ENV).ok().as_deref() == Some("0") {
        return false;
    }
    match AVAILABILITY.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => match Backend::connect() {
            Ok(backend) => {
                *BACKEND.lock().unwrap() = Some(backend);
                AVAILABILITY.store(1, Ordering::Relaxed);
                true
            }
            Err(error) => {
                log::warn!("VIIPER backend unavailable: {error}");
                AVAILABILITY.store(2, Ordering::Relaxed);
                false
            }
        },
    }
}

pub fn mouse_event(event: &MouseEvent) -> bool {
    BACKEND
        .lock()
        .unwrap()
        .as_mut()
        .map(|backend| backend.mouse_event(event))
        .unwrap_or(false)
}

pub fn key_event(event: &KeyEvent) -> bool {
    BACKEND
        .lock()
        .unwrap()
        .as_mut()
        .map(|backend| backend.key_event(event))
        .unwrap_or(false)
}

pub fn reset() {
    if let Some(backend) = BACKEND.lock().unwrap().as_mut() {
        backend.reset();
    }
}

fn viiper_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var(VIIPER_PATH_ENV) {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    let path = std::env::current_exe().ok()?.parent()?.join("viiper.exe");
    path.is_file().then_some(path)
}

fn usbip_path() -> Option<PathBuf> {
    let path_entries = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>());
    let candidates = path_entries
        .chain(std::env::var_os("ProgramFiles").map(|root| PathBuf::from(root).join("USBip")))
        .chain(
            std::env::var_os("ProgramFiles(x86)")
                .map(|root| PathBuf::from(root).join("USBip")),
        )
        .flat_map(|root| {
            vec![root.join("usbip.exe"), root.join("bin").join("usbip.exe")]
        });
    candidates.into_iter().find(|path| path.is_file())
}

fn path_with_usbip(usbip: &PathBuf) -> String {
    let Some(parent) = usbip.parent() else {
        return std::env::var("PATH").unwrap_or_default();
    };
    let mut paths = vec![parent.to_path_buf()];
    paths.extend(
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path)),
    );
    std::env::join_paths(paths)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn api_request(path: &str, payload: Option<Value>) -> Result<Value, String> {
    api_request_line(match payload {
        Some(payload) => format!("{path} {payload}"),
        None => path.to_owned(),
    })
}

fn api_request_line(request: String) -> Result<Value, String> {
    let mut stream = TcpStream::connect_timeout(
        &API_ADDR
            .to_socket_addrs()
            .map_err(|e| e.to_string())?
            .next()
            .ok_or_else(|| "VIIPER API address did not resolve".to_owned())?,
        DEVICE_TIMEOUT,
    )
    .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(DEVICE_TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(format!("{request}\0").as_bytes())
        .map_err(|e| e.to_string())?;
    stream.shutdown(Shutdown::Write).ok();
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|e| e.to_string())?;
    let response = String::from_utf8(response).map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_str(response.trim()).map_err(|e| e.to_string())?;
    if value.get("status").and_then(Value::as_u64).is_some_and(|status| status >= 400) {
        return Err(value.to_string());
    }
    Ok(value)
}

fn api_remove_device(bus_id: u64, device_id: &str) {
    let _ = api_request_line(format!("bus/{bus_id}/remove {device_id}"));
}

fn connect_device(bus_id: u64, device_id: &str) -> Result<TcpStream, String> {
    let mut stream = TcpStream::connect_timeout(
        &API_ADDR
            .to_socket_addrs()
            .map_err(|e| e.to_string())?
            .next()
            .ok_or_else(|| "VIIPER device address did not resolve".to_owned())?,
        DEVICE_TIMEOUT,
    )
    .map_err(|e| e.to_string())?;
    stream
        .set_nodelay(true)
        .map_err(|e| e.to_string())?;
    stream
        .write_all(format!("bus/{bus_id}/{device_id}\0").as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(stream)
}

fn device_id(value: &Value) -> Result<String, String> {
    value["devId"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("VIIPER device response has no devId: {value}"))
}
