//! Terminal byte decoding, key chords, bracketed paste, and mouse events.
//! This module has no rendering or filesystem side effects.

use std::fmt;

use crate::position::Size;

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    Backspace,
    Delete,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    F(u8),
    Space,
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct KeyEvent {
    pub key: Key,
    pub mods: Modifiers,
}

impl KeyEvent {
    pub fn plain(key: Key) -> Self {
        Self {
            key,
            mods: Modifiers::default(),
        }
    }

    pub fn char(ch: char) -> Self {
        Self {
            key: Key::Char(ch),
            mods: Modifiers::default(),
        }
    }

    pub fn ctrl(ch: char) -> Self {
        Self {
            key: Key::Char(ch),
            mods: Modifiers {
                ctrl: true,
                alt: false,
                shift: false,
            },
        }
    }

    pub fn canonical(&self) -> String {
        let mut out = String::new();
        if self.mods.ctrl {
            out.push_str("ctrl+");
        }
        if self.mods.alt {
            out.push_str("alt+");
        }
        if self.mods.shift {
            out.push_str("shift+");
        }
        out.push_str(&key_name(self.key));
        out
    }

    pub fn is_printable(&self) -> bool {
        !self.mods.ctrl && !self.mods.alt && matches!(self.key, Key::Char(_))
    }
}

pub fn key_name(key: Key) -> String {
    match key {
        Key::Char(c) => {
            if c == ' ' {
                "space".to_string()
            } else {
                c.to_lowercase().collect::<String>()
            }
        }
        Key::Enter => "enter".into(),
        Key::Tab => "tab".into(),
        Key::Backspace => "backspace".into(),
        Key::Delete => "delete".into(),
        Key::Escape => "esc".into(),
        Key::Up => "up".into(),
        Key::Down => "down".into(),
        Key::Left => "left".into(),
        Key::Right => "right".into(),
        Key::Home => "home".into(),
        Key::End => "end".into(),
        Key::PageUp => "pageup".into(),
        Key::PageDown => "pagedown".into(),
        Key::Insert => "insert".into(),
        Key::F(n) => format!("f{n}"),
        Key::Space => "space".into(),
    }
}

pub fn parse_key_name(name: &str) -> Option<Key> {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "enter" | "return" | "cr" => Some(Key::Enter),
        "tab" => Some(Key::Tab),
        "backspace" | "bs" => Some(Key::Backspace),
        "delete" | "del" => Some(Key::Delete),
        "esc" | "escape" => Some(Key::Escape),
        "up" => Some(Key::Up),
        "down" => Some(Key::Down),
        "left" => Some(Key::Left),
        "right" => Some(Key::Right),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "pageup" | "pgup" => Some(Key::PageUp),
        "pagedown" | "pgdn" => Some(Key::PageDown),
        "insert" | "ins" => Some(Key::Insert),
        "space" => Some(Key::Space),
        _ => {
            if let Some(rest) = lower.strip_prefix('f') {
                if let Ok(n) = rest.parse::<u8>() {
                    if (1..=12).contains(&n) {
                        return Some(Key::F(n));
                    }
                }
            }
            let mut chars = lower.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                Some(Key::Char(c))
            } else {
                None
            }
        }
    }
}

#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct KeyChord {
    pub keys: Vec<KeyEvent>,
}

impl KeyChord {
    pub fn single(event: KeyEvent) -> Self {
        Self { keys: vec![event] }
    }

    pub fn parse(text: &str) -> Result<Self, ChordError> {
        let mut keys = Vec::new();
        for token in text.split_whitespace() {
            let mut mods = Modifiers::default();
            let mut parts = token.split('+').collect::<Vec<_>>();
            let name = parts.pop().unwrap_or("");
            for part in parts {
                match part.to_ascii_lowercase().as_str() {
                    "ctrl" | "control" => mods.ctrl = true,
                    "alt" | "meta" => mods.alt = true,
                    "shift" => mods.shift = true,
                    other => {
                        return Err(ChordError(format!(
                            "unknown modifier {other:?} in {text:?}"
                        )))
                    }
                }
            }
            let key = parse_key_name(name)
                .ok_or_else(|| ChordError(format!("unknown key {name:?} in {text:?}")))?;
            keys.push(KeyEvent { key, mods });
        }
        if keys.is_empty() {
            return Err(ChordError("empty key chord".into()));
        }
        Ok(Self { keys })
    }

