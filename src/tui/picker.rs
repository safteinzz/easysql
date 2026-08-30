//! Picking a value the app already knows (a connection, an engine, an ssh host)
//! instead of retyping it.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use super::*;

/// A modal list picker. Anything the app already knows must be picked, never
/// re-typed: `items` is what you read, `values` is what the action receives, and
/// they differ whenever a readable row would be a useless machine answer.
pub(crate) struct Picker {
    pub(crate) title: String,
    pub(crate) items: Vec<String>,
    pub(crate) values: Vec<String>,
    pub(crate) idx: usize,
    pub(crate) action: PickerAction,
}

impl Picker {
    /// A picker whose rows are their own answer.
    pub(super) fn plain(
        title: impl Into<String>,
        items: Vec<String>,
        action: PickerAction,
    ) -> Self {
        Self {
            title: title.into(),
            values: items.clone(),
            items,
            idx: 0,
            action,
        }
    }

    /// A picker whose rows read one way and answer another: `(shown, value)`.
    pub(super) fn keyed(
        title: impl Into<String>,
        rows: Vec<(String, String)>,
        action: PickerAction,
    ) -> Self {
        Self {
            title: title.into(),
            items: rows.iter().map(|(shown, _)| shown.clone()).collect(),
            values: rows.into_iter().map(|(_, value)| value).collect(),
            idx: 0,
            action,
        }
    }
}

pub(crate) enum PickerAction {
    /// Which engine a new connection is for; the value is the engine slug.
    NewConnEngine,
    /// Which connection a password unlocks; the value is a `Conn::key`.
    PasswordFor,
    /// Write the chosen value into the active wizard's field at this index.
    FillField { field: usize },
}

pub(super) fn render_picker(f: &mut Frame, area: Rect, p: &Picker) {
    // Ten rows of list at most, so a long host list scrolls inside the box
    // rather than growing one that swallows the screen.
    // Two more rows than the list needs, for the blank and the key line, which
    // live in the body here like in every other kind of box.
    let rows = (p.items.len() as u16).clamp(1, 10);
    let rect = box_area(
        area,
        box_width(area.width),
        box_height(rows + 2, area.height),
    );
    f.render_widget(Clear, rect);

    // The frame is drawn on its own so the list and the key line can share the
    // body: a List cannot hold a trailing line of its own.
    let block = super::widgets::box_block(Color::Cyan, &p.title);
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let list_area = Rect {
        height: inner.height.saturating_sub(2),
        ..inner
    };
    let hint_area = Rect {
        y: inner.y + inner.height.saturating_sub(1),
        height: 1,
        ..inner
    };

    let items: Vec<ListItem> = p
        .items
        .iter()
        .map(|it| ListItem::new(Span::raw(it.clone())))
        .collect();
    let list = List::new(items)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("▸ ");

    // A local state gives us scrolling for free when there are many rows.
    let mut state = ListState::default();
    state.select(Some(p.idx));
    f.render_stateful_widget(list, list_area, &mut state);
    f.render_widget(
        Paragraph::new(super::widgets::box_hint(
            "j/k ↑↓ move · enter pick · esc cancel",
        )),
        hint_area,
    );
}

impl App {
    /// Drive a modal list picker: the list keys move, Enter chooses, Esc bails.
    pub(super) fn picker_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = self.picker.as_ref().map(|p| p.items.len()).unwrap_or(0);
        if len == 0 {
            self.picker = None;
            return None;
        }
        match key.code {
            KeyCode::Esc => {
                self.picker = None;
                self.set_status("cancelled");
            }
            KeyCode::Char('c') if ctrl => {
                self.picker = None;
                self.set_status("cancelled");
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let p = self.picker.as_mut().unwrap();
                p.idx = (p.idx + 1) % len;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let p = self.picker.as_mut().unwrap();
                p.idx = (p.idx + len - 1) % len;
            }
            KeyCode::Enter => return self.submit_picker(),
            _ => {}
        }
        None
    }

    pub(super) fn submit_picker(&mut self) -> Option<PendingRun> {
        let picker = self.picker.take()?;
        let choice = picker.values.get(picker.idx)?.clone();
        match picker.action {
            // Choosing an engine just opens the connection wizard for it: which
            // fields it even asks about depends on the answer.
            PickerAction::NewConnEngine => {
                let engine = Engine::from_slug(&choice)?;
                self.prompt = Some(Prompt::add_conn(engine));
                None
            }
            PickerAction::PasswordFor => {
                let conn = self.conns.iter().find(|c| c.key() == choice)?.clone();
                self.prompt = Some(Prompt::password(&conn));
                None
            }
            PickerAction::FillField { field } => {
                let p = self.prompt.as_mut()?;
                if let Some(f) = p.fields.get_mut(field) {
                    f.value = choice.clone();
                }
                // Picking the ssh host decides what the far end of the forward
                // means, so the target field is recomputed rather than left on
                // the answer for a different hop. Only the tunnel wizard has
                // both fields, so this is a no-op anywhere else.
                let target = p
                    .fields
                    .iter()
                    .position(|f| f.label.contains("as that machine sees it"));
                if let Some(i) = target {
                    let was = p.fields[i].value.clone();
                    let now = super::prompt::forward_target(&choice, &was);
                    if now != was {
                        p.fields[i].value = now;
                        self.set_status(format!(
                            "the database is on {choice} itself, so the forward points at its 127.0.0.1"
                        ));
                    }
                }
                None
            }
        }
    }

    /// The engine picker behind `c` on the Connections tab, and the one behind a
    /// password with no connection to hang it on.
    pub(super) fn pick_engine(&mut self) {
        let rows: Vec<(String, String)> = engines::ENGINES
            .into_iter()
            .map(|e| {
                let installed = self.installed[e.idx()];
                // Just the fact, not the command: you are choosing which database
                // to add a connection for, and an install command you cannot run
                // from here is noise. The offer to run it comes at Enter, on the
                // connection itself, where saying yes actually does something.
                let shown = format!(
                    "{:<9} {:<8}{}",
                    e.label(),
                    e.default_client(),
                    if installed {
                        String::new()
                    } else {
                        // Name the *package*, which is the part nobody can guess:
                        // a bare "not installed" reads as though postgres itself
                        // were absent, and "no psql" still leaves you wondering
                        // what to ask your package manager for.
                        match engines::install_package(e) {
                            Some(pkg) => format!("needs {pkg}"),
                            None => format!("no {} on this machine", e.default_client()),
                        }
                    }
                );
                (shown, e.slug().to_string())
            })
            .collect();
        self.picker = Some(Picker::keyed(
            "Which database?",
            rows,
            PickerAction::NewConnEngine,
        ));
    }

    /// Which connection a new password is for. Only the engines whose client
    /// reads a password file of its own can appear here: a SQLite file has
    /// nothing to unlock, and sqlcmd asks for itself.
    pub(super) fn pick_password_target(&mut self) {
        let rows: Vec<(String, String)> = self
            .conns
            .iter()
            .filter(|c| c.engine.stores_password())
            .map(|c| {
                (
                    format!("{:<9} {:<16} {}", c.engine.label(), c.name, c.target()),
                    c.key(),
                )
            })
            .collect();
        if rows.is_empty() {
            self.set_status("no connections to save a password for yet (press c on Connections)");
            return;
        }
        self.picker = Some(Picker::keyed(
            "Which connection is this password for?",
            rows,
            PickerAction::PasswordFor,
        ));
    }
}
