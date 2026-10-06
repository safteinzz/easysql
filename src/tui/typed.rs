//! The typed gate (red, no buttons, one field): in front of deleting what
//! exists nowhere else. Enter does nothing until the field holds the thing's
//! exact name, which the body shows so it is copied rather than guessed.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::confirm::ConfirmAction;
use super::widgets::TYPED_DEL_KEYS;
use super::*;

pub(crate) struct Typed {
    title: String,
    message: String,
    /// What has to be typed.
    name: String,
    input: String,
    /// The cursor in `input`, as characters after it (`line_edit::edit`).
    back: usize,
    action: ConfirmAction,
}

impl Typed {
    pub(super) fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        name: impl Into<String>,
        action: ConfirmAction,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            name: name.into(),
            input: String::new(),
            back: 0,
            action,
        }
    }

    fn unlocked(&self) -> bool {
        self.input == self.name
    }
}

impl App {
    pub(super) fn typed_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        let t = self.typed.as_mut()?;
        if key.code == KeyCode::Esc || super::input::is_ctrl_c(key) {
            self.typed = None;
            self.set_status("cancelled");
            return None;
        }
        match key.code {
            KeyCode::Enter if t.unlocked() => {
                let t = self.typed.take()?;
                return self.run_confirmed(t.action);
            }
            KeyCode::Enter => {}
            _ => {
                line_edit::edit(&mut t.input, &mut t.back, key);
            }
        }
        None
    }
}

pub(super) fn render_typed(f: &mut Frame, area: Rect, t: &Typed) {
    let width = box_width(area.width);
    let inner = box_inner_width(width);
    let field = format!("type {}:  {}█", t.name, t.input);
    // The message, a blank, the field, a blank, the keys, each counted wrapped.
    let rows = wrapped_line_count(&t.message, inner) + 1 + wrapped_line_count(&field, inner) + 2;
    let rect = box_area(area, width, box_height(rows as u16, area.height));
    f.render_widget(Clear, rect);

    let mut lines: Vec<Line> = t
        .message
        .lines()
        .map(|l| Line::raw(l.to_string()))
        .collect();
    lines.push(Line::raw(""));
    let typed = Style::default()
        .add_modifier(Modifier::BOLD)
        .fg(if t.unlocked() {
            Color::Red
        } else {
            Color::Reset
        });
    lines.push(Line::from(
        std::iter::once(Span::raw(format!("type {}:  ", t.name)))
            .chain(line_edit::with_cursor(&t.input, t.back, typed))
            .collect::<Vec<_>>(),
    ));
    lines.push(Line::raw(""));
    lines.push(box_hint(TYPED_DEL_KEYS));
    let para = Paragraph::new(lines)
        .block(box_block(Color::Red, &t.title))
        .wrap(Wrap { trim: false });
    f.render_widget(para, rect);
}
