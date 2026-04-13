//! Shared text-editing key handling for single-line-ish text inputs.
//!
//! Provides cursor-aware insert/delete/navigate on a `String` buffer.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Handle a text-editing key event. Returns true if the key was consumed.
///
/// The caller is responsible for handling non-text keys (Enter, Esc, etc.)
/// before calling this — any unhandled `Char`/arrow/Home/End/Backspace/Delete
/// should be delegated here.
pub fn handle_text_key(buf: &mut String, cursor: &mut usize, key: &KeyEvent) -> bool {
    // Clamp cursor in case the buffer was mutated externally
    if *cursor > buf.len() {
        *cursor = buf.len();
    }
    match key.code {
        KeyCode::Left => {
            *cursor = prev_char_boundary(buf, *cursor);
            true
        }
        KeyCode::Right => {
            *cursor = next_char_boundary(buf, *cursor);
            true
        }
        KeyCode::Home => {
            *cursor = 0;
            true
        }
        KeyCode::End => {
            *cursor = buf.len();
            true
        }
        KeyCode::Backspace => {
            if *cursor > 0 {
                let prev = prev_char_boundary(buf, *cursor);
                buf.replace_range(prev..*cursor, "");
                *cursor = prev;
            }
            true
        }
        KeyCode::Delete => {
            if *cursor < buf.len() {
                let next = next_char_boundary(buf, *cursor);
                buf.replace_range(*cursor..next, "");
            }
            true
        }
        KeyCode::Char(c) => {
            // Skip Ctrl-modified chars so callers can handle shortcuts
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                return false;
            }
            buf.insert(*cursor, c);
            *cursor += c.len_utf8();
            true
        }
        _ => false,
    }
}

fn prev_char_boundary(s: &str, i: usize) -> usize {
    if i == 0 {
        return 0;
    }
    let mut j = i - 1;
    while j > 0 && !s.is_char_boundary(j) {
        j -= 1;
    }
    j
}

fn next_char_boundary(s: &str, i: usize) -> usize {
    let len = s.len();
    if i >= len {
        return len;
    }
    let mut j = i + 1;
    while j < len && !s.is_char_boundary(j) {
        j += 1;
    }
    j
}
