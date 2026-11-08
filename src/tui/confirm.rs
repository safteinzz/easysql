//! Yes/No gates in front of anything destructive, and the offered fixes that
//! follow a failed connection.
//!
//! The two are not the same modal and must not look the same. A gate guards
//! something you cannot undo, so it is red and starts on No. An offer is the
//! app volunteering the next step after the client already failed, so it is
//! cyan and starts on Yes: you asked for this by pressing Enter.

use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::probe::{self, Fix};
use super::widgets::{GATE_KEYS, shell_join};
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
    DeleteSnippet {
        name: String,
    },
    /// Forget a forward: stop it when it runs, and drop the via that would
    /// reopen it for its connection.
    DeleteTunnel {
        owner: Option<String>,
        pid: Option<u32>,
        host: String,
    },
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
    /// An offered fix: change these settings, run this command suspended (an
    /// install, so sudo and the package manager can both ask, or an ssh that
    /// asks about a host key), then, once it has worked, do `retry` again.
    Remedy {
        settings: Vec<(String, String)>,
        run: Option<Vec<String>>,
        retry: Option<Retry>,
    },
    /// Offered when a forward's local port is taken: open the tunnel form for
    /// this connection on a free one.
    MoveTunnel {
        key: String,
        local: u16,
    },
    /// Offered when a SQL Server password is asked for and easysql is not yet
    /// allowed to keep one: switch the setting on, then open the password form.
    AllowMssqlPasswords {
        key: String,
    },
}

pub(super) fn render_confirm(f: &mut Frame, area: Rect, c: &Confirm) {
    // Size the box to the wrapped message so short prompts stay small and long
    // ones (a server's own error text) are not cut off. A gate is red and starts
    // on No; an offer is cyan and starts on Yes: the colour says how much is at
    // stake, the default focus says what Enter does.
    let accent = if c.danger { Color::Red } else { Color::Cyan };
    let width = box_width(
        area.width,
        super::widgets::widest(
            &c.title,
            [
                c.message.as_str(),
                " Yes (y)    No (n) ",
                &GATE_KEYS.join(super::widgets::SEP),
            ],
        ),
    );
    let msg_rows = wrapped_line_count(&c.message, box_inner_width(width)) as u16;
    // The message, a blank, the buttons, a blank, the keys.
    let rect = box_area(area, width, box_height(msg_rows + 4, area.height));
    f.render_widget(Clear, rect);

    // One `Line` per line of the message, since a line break inside one is
    // not drawn.
    let mut lines: Vec<Line> = c
        .message
        .lines()
        .map(|l| Line::raw(l.to_string()))
        .collect();
    lines.extend([
        Line::raw(""),
        box_buttons(accent, c.yes),
        Line::raw(""),
        box_hint(GATE_KEYS),
    ]);
    let para = Paragraph::new(lines)
        .block(box_block(accent, &c.title))
        .wrap(Wrap { trim: false });
    f.render_widget(para, rect);
}