    pub fn as_string(&self) -> String {
        self.keys
            .iter()
            .map(KeyEvent::canonical)
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn starts_with(&self, prefix: &[KeyEvent]) -> bool {
        self.keys.len() >= prefix.len() && self.keys[..prefix.len()] == *prefix
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChordError(pub String);

impl fmt::Display for ChordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ChordError {}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum MouseKind {
    Press(u8),
    Release(u8),
    Drag(u8),
    Move,
    ScrollUp,
    ScrollDown,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct MouseEvent {
    pub kind: MouseKind,
    pub x: usize,
    pub y: usize,
    pub mods: Modifiers,
}

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum SignalKind {
    Resize,
    Suspend,
    Continue,
    Terminate,
    Interrupt,
}

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub enum Event {
    Key(KeyEvent),
    /// Bracketed paste is text, never a sequence of editor commands.
    Paste(String),
    Mouse(MouseEvent),
    Resize(Size),
    Focus(bool),
    Signal(SignalKind),
    /// No bytes arrived before the poll timeout. Used for async result polling.
    Tick,
    /// ESC settled as a standalone key after a short timeout.
    EscapeTimeout,
}

enum State {
    Ground,
    Paste,
}

pub struct InputParser {
    buffer: Vec<u8>,
    state: State,
    paste: Vec<u8>,
    pending_escape: bool,
}

impl Default for InputParser {
    fn default() -> Self {
        Self::new()
    }
}

impl InputParser {
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(256),
            state: State::Ground,
            paste: Vec::new(),
            pending_escape: false,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Event> {
        self.buffer.extend_from_slice(bytes);
        self.drain(false)
    }

    /// Emit a lone ESC (or discard incomplete UTF-8) after a short poll timeout.
    pub fn flush(&mut self) -> Vec<Event> {
        self.drain(true)
    }

    fn drain(&mut self, flushing: bool) -> Vec<Event> {
        let mut events = Vec::new();
        loop {
            match self.state {
                State::Paste => {
                    if let Some(end) = find_subslice(&self.buffer, b"\x1b[201~") {
                        self.paste.extend_from_slice(&self.buffer[..end]);
                        self.buffer.drain(..end + 6);
                        let text = sanitize_paste(&self.paste);
                        self.paste.clear();
                        self.state = State::Ground;
                        events.push(Event::Paste(text));
                    } else {
                        self.paste.extend_from_slice(&self.buffer);
                        self.buffer.clear();
                        return events;
                    }
                }
                State::Ground => {
                    if self.buffer.is_empty() {
                        return events;
                    }
                    let first = self.buffer[0];
                    if first == 0x1b {
                        if self.buffer.len() == 1 {
                            if flushing && self.pending_escape {
                                self.buffer.clear();
                                self.pending_escape = false;
                                events.push(Event::Key(KeyEvent::plain(Key::Escape)));
                                return events;
                            }
                            self.pending_escape = true;
                            return events;
                        }
                        self.pending_escape = false;
                        if let Some((event, used)) = self.parse_escape_sequence() {
                            self.buffer.drain(..used);
                            match event {
                                ParseOutcome::Event(e) => events.push(e),
                                ParseOutcome::StartPaste => self.state = State::Paste,
                                ParseOutcome::Ignore => {}
                            }
                        } else {
                            // Incomplete escape sequence; wait for more bytes.
                            return events;
                        }
                    } else if first >= 0x80 {
                        if let Some((ch, used)) = decode_utf8_at(&self.buffer) {
                            self.buffer.drain(..used);
                            events.push(Event::Key(KeyEvent::char(ch)));
                        } else if flushing {
                            self.buffer.remove(0);
                            events.push(Event::Key(KeyEvent::char('\u{fffd}')));
                        } else {
                            return events;
                        }
                    } else {
                        self.buffer.remove(0);
                        if let Some(event) = control_byte_event(first) {
                            events.push(Event::Key(event));
                        }
                    }
                }
            }
        }
    }

    fn parse_escape_sequence(&self) -> Option<(ParseOutcome, usize)> {
        let b = &self.buffer;
        if b.len() < 2 {
            return None;
        }
        match b[1] {
            b'[' => self.parse_csi(),
            b'O' => self.parse_ss3(),
            b']' => self.parse_osc(),
            b'P' | b'X' | b'^' | b'_' => self.parse_osc(),
            0x1b => Some((
                ParseOutcome::Event(Event::Key(KeyEvent::plain(Key::Escape))),
                1,
            )),
            _ => {
                if let Some((ch, used)) = decode_utf8_at(&b[1..]) {
                    let mut event = KeyEvent::char(ch);
                    event.mods.alt = true;
                    Some((ParseOutcome::Event(Event::Key(event)), used + 1))
                } else {
                    None
                }
            }
        }
    }

    fn parse_csi(&self) -> Option<(ParseOutcome, usize)> {
        let b = &self.buffer;
        let mut i = 2usize;
        while i < b.len()
            && (b[i] == b'?'
                || b[i] == b'<'
                || b[i].is_ascii_digit()
                || b[i] == b';'
                || b[i] == b':')
        {
            i += 1;
        }
        if i >= b.len() {
            return None;
        }
        let final_byte = b[i];
        if !(0x40..=0x7e).contains(&final_byte) {
            return None;
        }
        let used = i + 1;
        let body = &b[2..i];
        if body == b"200" && final_byte == b'~' {
            return Some((ParseOutcome::StartPaste, used));
        }
        if body.is_empty() && final_byte == b'I' {
            return Some((ParseOutcome::Event(Event::Focus(true)), used));
        }
        if body.is_empty() && final_byte == b'O' {
            return Some((ParseOutcome::Event(Event::Focus(false)), used));
        }
        if final_byte == b'M' || final_byte == b'm' {
            if let Some(mouse) = parse_sgr_mouse(body, final_byte == b'M') {
                return Some((ParseOutcome::Event(Event::Mouse(mouse)), used));
            }
        }
        let params = parse_csi_params(body);
        let mods = params
            .get(1)
            .copied()
            .map(csi_modifiers)
            .unwrap_or_default();
        if final_byte == b'Z' {
            return Some((
                ParseOutcome::Event(Event::Key(KeyEvent {
                    key: Key::Tab,
                    mods: Modifiers {
                        shift: true,
                        ..mods
                    },
                })),
                used,
            ));
        }
        let key = match (final_byte, params.first().copied().unwrap_or(1)) {
            (b'A', _) => Some(Key::Up),
            (b'B', _) => Some(Key::Down),
            (b'C', _) => Some(Key::Right),
            (b'D', _) => Some(Key::Left),
            (b'H', _) => Some(Key::Home),
            (b'F', _) => Some(Key::End),
            (b'P', _) => Some(Key::F(1)),
            (b'Q', _) => Some(Key::F(2)),
            (b'R', _) => Some(Key::F(3)),
            (b'S', _) => Some(Key::F(4)),
            (b'~', 1) | (b'~', 7) => Some(Key::Home),
            (b'~', 2) => Some(Key::Insert),
            (b'~', 3) => Some(Key::Delete),
            (b'~', 4) | (b'~', 8) => Some(Key::End),
            (b'~', 5) => Some(Key::PageUp),
            (b'~', 6) => Some(Key::PageDown),
            (b'~', 11) => Some(Key::F(1)),
            (b'~', 12) => Some(Key::F(2)),
            (b'~', 13) => Some(Key::F(3)),
            (b'~', 14) => Some(Key::F(4)),
            (b'~', 15) => Some(Key::F(5)),
            (b'~', 17) => Some(Key::F(6)),
            (b'~', 18) => Some(Key::F(7)),
            (b'~', 19) => Some(Key::F(8)),
            (b'~', 20) => Some(Key::F(9)),
            (b'~', 21) => Some(Key::F(10)),
            (b'~', 23) => Some(Key::F(11)),
            (b'~', 24) => Some(Key::F(12)),
            _ => None,
        };
        if let Some(key) = key {
            Some((
                ParseOutcome::Event(Event::Key(KeyEvent { key, mods })),
                used,
            ))
        } else {
            Some((ParseOutcome::Ignore, used))
        }
    }

    fn parse_ss3(&self) -> Option<(ParseOutcome, usize)> {
        if self.buffer.len() < 3 {
            return None;
        }
        let key = match self.buffer[2] {
            b'P' => Key::F(1),
            b'Q' => Key::F(2),
            b'R' => Key::F(3),
            b'S' => Key::F(4),
            b'A' => Key::Up,
            b'B' => Key::Down,
            b'C' => Key::Right,
            b'D' => Key::Left,
            b'H' => Key::Home,
            b'F' => Key::End,
            _ => return Some((ParseOutcome::Ignore, 3)),
        };
        Some((ParseOutcome::Event(Event::Key(KeyEvent::plain(key))), 3))
    }

    fn parse_osc(&self) -> Option<(ParseOutcome, usize)> {
        for i in 1..self.buffer.len() {
            if self.buffer[i] == 0x07 {
                return Some((ParseOutcome::Ignore, i + 1));
            }
            if self.buffer[i] == 0x1b && self.buffer.get(i + 1) == Some(&b'\\') {
                return Some((ParseOutcome::Ignore, i + 2));
            }
        }
        None
    }
}

enum ParseOutcome {
    Event(Event),
    StartPaste,
    Ignore,
}

fn control_byte_event(byte: u8) -> Option<KeyEvent> {
    let event = match byte {
        0x00 => KeyEvent {
            key: Key::Space,
            mods: Modifiers {
                ctrl: true,
                alt: false,
                shift: false,
            },
        },
        0x08 => KeyEvent::plain(Key::Backspace),
        0x09 => KeyEvent::plain(Key::Tab),
        0x0a | 0x0d => KeyEvent::plain(Key::Enter),
        0x01..=0x1a => KeyEvent::ctrl((b'a' + byte - 1) as char),
        0x1c => KeyEvent::ctrl('\\'),
        0x1d => KeyEvent::ctrl(']'),
        0x1e => KeyEvent::ctrl('^'),
        0x1f => KeyEvent::ctrl('_'),
        0x7f => KeyEvent::plain(Key::Backspace),
        0x20..=0x7e => KeyEvent::char(byte as char),
        _ => return None,
    };
    Some(event)
}

fn decode_utf8_at(bytes: &[u8]) -> Option<(char, usize)> {
    if bytes.is_empty() {
        return None;
    }
    let len = match bytes[0] {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return None,
    };
    if bytes.len() < len {
        return None;
    }
    let text = std::str::from_utf8(&bytes[..len]).ok()?;
    let ch = text.chars().next()?;
    Some((ch, ch.len_utf8()))
}

fn parse_csi_params(body: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    for part in body.split(|b| *b == b';' || *b == b':') {
        if part.is_empty() {
            out.push(0);
            continue;
        }
        let digits = part.strip_prefix(b"?").unwrap_or(part);
        out.push(
            std::str::from_utf8(digits)
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
        );
    }
    if out.is_empty() {
        out.push(1);
    }
    out
}

fn csi_modifiers(param: u32) -> Modifiers {
    let value = param.saturating_sub(1);
    Modifiers {
        shift: value & 1 != 0,
        alt: value & 2 != 0,
        ctrl: value & 4 != 0,
    }
}

fn parse_sgr_mouse(body: &[u8], press: bool) -> Option<MouseEvent> {
    let text = std::str::from_utf8(body.strip_prefix(b"<").unwrap_or(body)).ok()?;
    let mut parts = text.split(';');
    let code: u32 = parts.next()?.parse().ok()?;
    let x: usize = parts.next()?.parse().ok()?;
    let y: usize = parts.next()?.parse().ok()?;
    let mods = Modifiers {
        shift: code & 4 != 0,
        alt: code & 8 != 0,
        ctrl: code & 16 != 0,
    };
    let kind = if code & 64 != 0 {
        if code & 1 != 0 {
            MouseKind::ScrollDown
        } else {
            MouseKind::ScrollUp
        }
    } else if code & 32 != 0 {
        MouseKind::Drag(code as u8 & 3)
    } else if code & 64 == 0 && press {
        MouseKind::Press(code as u8 & 3)
    } else if !press {
        MouseKind::Release(code as u8 & 3)
    } else {
        MouseKind::Move
    };
    Some(MouseEvent {
        kind,
        x: x.saturating_sub(1),
        y: y.saturating_sub(1),
        mods,
    })
}

fn sanitize_paste(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' => out.push('\n'),
            '\t' => out.push('\t'),
            c if crate::util::sanitize_char(c) == '\u{fffd}' => out.push('\u{fffd}'),
            c => out.push(c),
        }
    }
    out
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(bytes: &[u8]) -> Vec<Event> {
        let mut p = InputParser::new();
        let mut events = p.feed(bytes);
        events.extend(p.flush());
        events
    }

    #[test]
    fn parses_control_and_escape_keys() {
        assert_eq!(parse_all(b"\x13"), vec![Event::Key(KeyEvent::ctrl('s'))]);
        assert_eq!(parse_all(b"\x1a"), vec![Event::Key(KeyEvent::ctrl('z'))]);
        assert_eq!(
            parse_all(b"\x1b"),
            vec![Event::Key(KeyEvent::plain(Key::Escape))]
        );
        assert_eq!(
            parse_all(b"\t"),
            vec![Event::Key(KeyEvent::plain(Key::Tab))]
        );
    }

    #[test]
    fn parses_modified_arrows_and_function_keys() {
        let events = parse_all(b"\x1b[1;5C\x1b[15~");
        assert_eq!(
            events[0],
            Event::Key(KeyEvent {
                key: Key::Right,
                mods: Modifiers {
                    ctrl: true,
                    alt: false,
                    shift: false
                },
            })
        );
        assert_eq!(events[1], Event::Key(KeyEvent::plain(Key::F(5))));
    }

    #[test]
    fn bracketed_paste_is_text_not_commands() {
        let events = parse_all(b"\x1b[200~hello\x1b[31m\r\nworld\x1b[201~");
        assert_eq!(
            events,
            vec![Event::Paste("hello\u{fffd}[31m\nworld".to_string())]
        );
    }

    #[test]
    fn parses_sgr_mouse_and_alt_key() {
        let events = parse_all(b"\x1b[<0;4;9M\x1bb");
        assert_eq!(
            events[0],
            Event::Mouse(MouseEvent {
                kind: MouseKind::Press(0),
                x: 3,
                y: 8,
                mods: Modifiers::default(),
            })
        );
        assert_eq!(
            events[1],
            Event::Key(KeyEvent {
                key: Key::Char('b'),
                mods: Modifiers {
                    ctrl: false,
                    alt: true,
                    shift: false
                },
            })
        );
    }

    #[test]
    fn split_utf8_arrives_safely() {
        let mut p = InputParser::new();
        let bytes = "é".as_bytes();
        assert!(p.feed(&bytes[..1]).is_empty());
        assert_eq!(p.feed(&bytes[1..]), vec![Event::Key(KeyEvent::char('é'))]);
    }

    #[test]
    fn chord_canonicalization_round_trips() {
        let chord = KeyChord::parse("ctrl+w shift+v").unwrap();
        assert_eq!(chord.as_string(), "ctrl+w shift+v");
        assert_eq!(
            KeyChord::parse("ctrl+shift+z").unwrap().as_string(),
            "ctrl+shift+z"
        );
        assert!(KeyChord::parse("ctrl+bogus").is_err());
    }
}
