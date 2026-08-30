//! The alert: a failure that already happened and has to be read.
//!
//! It exists because the alternative is `set_status`, which clears itself after
//! `STATUS_TTL` and is wiped by the next redraw. That is fine for "saved" and
//! wrong for "the client could not be run": a screen that comes back unchanged
//! reads as a keypress that did nothing, and the one thing the user needed was
//! the sentence the client printed just before we drew over it.
//!
//! Yellow, not red: it has already happened, and reading it costs nothing. Red
//! is reserved for a gate in front of something about to be lost.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::*;

pub(crate) struct Alert {
    title: String,
    body: String,
    scroll: u16,
}

impl Alert {
    pub(super) fn new(title: impl Into<String>, body: impl Into<String>) -> Alert {
        Alert {
            title: title.into(),
            body: body.into(),
            scroll: 0,
        }
    }
}

impl App {
    /// Raise one. Anything already up is replaced: the newest failure is the
    /// one being reacted to.
    pub(super) fn alert(&mut self, title: impl Into<String>, body: impl Into<String>) {
        self.alert = Some(Alert::new(title, body));
    }

    /// Keys while an alert is up. Movement scrolls a body too long for the box;
    /// anything that is not a dismissal is swallowed, so a stray keypress
    /// cannot close a message before it has been read.
    pub(super) fn alert_key(&mut self, key: KeyEvent) {
        use KeyCode::*;
        let Some(a) = &mut self.alert else { return };
        match key.code {
            Down | Char('j') | PageDown => a.scroll = a.scroll.saturating_add(1),
            Up | Char('k') | PageUp => a.scroll = a.scroll.saturating_sub(1),
            Esc | Enter | Char('q' | ' ') => self.alert = None,
            _ => {}
        }
    }
}

pub(super) fn render_alert(f: &mut Frame, area: Rect, a: &Alert) {
    let width = box_width(area.width);
    let rows = wrapped_line_count(&a.body, box_inner_width(width)) as u16;
    // The body, a blank, the keys.
    let rect = box_area(area, width, box_height(rows + 2, area.height));
    f.render_widget(Clear, rect);

    let mut lines: Vec<Line> = a.body.lines().map(|l| Line::raw(l.to_string())).collect();
    lines.push(Line::raw(""));
    lines.push(box_hint("j/k ↑↓ scroll · esc dismiss"));

    let para = Paragraph::new(lines)
        .block(box_block(Color::Yellow, &a.title))
        .wrap(Wrap { trim: false })
        .scroll((a.scroll, 0));
    f.render_widget(para, rect);
}
