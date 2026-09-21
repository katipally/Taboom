use anyhow::Result;
use std::collections::HashSet;
use taboom_humanizer::InputEvent;
use tracing::debug;

pub trait UinputBackend: Send + Sync {
    fn emit_rel(&self, code: u16, value: i32) -> Result<()>;
    fn emit_key(&self, code: u16, value: i32) -> Result<()>;
    fn emit_wheel(&self, value: i32) -> Result<()>;
    fn emit_wheel_hi_res(&self, value: i32) -> Result<()>;
    fn emit_sync(&self) -> Result<()>;
    fn drain_summary(&self) -> Option<MockEventSummary> { None }
}

pub struct LinuxUinput {
    _mouse_fd: i32,
    _keyboard_fd: i32,
}

impl LinuxUinput {
    pub fn create() -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            anyhow::bail!("real uinput creation requires kernel support, use setup scripts");
        }
        #[cfg(not(target_os = "linux"))]
        {
            anyhow::bail!("uinput only available on Linux");
        }
    }
}

impl UinputBackend for LinuxUinput {
    fn emit_rel(&self, _code: u16, _value: i32) -> Result<()> {
        anyhow::bail!("stub: requires real uinput fd")
    }
    fn emit_key(&self, _code: u16, _value: i32) -> Result<()> {
        anyhow::bail!("stub: requires real uinput fd")
    }
    fn emit_wheel(&self, _value: i32) -> Result<()> {
        anyhow::bail!("stub: requires real uinput fd")
    }
    fn emit_wheel_hi_res(&self, _value: i32) -> Result<()> {
        anyhow::bail!("stub: requires real uinput fd")
    }
    fn emit_sync(&self) -> Result<()> {
        anyhow::bail!("stub: requires real uinput fd")
    }
}

pub struct MockUinput {
    pub events: std::sync::Mutex<Vec<MockEvent>>,
}

#[derive(Debug, Clone)]
pub enum MockEvent {
    Rel { code: u16, value: i32 },
    Key { code: u16, value: i32 },
    Wheel { value: i32 },
    WheelHiRes { value: i32 },
    Sync,
}

#[derive(Debug, Default)]
pub struct MockEventSummary {
    pub rels: usize,
    pub keys: usize,
    pub wheels: usize,
    pub syncs: usize,
    pub last_rel_code: u16,
    pub last_rel_value: i32,
    pub last_key_code: u16,
    pub last_key_value: i32,
    pub last_wheel_value: i32,
}

impl MockUinput {
    pub fn new() -> Self {
        Self {
            events: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn drain(&self) -> Vec<MockEvent> {
        self.events.lock().unwrap().drain(..).collect()
    }

    pub fn summarize_and_drain(&self) -> MockEventSummary {
        let events = self.drain();
        let mut summary = MockEventSummary::default();
        for ev in &events {
            match ev {
                MockEvent::Rel { code, value } => {
                    summary.rels += 1;
                    summary.last_rel_code = *code;
                    summary.last_rel_value = *value;
                }
                MockEvent::Key { code, value } => {
                    summary.keys += 1;
                    summary.last_key_code = *code;
                    summary.last_key_value = *value;
                }
                MockEvent::Wheel { value } => {
                    summary.wheels += 1;
                    summary.last_wheel_value = *value;
                }
                MockEvent::WheelHiRes { value } => {
                    summary.wheels += 1;
                    summary.last_wheel_value = *value;
                }
                MockEvent::Sync => summary.syncs += 1,
            }
        }
        summary
    }
}

impl UinputBackend for MockUinput {
    fn emit_rel(&self, code: u16, value: i32) -> Result<()> {
        self.events.lock().unwrap().push(MockEvent::Rel { code, value });
        Ok(())
    }
    fn emit_key(&self, code: u16, value: i32) -> Result<()> {
        self.events.lock().unwrap().push(MockEvent::Key { code, value });
        Ok(())
    }
    fn emit_wheel(&self, value: i32) -> Result<()> {
        self.events.lock().unwrap().push(MockEvent::Wheel { value });
        Ok(())
    }
    fn emit_wheel_hi_res(&self, value: i32) -> Result<()> {
        self.events.lock().unwrap().push(MockEvent::WheelHiRes { value });
        Ok(())
    }
    fn emit_sync(&self) -> Result<()> {
        self.events.lock().unwrap().push(MockEvent::Sync);
        Ok(())
    }
    fn drain_summary(&self) -> Option<MockEventSummary> {
        Some(self.summarize_and_drain())
    }
}

#[derive(Debug, Clone)]
pub struct InputConfig {
    pub accel_profile: AccelProfile,
    pub pointer_speed: f64,
}

#[derive(Debug, Clone, Copy)]
pub enum AccelProfile {
    Flat,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            accel_profile: AccelProfile::Flat,
            pointer_speed: 0.0,
        }
    }
}