impl App {
    /// Resolve a pending yes/no. `y` proceeds and `n`/`Esc`/Ctrl-C cancels
    /// outright, or move between the buttons (`h`/`l`, the arrows, Tab) and
    /// press Enter. Any other key is ignored so a stray keypress cannot dismiss
    /// the modal.
    pub(super) fn confirm_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        use KeyCode::*;
        if super::input::is_ctrl_c(key) {
            self.confirm = None;
            self.set_status("cancelled");
            return None;
        }
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
        self.run_confirmed(c.action)
    }

    /// Carry out an answered gate, typed or not, or an accepted offer.
    pub(super) fn run_confirmed(&mut self, action: ConfirmAction) -> Option<PendingRun> {
        match action {
            ConfirmAction::DeleteConn { engine, name } => {
                let conn = self
                    .conns
                    .iter()
                    .find(|c| c.engine == engine && c.name == name)
                    .cloned();
                let done = conn.map(|c| engines::delete_with_keyed(&c));
                // Its password, its tunnel and its history may go with it, and
                // on a failure the connection itself may be gone even though
                // what it kept elsewhere was not, so every list is read again.
                self.history = crate::history::load();
                self.refresh_creds();
                self.refresh_conns();
                self.refresh_tunnels();
                self.start_probes();
                match done {
                    Some(Ok(_)) => {
                        self.set_status(format!("deleted '{name}'{}", crate::ini::backup_note()))
                    }
                    Some(Err(e)) => self.report_failure(
                        "not deleted",
                        &format!("'{name}'"),
                        "",
                        &crate::commands::said(&e),
                    ),
                    None => self.set_status(format!("'{name}' is already gone")),
                }
            }
            ConfirmAction::DeleteTunnel { owner, pid, host } => {
                if let Some(pid) = pid {
                    let _ = tunnels::kill(pid);
                }
                // A connection pointed at the forward goes back to the address
                // the record kept, which is the only place it is written down.
                let conn = owner
                    .as_ref()
                    .and_then(|key| self.conns.iter().find(|c| &c.key() == key).cloned());
                let forgot = match (&conn, owner.as_deref()) {
                    (Some(c), _) => crate::vias::forget(c),
                    (None, Some(key)) => crate::vias::remove(key).map(|_| None),
                    (None, None) => Ok(None),
                };
                self.refresh_conns();
                self.refresh_tunnels();
                // Whatever was reachable through it is not any more.
                self.start_probes();
                match forgot {
                    Err(e) => self.report_failure(
                        "not forgotten",
                        "could not forget it",
                        "",
                        &crate::commands::said(&e),
                    ),
                    Ok(Some(back)) => self.set_status(format!(
                        "deleted the tunnel through {host}, and pointed the connection back at {back}"
                    )),
                    Ok(None) => self.set_status(format!("deleted the tunnel through {host}")),
                }
            }
            ConfirmAction::DeleteSnippet { name } => match crate::snippets::delete(&name) {
                Ok(_) => match self.refresh_snippets() {
                    Ok(_) => self.set_status(format!("deleted '{name}'")),
                    Err(e) => self.report_failure(
                        "~/.psqlrc not updated",
                        &format!("deleted '{name}', but `:name` in psql was not updated"),
                        "",
                        &crate::commands::said(&e),
                    ),
                },
                Err(e) => self.report_failure(
                    "not deleted",
                    "could not delete it",
                    "",
                    &crate::commands::said(&e),
                ),
            },
            ConfirmAction::ForgetPassword(cred) => {
                let where_stored = cred.where_stored();
                match creds::delete(&cred) {
                    Ok(_) => {
                        self.refresh_creds();
                        self.set_status(format!(
                            "forgotten from {where_stored}{}",
                            crate::ini::backup_note()
                        ));
                    }
                    Err(e) if e.downcast_ref::<creds::Gone>().is_some() => self.reread_creds(&e),
                    Err(e) => self.report_failure(
                        "not forgotten",
                        "could not forget it",
                        "",
                        &crate::commands::said(&e),
                    ),
                }
            }
            // The three offered fixes all end the same way: open the wizard that
            // fixes the thing, with every field already filled in.
            ConfirmAction::SavePassword { key } => {
                if let Some(conn) = self.conns.iter().find(|c| c.key() == key).cloned() {
                    self.prompt = Some(Prompt::password(&conn));
                }
            }
            ConfirmAction::AllowMssqlPasswords { key } => {
                self.settings.set("mssql_passwords", "on");
                if let Err(e) = self.settings.save() {
                    self.report_failure(
                        "setting not saved",
                        "could not save the setting",
                        "",
                        &crate::commands::said(&e),
                    );
                    return None;
                }
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
            // A fix that runs something goes out suspended like a session
            // does, because sudo, the package manager and ssh all need the
            // real terminal to ask.
            ConfirmAction::Remedy {
                settings,
                run,
                retry,
            } => {
                if !settings.is_empty() {
                    for (key, value) in &settings {
                        self.settings.set(key, value);
                    }
                    if let Err(e) = self.settings.save() {
                        self.report_failure(
                            "setting not saved",
                            "could not save the setting",
                            "",
                            &crate::commands::said(&e),
                        );
                        return None;
                    }
                    self.apply_settings();
                }
                return match run {
                    Some(argv) => Some(PendingRun {
                        label: super::widgets::shell_join_display(&argv),
                        argv,
                        connect: None,
                        then: retry,
                    }),
                    None => retry.and_then(|r| self.retry(r)),
                };
            }
            ConfirmAction::MoveTunnel { key, local } => {
                if let Some(conn) = self.conns.iter().find(|c| c.key() == key).cloned() {
                    let mut p = Prompt::forward(&conn, &self.settings);
                    if let Some(f) = p.fields.get_mut(3) {
                        f.value = local.to_string();
                    }
                    self.prompt = Some(p);
                }
            }
        }
        None
    }

    /// SQL Server has no password file, so keeping one is a choice the user
    /// makes once: say what easysql would do, and do it on Yes.
    pub(super) fn offer_mssql_passwords(&mut self, conn: &Conn) {
        let (client, why, handed) = match conn.engine.uses_usql(&self.settings) {
            true => (
                "usql",
                "usql asks every time it opens",
                "in a passfile that lasts one session",
            ),
            false => (
                "sqlcmd",
                "sqlcmd has no password file, so it asks every time it opens",
                "in SQLCMDPASSWORD",
            ),
        };
        self.confirm = Some(Confirm::offer(
            format!("keep {client} passwords?"),
            format!(
                "{why}. easysql can keep \
                 one in {} (chmod 600) and hand it over {handed}, never on the command \
                 line where `ps` would show it. That is the one secret easysql would hold. Allow \
                 it and save a password for '{}'?",
                crate::ini::collapse_tilde(&engines::mssql::pass_path().to_string_lossy()),
                conn.name
            ),
            ConfirmAction::AllowMssqlPasswords { key: conn.key() },
        ));
    }

    /// What this machine lacks to open `conn`, offered as one fix: its client
    /// (installed, or the setting that names a missing one put back), and
    /// ssh when the forward it needs has to be reopened. `cargo install
    /// easysql` could never have brought them, since they are not Rust. A
    /// client nothing here can install is an alert naming where it comes from.
    /// Returns whether anything was missing.
    pub(super) fn offer_missing(&mut self, conn: &Conn) -> bool {
        let engine = conn.engine;
        let client_ok = self.installed[engine.idx()];
        let needs_ssh = !engines::on_path("ssh")
            && crate::vias::get(&conn.key()).is_some_and(|v| tunnels::carrying_via(&v).is_none());
        if client_ok && !needs_ssh {
            return false;
        }
        // The setting that points at a missing client goes back to the engine's
        // own, and what is still missing after that gets installed.
        let configured = engine.client_argv(&self.settings)[0].clone();
        let mut after = self.settings.clone();
        let mut settings: Vec<(String, String)> = Vec::new();
        let mut lacks: Vec<String> = Vec::new();
        if !client_ok {
            if engine.uses_usql(&self.settings) {
                settings.push(("usql_tui".into(), "off".into()));
                lacks.push(format!(
                    "`usql` is not installed, and the `usql_tui` setting opens {} connections in it",
                    engine.label()
                ));
            } else if configured != engine.default_client() {
                settings.push((
                    engine.client_setting().into(),
                    engine.default_client().into(),
                ));
                lacks.push(format!(
                    "`{configured}` is not installed, and the `{}` setting opens {} sessions with it",
                    engine.client_setting(),
                    engine.label()
                ));
            } else {
                lacks.push(format!("`{configured}` is not installed"));
            }
            for (key, value) in &settings {
                after.set(key, value);
            }
        }
        let client_missing = !client_ok && !engines::client_installed(engine, &after);
        if client_missing && engines::install_argv(engine, &after).is_none() {
            let fallback = (!settings.is_empty()).then_some(&after);
            return self.alert_not_installed(engine, &configured, fallback);
        }
        if needs_ssh {
            lacks.push(format!(
                "its tunnel needs `ssh`, which is not installed{}",
                if client_ok { "" } else { " either" }
            ));
        }
        let tools: &[engines::Tool] = if needs_ssh {
            &[engines::Tool::Ssh]
        } else {
            &[]
        };
        let run = match client_missing || needs_ssh {
            true => engines::install_argv_for(client_missing.then_some(engine), tools),
            false => None,
        };
        if (client_missing || needs_ssh) && run.is_none() {
            // Nothing here can install it: say what is missing and stop.
            self.alert(
                "missing here",
                format!(
                    "{}. easysql is a front end, and no package manager it knows is on PATH to install them.",
                    capitalized(&lacks.join(", and "))
                ),
            );
            return true;
        }
        let mut steps: Vec<String> = settings
            .iter()
            .map(|(key, value)| format!("set `{key}` to `{value}`"))
            .collect();
        if let Some(argv) = &run {
            steps.push(format!("run `{}`", shell_join(argv)));
        }
        let sudo = match run {
            Some(_) => {
                " sudo will ask for your password, and the package manager will show what it is about to install."
            }
            None => "",
        };
        let mut names: Vec<&str> = Vec::new();
        if !client_ok {
            names.push(&configured);
        }
        if needs_ssh {
            names.push("ssh");
        }
        self.confirm = Some(Confirm::offer(
            format!("{} not installed", names.join(" and ")),
            format!(
                "{}. {} now, then open {}?{sudo}",
                capitalized(&lacks.join(", and ")),
                capitalized(&steps.join(" and ")),
                conn.name
            ),
            ConfirmAction::Remedy {
                settings,
                run,
                retry: Some(Retry::Open(conn.key())),
            },
        ));
        true
    }

    /// The alert for a client that nothing here can install, `program` being
    /// the one the settings name: where it comes from instead, which only the
    /// person can go and get. `fallback` is the settings that would open it in
    /// the engine's own client, which is missing too.
    fn alert_not_installed(
        &mut self,
        engine: Engine,
        program: &str,
        fallback: Option<&crate::settings::Settings>,
    ) -> bool {
        let hint = engines::install_hint_for(engine, &self.settings, program);
        let own = fallback
            .map(|s| {
                format!(
                    "\n\nIts own client, {}, is not installed either: {}.",
                    engine.default_client(),
                    engines::install_hint(engine, s)
                )
            })
            .unwrap_or_default();
        self.alert(
            format!("{program} is not installed"),
            format!(
                "easysql is a front end: it needs the real {program} to hand the terminal to, and cargo could not have installed it.\n\n{hint}.{own}"
            ),
        );
        true
    }

    /// A forward that did not come up. Missing ssh is offered as an install,
    /// a host ssh has never seen as one interactive ssh that asks about its
    /// key, and a taken port as the tunnel form on a free one; each then
    /// retries what failed. Anything else is reported in ssh's own words.
    pub(super) fn tunnel_failed(
        &mut self,
        e: &anyhow::Error,
        owner: Option<String>,
        retry: Retry,
        said: &str,
        cmd: &str,
    ) {
        let offer = match e.downcast_ref::<tunnels::Blocked>() {
            Some(tunnels::Blocked::NoSsh) => engines::install_argv_for(None, &[engines::Tool::Ssh])
                .map(|argv| {
                    Confirm::offer(
                        "ssh is missing",
                        format!(
                            "ssh is not installed, and every tunnel runs through it. Run `{}` now, then try again? sudo will ask for your password.",
                            shell_join(&argv)
                        ),
                        ConfirmAction::Remedy {
                            settings: Vec::new(),
                            run: Some(argv),
                            retry: Some(retry),
                        },
                    )
                }),
            Some(tunnels::Blocked::UnknownHostKey { host, said: words }) => Some(Confirm::offer(
                format!("{host} is new to ssh"),
                format!(
                    "{words}\n\nssh has never seen {host}'s key, and a tunnel opened in the background cannot ask whether to trust it. Connect to {host} once now, check the fingerprint it shows and answer yes, then try again?"
                ),
                ConfirmAction::Remedy {
                    settings: Vec::new(),
                    run: Some(vec!["ssh".into(), host.clone(), "exit".into()]),
                    retry: Some(retry),
                },
            )),
            Some(tunnels::Blocked::PortTaken { port, by }) => owner
                .zip(tunnels::free_port_after(*port))
                .map(|(key, free)| {
                    let holder = match by {
                        Some(by) => format!("forwards to {by}"),
                        None => "is held by another program".to_string(),
                    };
                    Confirm::offer(
                        "port taken",
                        format!(
                            "localhost:{port} {holder}, so this tunnel cannot open there. Open the tunnel form for it on localhost:{free} instead?"
                        ),
                        ConfirmAction::MoveTunnel { key, local: free },
                    )
                }),
            None => None,
        };
        match offer {
            Some(offer) => self.confirm = Some(offer),
            None => self.report_failure("tunnel failed", said, cmd, &crate::commands::said(e)),
        }
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
            Fix::Password if conn.engine == Engine::MsSql && !self.settings.mssql_passwords => {
                return self.offer_mssql_passwords(conn);
            }
            Fix::Password => Confirm::offer(
                "the server wants a password",
                format!(
                    "{said}. easysql can save one {}, so this never asks again. Save a password now?",
                    match conn.engine {
                        Engine::Pg => "where psql looks for it, in ~/.pgpass".to_string(),
                        Engine::MsSql => format!(
                            "and hand it to {} itself",
                            conn.engine.client_name(&self.settings)
                        ),
                        _ => "where mysql looks for it, in ~/.my.cnf".to_string(),
                    }
                ),
                ConfirmAction::SavePassword { key },
            ),
            // A forward already carrying this target means nothing is missing: the
            // connection is simply still pointed past it.
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

/// `s` with its first letter in upper case, for a part that starts a sentence.
fn capitalized(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
