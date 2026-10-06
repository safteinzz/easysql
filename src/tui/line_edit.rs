//! One line of typed text with a cursor, edited with the keys a shell answers
//! to (readline's emacs bindings). Every form field and the `/` filter go
//! through here, so a typo is fixed the same way everywhere.
//!
//! The cursor is kept as `back`, the number of characters after it, so code
//! that assigns a new value without knowing about the cursor leaves it at the
//! end, and a `back` larger than the text is read as the start.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;

/// Apply an editing key to `text`. Returns false for a key that is not
/// editing, which the caller is free to use.
pub(super) fn edit(text: &mut String, back: &mut usize, key: KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let mut chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut at = len - (*back).min(len);
    match key.code {
        KeyCode::Left => at = at.saturating_sub(1),
        KeyCode::Right => at = (at + 1).min(len),
        KeyCode::Home => at = 0,
        KeyCode::End => at = len,
        KeyCode::Backspace if alt || ctrl => {
            let from = word_start(&chars, at, char::is_alphanumeric);
            chars.drain(from..at);
            at = from;
        }
        KeyCode::Backspace => {
            if at > 0 {
                chars.remove(at - 1);
                at -= 1;
            }
        }
        KeyCode::Delete => {
            if at < len {
                chars.remove(at);
            }
        }
        KeyCode::Char(c) if ctrl => match c {
            'a' => at = 0,
            'e' => at = len,
            'b' => at = at.saturating_sub(1),
            'f' => at = (at + 1).min(len),
            'h' if at > 0 => {
                chars.remove(at - 1);
                at -= 1;
            }
            'd' if at < len => {
                chars.remove(at);
            }
            // Back to a space, like a shell, so a whole path or host goes at once.
            'w' => {
                let from = word_start(&chars, at, |c| !c.is_whitespace());
                chars.drain(from..at);
                at = from;
            }
            'u' => {
                chars.drain(..at);
                at = 0;
            }
            'h' | 'd' => {}
            // Not Ctrl-k: a form moves to the previous field with it.
            _ => return false,
        },
        KeyCode::Char(c) if alt => match c {
            'b' => at = word_start(&chars, at, char::is_alphanumeric),
            'f' => at = word_end(&chars, at),
            'd' => {
                let to = word_end(&chars, at);
                chars.drain(at..to);
            }
            _ => return false,
        },
        KeyCode::Char(c) => {
            chars.insert(at, c);
            at += 1;
        }
        _ => return false,
    }
    *back = chars.len() - at;
    *text = chars.into_iter().collect();
    true
}

/// Where the word ending at `at` starts: skip what is not part of a word, then
/// what is.
fn word_start(chars: &[char], mut at: usize, in_word: fn(char) -> bool) -> usize {
    while at > 0 && !in_word(chars[at - 1]) {
        at -= 1;
    }
    while at > 0 && in_word(chars[at - 1]) {
        at -= 1;
    }
    at
}

fn word_end(chars: &[char], mut at: usize) -> usize {
    while at < chars.len() && !chars[at].is_alphanumeric() {
        at += 1;
    }
    while at < chars.len() && chars[at].is_alphanumeric() {
        at += 1;
    }
    at
}

/// `text` drawn in `style` with the cursor on it: the character under the
/// cursor reversed, or a block after the last one.
pub(super) fn with_cursor(text: &str, back: usize, style: Style) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let at = chars.len() - back.min(chars.len());
    let before: String = chars[..at].iter().collect();
    let mut spans = vec![Span::styled(before, style)];
    match chars.get(at) {
        Some(c) => {
            spans.push(Span::styled(
                c.to_string(),
                style.add_modifier(Modifier::REVERSED),
            ));
            spans.push(Span::styled(
                chars[at + 1..].iter().collect::<String>(),
                style,
            ));
        }
        None => spans.push(Span::raw("█")),
    }
    spans
}