pub struct InputDevices {
    backend: Box<dyn UinputBackend>,
    pressed_keys: HashSet<u16>,
    pressed_buttons: HashSet<u8>,
    cursor_x: i32,
    cursor_y: i32,
    config: InputConfig,
}

impl InputDevices {
    pub fn new(backend: Box<dyn UinputBackend>, config: InputConfig) -> Self {
        Self {
            backend,
            pressed_keys: HashSet::new(),
            pressed_buttons: HashSet::new(),
            cursor_x: 0,
            cursor_y: 0,
            config,
        }
    }

    pub fn cursor_position(&self) -> (i32, i32) {
        (self.cursor_x, self.cursor_y)
    }

    pub fn set_cursor_position(&mut self, x: i32, y: i32) {
        self.cursor_x = x;
        self.cursor_y = y;
    }

    pub fn execute(&mut self, event: &InputEvent) -> Result<()> {
        match event {
            InputEvent::MouseRel { dx, dy } => {
                let scale = 1.0 + self.config.pointer_speed;
                let sdx = (*dx as f64 * scale) as i32;
                let sdy = (*dy as f64 * scale) as i32;
                if sdx != 0 {
                    self.backend.emit_rel(REL_X, sdx)?;
                }
                if sdy != 0 {
                    self.backend.emit_rel(REL_Y, sdy)?;
                }
                self.cursor_x += sdx;
                self.cursor_y += sdy;
            }
            InputEvent::MouseButton { button, pressed } => {
                let code = match button {
                    0 => KEY_BTN_LEFT,
                    1 => KEY_BTN_RIGHT,
                    2 => KEY_BTN_MIDDLE,
                    _ => return Ok(()),
                };
                let value = if *pressed { 1 } else { 0 };
                self.backend.emit_key(code, value)?;
                if *pressed {
                    self.pressed_buttons.insert(*button);
                } else {
                    self.pressed_buttons.remove(button);
                }
            }
            InputEvent::Key { code, pressed } => {
                let value = if *pressed { 1 } else { 0 };
                self.backend.emit_key(*code, value)?;
                if *pressed {
                    self.pressed_keys.insert(*code);
                } else {
                    self.pressed_keys.remove(code);
                }
            }
            InputEvent::Wheel { delta, hi_res_delta } => {
                if *delta != 0 {
                    self.backend.emit_wheel(*delta)?;
                }
                if *hi_res_delta != 0 {
                    self.backend.emit_wheel_hi_res(*hi_res_delta)?;
                }
            }
            InputEvent::Sync => {
                self.backend.emit_sync()?;
            }
        }
        Ok(())
    }

    pub fn release_all(&mut self) -> Result<()> {
        let keys: Vec<u16> = self.pressed_keys.drain().collect();
        let key_count = keys.len();
        for code in keys {
            self.backend.emit_key(code, 0)?;
        }

        let buttons: Vec<u8> = self.pressed_buttons.drain().collect();
        let btn_count = buttons.len();
        for button in buttons {
            let code = match button {
                0 => KEY_BTN_LEFT,
                1 => KEY_BTN_RIGHT,
                2 => KEY_BTN_MIDDLE,
                _ => continue,
            };
            self.backend.emit_key(code, 0)?;
        }

        if key_count > 0 || btn_count > 0 {
            self.backend.emit_sync()?;
            debug!(keys = key_count, buttons = btn_count, "released all held inputs");
        }

        Ok(())
    }

    pub fn drain_backend_events(&self) -> Option<MockEventSummary> {
        self.backend.drain_summary()
    }
}

const REL_X: u16 = 0;
const REL_Y: u16 = 1;
const KEY_BTN_LEFT: u16 = 272;
const KEY_BTN_RIGHT: u16 = 273;
const KEY_BTN_MIDDLE: u16 = 274;

pub struct KeymapResolver {
    layout: String,
}

#[derive(Debug, Clone)]
pub struct KeyAction {
    pub keycode: u16,
    pub pressed: bool,
}

impl KeymapResolver {
    pub fn new(layout: &str) -> Self {
        Self {
            layout: layout.to_string(),
        }
    }

    pub fn char_to_keycodes(&self, ch: char) -> Vec<KeyAction> {
        let mut actions = Vec::new();
        let needs_shift = ch.is_ascii_uppercase()
            || matches!(
                ch,
                '!' | '@' | '#' | '$' | '%' | '^' | '&' | '*' | '(' | ')'
                    | '_' | '+' | '{' | '}' | '|' | ':' | '"' | '<' | '>' | '?' | '~'
            );

        let base_char = if needs_shift {
            match ch {
                '!' => '1', '@' => '2', '#' => '3', '$' => '4', '%' => '5',
                '^' => '6', '&' => '7', '*' => '8', '(' => '9', ')' => '0',
                '_' => '-', '+' => '=', '{' => '[', '}' => ']', '|' => '\\',
                ':' => ';', '"' => '\'', '<' => ',', '>' => '.', '?' => '/',
                '~' => '`',
                c => c.to_ascii_lowercase(),
            }
        } else {
            ch
        };

        let keycode = self.char_to_raw_keycode(base_char);
        let keycode = match keycode {
            Some(k) => k,
            None => return actions,
        };

        if needs_shift {
            actions.push(KeyAction { keycode: 42, pressed: true });
        }
        actions.push(KeyAction { keycode, pressed: true });
        actions.push(KeyAction { keycode, pressed: false });
        if needs_shift {
            actions.push(KeyAction { keycode: 42, pressed: false });
        }

        actions
    }

