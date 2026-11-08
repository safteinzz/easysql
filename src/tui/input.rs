//! Key dispatch: the app-wide keys, then whichever view owns the rest.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::confirm::ConfirmAction;
use super::widgets::{shell_join, shell_join_display, with_env};
use super::*;

/// Ctrl-C, which does what Esc does under a box or in a form and quits from a view.
pub(super) fn is_ctrl_c(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
}

/// Editors offered when `$VISUAL` and `$EDITOR` name none installed here: the
/// friendlier one first, then the one every Unix has.
const FALLBACK_EDITORS: [&str; 2] = ["nano", "vi"];

impl App {
    pub(super) fn on_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        if self.show_help {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let half = 10;
            match key.code {
                _ if is_ctrl_c(key) => self.show_help = false,
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') => self.show_help = false,
                KeyCode::Char('d') if ctrl => {
                    self.help_scroll = self.help_scroll.saturating_add(half)
                }
                KeyCode::Char('u') if ctrl => {
                    self.help_scroll = self.help_scroll.saturating_sub(half)
                }
                KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(half),
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(half),
                KeyCode::Char('j') | KeyCode::Down => {
                    self.help_scroll = self.help_scroll.saturating_add(1)
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                KeyCode::Char('g') | KeyCode::Home => self.help_scroll = 0,
                // `render_help` clamps this to the last screenful.
                KeyCode::Char('G') | KeyCode::End => self.help_scroll = usize::MAX,
                _ => {}
            }
            return None;
        }
        // An alert owns every key until it is dismissed: it is there because
        // something failed and the message is the only copy of why.
        if self.alert.is_some() {
            self.alert_key(key);
            return None;
        }
        if self.confirm.is_some() {
            return self.confirm_key(key);
        }
        if self.typed.is_some() {
            return self.typed_key(key);
        }
        if self.picker.is_some() {
            return self.picker_key(key);
        }
        if self.prompt.is_some() {
            return self.prompt_key(key);
        }
        if self.searching {
            return self.search_key(key);
        }
        self.nav_key(key)
    }

