//! Yes/No gates in front of anything destructive, and the offered fixes that
//! follow a failed connection.
//!
//! The two are not the same modal and must not look the same. A gate guards
//! something you cannot undo, so it is red and starts on No. An offer is the
//! app volunteering the next step after the client already failed, so it is
//! cyan and starts on Yes: you asked for this by pressing Enter.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::probe::{self, Fix};
use super::widgets::shell_join;
use super::*;

pub(crate) struct Confirm {
    pub(crate) title: String,
    pub(crate) message: String,
    pub(crate) action: ConfirmAction,
    /// Which button is selected.
    pub(crate) yes: bool,
    /// A gate in front of something irreversible, rather than an offered fix.
    pub(crate) danger: bool,
}

impl Confirm {
    /// A gate. Starts on No, because these guard destructive things and a reflex
    /// Enter must never be the one that fires them.
    pub(super) fn new(
        title: impl Into<String>,
        message: impl Into<String>,
        action: ConfirmAction,
    ) -> Self {
        Self {
            title: title.into(),
            message: message.into(),
            action,
            yes: false,
            danger: true,
        }
    }

    /// An offered fix. Starts on Yes: nothing here is destructive, and the user
    /// arrived at it by asking for something that did not work.
    pub(super) fn offer(
        title: impl Into<String>,
        message: impl Into<String>,
        action: ConfirmAction,
    ) -> Self {
        Self {
            yes: true,
            danger: false,
            ..Self::new(title, message, action)
        }
    }
}

pub(crate) enum ConfirmAction {
    DeleteConn {
        engine: Engine,
        name: String,
    },
    ForgetPassword(creds::Cred),
    /// Offered after an authentication failure: open the password wizard.
    SavePassword {
        key: String,
    },
    /// Offered when nothing answered, or `pg_hba.conf` refused this address.
    /// Point a connection at the near end of a forward that already exists.
    RepointConn {
        key: String,
        local: String,
    },
    OpenTunnel {
        key: String,
    },
    /// Offered when the server was reached but the database or role is wrong.
    FixConn {
        key: String,
    },
    /// Offered when the client itself is missing: run this machine's own package
    /// manager, suspended, so sudo and the manager can both prompt.
    InstallClient {
        engine: Engine,
    },
}