    fn char_to_raw_keycode(&self, ch: char) -> Option<u16> {
        match ch {
            'a'..='z' => Some(30 + (ch as u16 - b'a' as u16)),
            '0' => Some(11),
            '1'..='9' => Some(2 + (ch as u16 - b'1' as u16)),
            ' ' => Some(57),
            '.' => Some(52),
            ',' => Some(51),
            ';' => Some(39),
            '\'' => Some(40),
            '-' => Some(12),
            '=' => Some(13),
            '[' => Some(26),
            ']' => Some(27),
            '\\' => Some(43),
            '/' => Some(53),
            '`' => Some(41),
            '\n' | '\r' => Some(28),
            '\t' => Some(15),
            _ => None,
        }
    }

    pub fn can_type(&self, ch: char) -> bool {
        let base = if ch.is_ascii_uppercase() {
            ch.to_ascii_lowercase()
        } else {
            ch
        };
        let mapped = self.char_to_raw_keycode(base).is_some();
        if !mapped {
            debug!(layout = %self.layout, char = ?ch, "unmappable character");
        }
        mapped
    }
}

pub const PASTE_THRESHOLD: usize = 200;

pub fn should_paste(text: &str, resolver: &KeymapResolver) -> bool {
    if text.len() > PASTE_THRESHOLD {
        return true;
    }
    text.chars().any(|ch| !resolver.can_type(ch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_uinput_captures_events() {
        let mock = MockUinput::new();
        mock.emit_rel(0, 10).unwrap();
        mock.emit_key(30, 1).unwrap();
        mock.emit_wheel(3).unwrap();
        mock.emit_wheel_hi_res(120).unwrap();
        mock.emit_sync().unwrap();
        let events = mock.drain();
        assert_eq!(events.len(), 5);
        match &events[0] {
            MockEvent::Rel { code, value } => { assert_eq!(*code, 0); assert_eq!(*value, 10); }
            _ => panic!("expected Rel"),
        }
        match &events[1] {
            MockEvent::Key { code, value } => { assert_eq!(*code, 30); assert_eq!(*value, 1); }
            _ => panic!("expected Key"),
        }
        match &events[2] {
            MockEvent::Wheel { value } => assert_eq!(*value, 3),
            _ => panic!("expected Wheel"),
        }
        match &events[3] {
            MockEvent::WheelHiRes { value } => assert_eq!(*value, 120),
            _ => panic!("expected WheelHiRes"),
        }
    }

    #[test]
    fn release_all_clears_state() {
        let mock = Box::new(MockUinput::new());
        let mut devices = InputDevices::new(mock, InputConfig::default());
        devices.execute(&InputEvent::Key { code: 30, pressed: true }).unwrap();
        devices.execute(&InputEvent::MouseButton { button: 0, pressed: true }).unwrap();
        assert!(!devices.pressed_keys.is_empty());
        assert!(!devices.pressed_buttons.is_empty());
        devices.release_all().unwrap();
        assert!(devices.pressed_keys.is_empty());
        assert!(devices.pressed_buttons.is_empty());
    }

    #[test]
    fn keymap_resolver_basic() {
        let resolver = KeymapResolver::new("us");
        let actions = resolver.char_to_keycodes('a');
        assert_eq!(actions.len(), 2);
        assert!(actions[0].pressed);
        assert!(!actions[1].pressed);
    }

    #[test]
    fn keymap_resolver_shift() {
        let resolver = KeymapResolver::new("us");
        let actions = resolver.char_to_keycodes('A');
        assert_eq!(actions.len(), 4);
        assert_eq!(actions[0].keycode, 42);
    }

    #[test]
    fn paste_threshold_triggers() {
        let resolver = KeymapResolver::new("us");
        let short = "hello";
        let long = "a".repeat(PASTE_THRESHOLD + 1);
        assert!(!should_paste(short, &resolver));
        assert!(should_paste(&long, &resolver));
    }

    #[test]
    fn unmappable_char_triggers_paste() {
        let resolver = KeymapResolver::new("us");
        assert!(should_paste("hello\u{00e9}world", &resolver));
    }

    #[test]
    fn cursor_tracking() {
        let mock = Box::new(MockUinput::new());
        let mut devices = InputDevices::new(mock, InputConfig::default());
        devices.set_cursor_position(100, 200);
        devices.execute(&InputEvent::MouseRel { dx: 10, dy: -5 }).unwrap();
        assert_eq!(devices.cursor_position(), (110, 195));
    }
}