    /// Typing a `/` filter. Everything printable goes into the query, so this
    /// has to run before the per-view letters; the list keeps updating under it
    /// and the arrows still move, which is what makes "type then Enter" work.
    pub(super) fn search_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        // Esc drops the filter entirely, and Ctrl-C with it, so a reflex Ctrl-C
        // steps out of the query before it can quit; Enter keeps it and hands
        // the keys back to the list, so you can search then act on what you found.
        if key.code == KeyCode::Esc || is_ctrl_c(key) {
            self.query.clear();
            self.query_back = 0;
            self.searching = false;
            self.requery();
            return None;
        }
        match key.code {
            KeyCode::Enter => self.searching = false,
            KeyCode::Down => self.move_sel(1),
            KeyCode::Up => self.move_sel(-1),
            _ => {
                if line_edit::edit(&mut self.query, &mut self.query_back, key) {
                    self.requery();
                }
            }
        }
        None
    }

    pub(super) fn nav_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // On a view Ctrl-C quits, even though `c` alone is create.
        if is_ctrl_c(key) {
            self.should_quit = true;
            return None;
        }

        // Movement - arrows and vim keys are interchangeable, and the Ctrl-chord
        // variants navigate too (so the same fingers work everywhere). Views are
        // the horizontal axis (←→ / h l / Tab); the list is the vertical (↑↓ / k j).
        match key.code {
            KeyCode::Char('q') if !ctrl => {
                self.should_quit = true;
                return None;
            }
            KeyCode::Char('?') if !ctrl => {
                self.show_help = true;
                self.help_scroll = 0;
                return None;
            }
            // `/` is the filter.
            KeyCode::Char('/') if !ctrl => {
                self.query.clear();
                self.query_back = 0;
                self.searching = true;
                self.requery();
                return None;
            }
            // Outside a search, Esc's only job is to undo one: clearing the
            // filter is the way back to the whole list.
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.requery();
                self.set_status("filter cleared");
                return None;
            }
            KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                self.cycle_view(1);
                return None;
            }
            KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') => {
                self.cycle_view(-1);
                return None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_sel(1);
                return None;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_sel(-1);
                return None;
            }
            _ => {}
        }

        // Any leftover Ctrl-chord is a navigation intent, never an action trigger.
        if ctrl {
            return None;
        }

        match self.view {
            View::Connections => self.conns_key(key),
            View::Passwords => self.passwords_key(key),
            View::Tunnels => self.tunnels_key(key),
            View::Snippets => self.snippets_key(key),
            View::Settings => self.settings_key(key),
        }
    }

    pub(super) fn conns_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        match key.code {
            KeyCode::Enter => {
                let conn = self.selected_conn()?.clone();
                self.open_conn(conn)
            }
            // `c` = create, the same key on every tab that makes something.
            KeyCode::Char('c') => {
                self.pick_engine();
                None
            }
            KeyCode::Char('e') => {
                let conn = self.selected_conn()?.clone();
                self.prompt = Some(Prompt::edit_conn(&conn));
                None
            }
            KeyCode::Char('d') => {
                let conn = self.selected_conn()?.clone();
                // What goes with it, since a saved password cannot be put back.
                let password = match conn.engine {
                    Engine::MySql => engines::mysql::has_password(&conn.name),
                    Engine::MsSql => engines::mssql::has_password(&conn.name),
                    // One written into the service block goes with the block.
                    Engine::Pg | Engine::Sqlite => conn
                        .extra
                        .iter()
                        .any(|(k, _)| engines::is_secret_key(conn.engine, k)),
                };
                let mut also = String::new();
                if password {
                    also.push_str(" Its saved password goes with it, and easysql cannot show it or put it back.");
                }
                if crate::vias::get(&conn.key()).is_some() {
                    also.push_str(match engines::forward_only_of(&conn) {
                        Some(_) => " So does its tunnel record, and the tunnel it has open stops.",
                        None => " So does its tunnel record.",
                    });
                }
                self.confirm = Some(Confirm::new(
                    "delete connection",
                    format!(
                        "Delete '{}' from {}? The database itself is untouched.{also}",
                        conn.name,
                        crate::ini::collapse_tilde(&conn.engine.store().to_string_lossy())
                    ),
                    ConfirmAction::DeleteConn {
                        engine: conn.engine,
                        name: conn.name,
                    },
                ));
                None
            }
            // `p` sets the password for the row you are on, which is the one
            // place you already know which connection you mean.
            KeyCode::Char('p') => {
                let conn = self.selected_conn()?.clone();
                match conn.engine {
                    Engine::Sqlite => {
                        self.set_status("a sqlite file has no password");
                        return None;
                    }
                    Engine::MsSql if !self.settings.mssql_passwords => {
                        self.offer_mssql_passwords(&conn);
                        return None;
                    }
                    _ => {}
                }
                self.prompt = Some(Prompt::password(&conn));
                None
            }
            KeyCode::Char('t') => {
                let conn = self.selected_conn()?.clone();
                if !conn.engine.networked() {
                    self.set_status("a sqlite file is already local; there is nothing to tunnel");
                    return None;
                }
                self.prompt = Some(Prompt::forward(&conn, &self.settings));
                None
            }
            // Yank the command, since the reason to leave the toolbox for a
            // connection is almost always to paste it into a script.
            KeyCode::Char('y') => {
                let c = self.selected_conn()?;
                let cmd = with_env(
                    &c.connect_env(&self.settings),
                    shell_join(&c.connect_argv(&self.settings)),
                );
                self.copy(cmd);
                None
            }
            // `Y` is the same yank one level louder: the URL a GUI, an ORM
            // config or a colleague's chat window wants. Never the password.
            KeyCode::Char('Y') => {
                let url = self.selected_conn()?.url();
                self.copy(url);
                None
            }
            KeyCode::Char('r') => {
                self.refresh_conns();
                self.refresh_creds();
                self.start_probes();
                self.set_status(match self.settings.probe {
                    true => "refreshed every connection file, re-checking ports",
                    false => "refreshed every connection file",
                });
                None
            }
            _ => None,
        }
    }

    pub(super) fn passwords_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        match key.code {
            KeyCode::Char('c') => {
                self.pick_password_target();
                None
            }
            // Deleting a stored password is not undoable (we never held the
            // secret to put back), so it is gated.
            KeyCode::Char('d') => {
                let cred = self.selected_cred()?.clone();
                self.confirm = Some(Confirm::new(
                    "forget password",
                    format!(
                        "Remove the password for {} from {}? easysql cannot show it or put it back.",
                        cred.user,
                        cred.where_stored()
                    ),
                    ConfirmAction::ForgetPassword(cred),
                ));
                None
            }
            // `e` moves an entry to the right host/port/database/user without
            // asking for the secret again. Only `.pgpass` has entries of its
            // own: a MySQL password lives inside its connection's group, so
            // there is nothing to edit here that is not the connection itself.
            KeyCode::Char('e') => {
                let cred = self.selected_cred()?.clone();
                match cred.source {
                    creds::Source::Pgpass(idx) => {
                        self.prompt = Some(Prompt::edit_password(&cred, idx));
                    }
                    creds::Source::MyCnf(ref name) => {
                        self.set_status(format!(
                            "this password lives in [client{name}]: edit the connection instead"
                        ));
                    }
                    // Keyed on the connection's name alone, so there is nothing
                    // to move: `p` on the connection replaces it.
                    creds::Source::MsSql(ref name) => {
                        self.set_status(format!(
                            "this password belongs to '{name}': `p` on the connection replaces it"
                        ));
                    }
                }
                None
            }
            // libpq takes the *first* matching line, so which of two overlapping entries
            // wins is decided by their order, which no wizard field can express. Backed
            // up first, since an editor is the one write path easysql does not control.
            KeyCode::Char('o') => {
                let cred = self.selected_cred()?.clone();
                let path = match cred.source {
                    creds::Source::Pgpass(_) => creds::pgpass_path(),
                    creds::Source::MyCnf(_) => engines::mysql::cnf_path(),
                    creds::Source::MsSql(_) => engines::mssql::pass_path(),
                };
                if let Err(e) = crate::ini::backup(&path) {
                    self.report_failure(
                        "not opened",
                        "could not back it up, so not opening it",
                        "",
                        &crate::commands::said(&e),
                    );
                    return None;
                }
                self.open_in_editor(&path)
            }
            KeyCode::Char('r') => {
                self.refresh_creds();
                self.set_status("refreshed ~/.pgpass and ~/.my.cnf");
                None
            }
            _ => None,
        }
    }

    pub(super) fn snippets_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        match key.code {
            KeyCode::Char('c') => {
                self.prompt = Some(Prompt::snippet(None));
                None
            }
            KeyCode::Char('e') => {
                let s = self.selected_snippet()?.clone();
                self.prompt = Some(Prompt::snippet(Some(&s)));
                None
            }
            // SQL is multi-line and a one-line field is the wrong shape for it,
            // so the real editing path is the file itself.
            KeyCode::Char('o') => {
                let s = self.selected_snippet()?.clone();
                self.open_in_editor(&s.path)
            }
            // The `.sql` file is the only copy of what was written, so the
            // delete takes its typed name rather than a Yes.
            KeyCode::Char('d') => {
                let s = self.selected_snippet()?.clone();
                self.typed = Some(typed::Typed::new(
                    "delete snippet",
                    format!(
                        "Delete '{}'? Its file goes with it, and it is the only copy.",
                        s.name
                    ),
                    s.name.clone(),
                    ConfirmAction::DeleteSnippet { name: s.name },
                ));
                None
            }
            KeyCode::Char('r') => {
                match self.refresh_snippets() {
                    Ok(true) => self.set_status(format!(
                        "refreshed · rewrote the easysql block in {}",
                        crate::ini::collapse_tilde(
                            &crate::snippets::psqlrc_path().to_string_lossy()
                        )
                    )),
                    Ok(false) => self.set_status("refreshed"),
                    Err(e) => self.report_failure(
                        "~/.psqlrc not updated",
                        "refreshed, but ~/.psqlrc was not updated",
                        "",
                        &crate::commands::said(&e),
                    ),
                }
                None
            }
            _ => None,
        }
    }

    pub(super) fn tunnels_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        match key.code {
            // A row a connection remembers can be turned back on from here: the
            // forward is the one thing between you and a database that is not
            // reachable any other way, and rebuilding it by hand is what this
            // tab exists to stop.
            KeyCode::Enter => {
                self.toggle_tunnel();
                None
            }
            // Stopping is Enter's; `d` forgets the row, stopping it first.
            KeyCode::Char('d') => {
                let t = self.selected_tunnel()?;
                let named =
                    t.owner
                        .as_ref()
                        .map(|key| match self.conns.iter().find(|c| &c.key() == key) {
                            Some(c) => format!("'{}'", c.name),
                            None => "the connection that asked for it".to_string(),
                        });
                let mut after = match (named, t.on()) {
                    (Some(name), true) => {
                        format!(
                            "It is stopped first, and {name} no longer reopens it on the way in."
                        )
                    }
                    (Some(name), false) => format!("{name} no longer reopens it on the way in."),
                    (None, _) => "Nothing remembers it, so stopping it is what deletes it.".into(),
                };
                // A forward two connections share is one row naming one of them.
                let others: Vec<String> = self
                    .vias
                    .iter()
                    .filter(|(key, v)| {
                        t.owner.as_ref() != Some(key)
                            && t.live.as_ref().is_some_and(|l| l.carries(v))
                    })
                    .map(
                        |(key, _)| match self.conns.iter().find(|c| &c.key() == key) {
                            Some(c) => format!("'{}'", c.name),
                            None => key.clone(),
                        },
                    )
                    .collect();
                if !others.is_empty() {
                    after.push_str(&format!(
                        " It carries {} too, whose sessions through it end, and which still reopens it on the way in.",
                        others.join(", ")
                    ));
                }
                self.confirm = Some(Confirm::new(
                    "delete tunnel",
                    format!(
                        "Delete the forward -{} {} through {}? {after}",
                        t.kind, t.spec, t.host
                    ),
                    ConfirmAction::DeleteTunnel {
                        owner: t.owner.clone(),
                        pid: t.pid(),
                        host: t.host.clone(),
                    },
                ));
                None
            }
            KeyCode::Char('r') => {
                self.refresh_tunnels();
                self.set_status("refreshed tunnels");
                None
            }
            _ => None,
        }
    }

    /// Enter on the Tunnels tab: off when it is up, on when it is not. Opening
    /// one is only possible for a forward something remembers, which is every
    /// row that is off.
    fn toggle_tunnel(&mut self) {
        let Some(t) = self.selected_tunnel() else {
            return;
        };
        let (kind, spec, host, pid) = (t.kind, t.spec.clone(), t.host.clone(), t.pid());
        match pid {
            Some(pid) => {
                let stopped = tunnels::kill(pid);
                self.refresh_tunnels();
                // Whatever was reachable through it is not any more.
                self.start_probes();
                match stopped {
                    Ok(true) => self.set_status(format!("stopped the tunnel through {host}")),
                    Ok(false) => {
                        self.set_status(format!("the tunnel through {host} had already stopped"))
                    }
                    Err(e) => self.report_failure(
                        "not forgotten",
                        "stopped, but not forgotten",
                        "",
                        &crate::commands::said(&e),
                    ),
                }
            }
            None => self.reopen_tunnel(kind, &spec, &host),
        }
    }

    /// Bring a forward on the Tunnels tab back up.
    pub(super) fn reopen_tunnel(&mut self, kind: char, spec: &str, host: &str) {
        match tunnels::open(kind, spec, host) {
            Ok(t) => {
                self.refresh_tunnels();
                self.start_probes();
                self.select_tunnel(spec, host);
                self.set_status(format!(
                    "reopened the tunnel through {host} (pid {})",
                    t.pid
                ));
            }
            Err(e) => {
                let owner = self
                    .tunnels
                    .iter()
                    .find(|t| t.spec == spec && t.host == host)
                    .and_then(|t| t.owner.clone());
                let retry = Retry::Reopen {
                    kind,
                    spec: spec.to_string(),
                    host: host.to_string(),
                };
                self.tunnel_failed(
                    &e,
                    owner,
                    retry,
                    &format!("could not reopen the tunnel through {host}"),
                    &tunnels::command(kind, spec, host),
                );
            }
        }
    }

    /// Open a connection, as Enter on it does: what this machine lacks for it
    /// is offered first, then its forward is brought back, then the client
    /// takes the terminal.
    pub(super) fn open_conn(&mut self, conn: Conn) -> Option<PendingRun> {
        if self.offer_missing(&conn) {
            return None;
        }
        // Bring back the forward this connection depends on before handing
        // over the terminal, or it opens onto a local port that nothing is
        // listening on any more.
        match crate::vias::ensure(&conn.key()) {
            Some(Ok(msg)) => {
                self.refresh_tunnels();
                // The port that was closed a moment ago is open now, so the dot
                // beside this connection is out of date before the session even
                // starts.
                self.start_probes();
                self.set_status(msg);
            }
            Some(Err(e)) => {
                let cmd = crate::vias::get(&conn.key())
                    .map(|v| v.command())
                    .unwrap_or_default();
                self.tunnel_failed(
                    &e,
                    Some(conn.key()),
                    Retry::Open(conn.key()),
                    &format!("could not reopen the tunnel for {}", conn.name),
                    &cmd,
                );
                return None;
            }
            None => {}
        }
        let argv = conn.connect_argv(&self.settings);
        Some(PendingRun {
            label: shell_join_display(&argv),
            argv,
            connect: Some(conn),
            then: None,
        })
    }

    /// Do again what an offered fix was for, once the fix has worked.
    pub(super) fn retry(&mut self, retry: Retry) -> Option<PendingRun> {
        match retry {
            Retry::Open(key) => {
                let conn = self.conns.iter().find(|c| c.key() == key).cloned()?;
                self.goto_view(View::Connections);
                self.select_conn(&key);
                self.open_conn(conn)
            }
            Retry::Reopen { kind, spec, host } => {
                self.reopen_tunnel(kind, &spec, &host);
                None
            }
            Retry::Submit => self.submit_prompt(),
            Retry::Copy(text) => {
                self.copy(text);
                None
            }
        }
    }

    /// Put `text` on the clipboard, offering to install a helper when none is.
    pub(super) fn copy(&mut self, text: String) {
        if !crate::clip::any_installed()
            && let Some(tool) = crate::clip::session_tool()
            && let Some(argv) = engines::install_argv_for(None, &[tool])
        {
            self.confirm = Some(Confirm::offer(
                "no clipboard tool",
                format!(
                    "Nothing here can put text on the clipboard. Run `{}` now, then copy it? sudo will ask for your password.",
                    shell_join(&argv)
                ),
                ConfirmAction::Remedy {
                    settings: Vec::new(),
                    run: Some(argv),
                    retry: Some(Retry::Copy(text)),
                },
            ));
            return;
        }
        match crate::clip::copy(&text) {
            Ok(tool) => self.set_status(format!("copied '{text}' to the clipboard ({tool})")),
            Err(e) => self.report_failure("not copied", "clipboard", "", &e),
        }
    }

    /// Open `path` in `$VISUAL` or `$EDITOR`, split like a client command so
    /// `EDITOR="code -w"` works. When neither names a program installed here,
    /// an editor that is installed is offered instead.
    pub(super) fn open_in_editor(&mut self, path: &std::path::Path) -> Option<PendingRun> {
        let editor = std::env::var("VISUAL")
            .or_else(|_| std::env::var("EDITOR"))
            .unwrap_or_default();
        let mut argv: Vec<String> = editor.split_whitespace().map(str::to_string).collect();
        let shown = crate::ini::collapse_tilde(&path.to_string_lossy());
        if argv
            .first()
            .is_some_and(|program| engines::on_path(program))
        {
            argv.push(path.to_string_lossy().into_owned());
            return Some(PendingRun {
                label: shell_join_display(&argv),
                argv,
                connect: None,
                then: None,
            });
        }
        let why = match argv.first() {
            Some(program) => {
                format!("`{program}`, which $VISUAL or $EDITOR names, is not installed")
            }
            None => "Neither $VISUAL nor $EDITOR is set".to_string(),
        };
        let Some(fallback) = FALLBACK_EDITORS.into_iter().find(|e| engines::on_path(e)) else {
            self.set_failed(format!(
                "{why}: set one to an editor installed here to open {shown}"
            ));
            return None;
        };
        self.confirm = Some(Confirm::offer(
            "no editor",
            format!("{why}. Open {shown} in {fallback} instead?"),
            ConfirmAction::Remedy {
                settings: Vec::new(),
                run: Some(vec![
                    fallback.to_string(),
                    path.to_string_lossy().into_owned(),
                ]),
                retry: None,
            },
        ));
        None
    }

    /// The Settings tab. A cycled setting changes in place on Enter; a typed one
    /// opens a one-field wizard. Every change is written to the file at once,
    /// so there is no unsaved state to lose or a save key to remember.
    pub(super) fn settings_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        match key.code {
            KeyCode::Enter | KeyCode::Char('e') => {
                let row = self.selected_setting()?;
                match row.choices {
                    Some(_) => {
                        self.settings.cycle(row.key, 1);
                        // Read by key: under a filter the row at the cursor may
                        // be another one once the value has changed.
                        let shown = self
                            .settings
                            .rows()
                            .into_iter()
                            .find(|r| r.key == row.key)
                            .map(|r| r.value)
                            .unwrap_or_default();
                        let saved = self.settings.save();
                        self.apply_settings();
                        match saved {
                            Ok(_) => self.set_status(format!("{} = {shown}", row.label)),
                            Err(e) => self.report_failure(
                                "settings not saved",
                                "could not save settings",
                                "",
                                &crate::commands::said(&e),
                            ),
                        }
                    }
                    None => self.prompt = Some(Prompt::edit_setting(&row)),
                }
                None
            }
            // `d` is "get rid of my answer", the same shape as delete elsewhere.
            // Trivially redone, so no confirm gate.
            KeyCode::Char('d') => {
                let row = self.selected_setting()?;
                self.settings.reset(row.key);
                let saved = self.settings.save();
                self.apply_settings();
                match saved {
                    Ok(_) => self.set_status(format!("{} back to {}", row.label, row.default)),
                    Err(e) => self.report_failure(
                        "settings not saved",
                        "could not save settings",
                        "",
                        &crate::commands::said(&e),
                    ),
                }
                None
            }
            KeyCode::Char('r') => {
                self.settings = crate::settings::load();
                self.settings.in_tui = true;
                self.apply_settings();
                self.set_status(format!(
                    "refreshed {}",
                    crate::ini::collapse_tilde(&crate::settings::path().to_string_lossy())
                ));
                None
            }
            _ => None,
        }
    }
}