pub(super) fn render_confirm(f: &mut Frame, area: Rect, c: &Confirm) {
    // Size the box to the wrapped message so short prompts stay small and long
    // ones (a server's own error text) are not cut off. Padding gives every line,
    // wrapped continuations included, a uniform margin instead of butting the border.
    let width = (area.width * 66 / 100).clamp(34, 74);
    let inner = width.saturating_sub(6) as usize; // borders (2) + horizontal padding (4)
    let msg_rows = wrapped_line_count(&c.message, inner) as u16;
    // borders (2) + vertical padding (2) + blank + buttons. Forgetting the borders
    // here clips the buttons off, leaving no visible way to answer.
    let height = (msg_rows + 6).min(area.height);

    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    f.render_widget(Clear, rect);

    let accent = if c.danger { Color::Red } else { Color::Cyan };
    // The selected button is highlighted, so the answer is visible at a glance
    // rather than being a keystroke you had to know about.
    let button = |label: &str, selected: bool| {
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::DIM)
        };
        Span::styled(format!(" {label} "), style)
    };
    let lines = vec![
        Line::raw(c.message.clone()),
        Line::raw(""),
        Line::from(vec![
            button("Yes (y)", c.yes),
            Span::raw("  "),
            button("No (n)", !c.yes),
            Span::styled(
                "   ←/→ then Enter",
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]),
    ];
    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", c.title))
                .border_style(Style::default().fg(accent))
                .padding(Padding::new(2, 2, 1, 1)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(para, rect);
}

impl App {
    /// Resolve a pending yes/no. `y` proceeds and `n`/`Esc` cancels outright, or
    /// move between the buttons (`h`/`l`, the arrows, Tab) and press Enter. Any
    /// other key is ignored so a stray keypress cannot dismiss the modal.
    pub(super) fn confirm_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        use KeyCode::*;
        match key.code {
            Left | Right | Char('h') | Char('l') | Tab | BackTab => {
                if let Some(c) = self.confirm.as_mut() {
                    c.yes = !c.yes;
                }
                return None;
            }
            Char('n') | Char('N') | Esc => {
                self.confirm = None;
                self.set_status("cancelled");
                return None;
            }
            Char('y') | Char('Y') => {}
            Enter => {
                if !self.confirm.as_ref().is_some_and(|c| c.yes) {
                    self.confirm = None;
                    self.set_status("cancelled");
                    return None;
                }
            }
            _ => return None,
        }
        let c = self.confirm.take()?;
        match c.action {
            ConfirmAction::DeleteConn { engine, name } => {
                let conn = self
                    .conns
                    .iter()
                    .find(|c| c.engine == engine && c.name == name)
                    .cloned();
                // The remembered tunnel goes with it, or the file fills up
                // with forwards for connections that no longer exist.
                let key = conn.as_ref().map(|c| c.key());
                match conn.map(|c| engines::delete(&c)) {
                    Some(Ok(_)) => {
                        if let Some(key) = key {
                            let _ = crate::vias::remove(&key);
                        }
                        self.refresh_conns();
                        self.set_status(format!("deleted '{name}' (file backed up first)"));
                    }
                    Some(Err(e)) => self.set_status(format!("delete failed: {e}")),
                    None => self.set_status(format!("'{name}' is already gone")),
                }
            }
            ConfirmAction::ForgetPassword(cred) => {
                let where_stored = cred.where_stored();
                match creds::delete(&cred) {
                    Ok(_) => {
                        self.refresh_creds();
                        self.set_status(format!("forgotten from {where_stored} (backed up first)"));
                    }
                    Err(e) => self.set_status(format!("could not forget it: {e}")),
                }
            }
            // The three offered fixes all end the same way: open the wizard that
            // fixes the thing, with every field already filled in.
            ConfirmAction::SavePassword { key } => {
                if let Some(conn) = self.conns.iter().find(|c| c.key() == key).cloned() {
                    self.prompt = Some(Prompt::password(&conn));
                }
            }
            ConfirmAction::RepointConn { key, local } => {
                if let Some(conn) = self.conns.iter().find(|c| c.key() == key).cloned() {
                    let mut p = Prompt::edit_conn(&conn);
                    // Pre-fill the two fields that move, so the answer is
                    // already there and Enter is the whole interaction.
                    if let Some(f) = p.fields.get_mut(1) {
                        f.value = "127.0.0.1".to_string();
                    }
                    if let Some(f) = p.fields.get_mut(2) {
                        f.value = local.clone();
                    }
                    self.prompt = Some(p);
                }
            }
            ConfirmAction::OpenTunnel { key } => {
                if let Some(conn) = self.conns.iter().find(|c| c.key() == key).cloned() {
                    self.prompt = Some(Prompt::forward(&conn, &self.settings));
                }
            }
            ConfirmAction::FixConn { key } => {
                if let Some(conn) = self.conns.iter().find(|c| c.key() == key).cloned() {
                    self.prompt = Some(Prompt::edit_conn(&conn));
                }
            }
            // The one offered fix that runs something rather than opening a
            // wizard. It goes out suspended like a session does, because sudo
            // and the package manager both need the real terminal to ask.
            ConfirmAction::InstallClient { engine } => {
                if let Some(argv) = engines::install_argv(engine) {
                    return Some(PendingRun {
                        label: shell_join(&argv),
                        argv,
                        connect: None,
                    });
                }
                self.set_status(format!(
                    "{}: {}",
                    engine.default_client(),
                    engines::install_hint(engine)
                ));
            }
        }
        None
    }

    /// The client this connection needs is not on PATH. `cargo install easysql`
    /// could never have brought it: the clients are not Rust. So rather than
    /// failing at the moment you press Enter, work out the one command this
    /// machine needs and offer to run it.
    pub(super) fn offer_install(&mut self, engine: Engine) {
        let configured = engine.client_argv(&self.settings)[0].clone();
        // Only offer to install the *default* client. Somebody who pointed the
        // setting at `pgcli` or a docker wrapper wants that, and installing
        // postgresql-client would not be what they asked for.
        if configured != engine.default_client() {
            self.set_status(format!(
                "'{configured}' is not on PATH (Settings · {} with)",
                engine.label()
            ));
            return;
        }
        let Some(argv) = engines::install_argv(engine) else {
            self.set_status(format!(
                "{configured} is not installed, and I do not recognise this machine's package manager: {}",
                engines::install_hint(engine)
            ));
            return;
        };
        self.confirm = Some(Confirm::offer(
            format!("{configured} is not installed"),
            format!(
                "easysql is a front end: it needs the real {configured} to hand the terminal to, and cargo could not have installed it. Run `{}` now? sudo will ask for your password, and the package manager will show what it is about to install.",
                shell_join(&argv)
            ),
            ConfirmAction::InstallClient { engine },
        ));
    }

    /// After a failed open: ask the server the same question non-interactively,
    /// then offer the step that would actually get you in. The client's own
    /// words lead, because they are more precise than anything we would write.
    pub(super) fn offer_fix(&mut self, conn: &Conn) {
        let Some(failure) = probe::why_failed(conn, &self.settings) else {
            return;
        };
        let key = conn.key();
        let said = failure.said;
        self.confirm = Some(match failure.fix {
            Fix::Password => Confirm::offer(
                "the server wants a password",
                format!(
                    "{said}. easysql can save one where {} looks for it, so this never asks again. Save a password now?",
                    match conn.engine {
                        Engine::Pg => "~/.pgpass",
                        _ => "~/.my.cnf",
                    }
                ),
                ConfirmAction::SavePassword { key },
            ),
            // A tunnel that already carries this exact target changes the
            // advice completely: the forward is not missing, the connection is
            // simply still pointed past it at an address that will not answer.
            // Offering to dig a second identical tunnel would be the app not
            // looking at what it already has.
            Fix::Tunnel => match self.tunnel_carrying(conn) {
                Some(local) => Confirm::offer(
                    "a tunnel to it is already open",
                    format!(
                        "{said}. There is already a forward carrying {}:{}, reachable here as localhost:{local}, but this connection still points at the far address. Repoint it now?",
                        conn.host,
                        conn.port_or_default(),
                    ),
                    ConfirmAction::RepointConn { key, local },
                ),
                None => Confirm::offer(
                    "nothing answered there",
                    format!(
                        "{said}. A database like this is usually only reachable from inside the network, through a machine that is. Open an ssh tunnel to it now?"
                    ),
                    ConfirmAction::OpenTunnel { key },
                ),
            },
            Fix::Name => Confirm::offer(
                "the server does not know that name",
                format!("{said}. Edit the connection now?"),
                ConfirmAction::FixConn { key },
            ),
        });
    }
}
