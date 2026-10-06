//! Key presses as the dashboard understands them.
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Tab,
    BackTab,
    Up,
    Down,
    Esc,
    CtrlC,
    /// Any other key: it cancels a prompt, closes help, and does nothing else.
    Other,
}

/// The key of a terminal key event, or `None` for a key release (some terminals report them).
pub fn from_event(event: &KeyEvent) -> Option<Key> {
    if event.kind == KeyEventKind::Release {
        return None;
    }
    let control = event.modifiers.contains(KeyModifiers::CONTROL);
    let alt = event.modifiers.contains(KeyModifiers::ALT);
    Some(match event.code {
        KeyCode::Char('c') | KeyCode::Char('C') if control => Key::CtrlC,
        KeyCode::Char(c) if !control && !alt => Key::Char(c),
        KeyCode::Tab => Key::Tab,
        KeyCode::BackTab => Key::BackTab,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Esc => Key::Esc,
        _ => Key::Other,
    })
}
