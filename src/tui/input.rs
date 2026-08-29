//! Key dispatch: the app-wide keys, then whichever view owns the rest.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::confirm::ConfirmAction;
use super::widgets::shell_join;
use super::*;

impl App {
    pub(super) fn on_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        if self.show_help {
            if matches!(
                key.code,
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q')
            ) {
                self.show_help = false;
            }
            return None;
        }
        if self.confirm.is_some() {
            return self.confirm_key(key);
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
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return None;
        }
        match key.code {
            // Esc drops the filter entirely; Enter keeps it and hands the keys
            // back to the list, so you can search then act on what you found.
            KeyCode::Esc => {
                self.query.clear();
                self.searching = false;
                self.requery();
            }
            KeyCode::Enter => self.searching = false,
            KeyCode::Backspace => {
                self.query.pop();
                self.requery();
            }
            KeyCode::Down => self.move_sel(1),
            KeyCode::Up => self.move_sel(-1),
            KeyCode::Char(c) if !ctrl => {
                self.query.push(c);
                self.requery();
            }
            _ => {}
        }
        None
    }

    pub(super) fn nav_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // Ctrl-C always quits, even while a per-view letter (like `c` create) is bound.
        if ctrl && key.code == KeyCode::Char('c') {
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
                return None;
            }
            // `/` is the filter, the same key it is in vim, less and man.
            KeyCode::Char('/') if !ctrl => {
                self.query.clear();
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
                // Handle it here rather than letting the spawn fail with an
                // errno: "no such file or directory" does not tell anybody to
                // install psql, let alone how.
                if !self.installed[conn.engine.idx()] {
                    self.offer_install(conn.engine);
                    return None;
                }
                // Bring back the forward this connection depends on before
                // handing over the terminal, or it opens onto a local port that
                // nothing is listening on any more.
                match crate::vias::ensure(&conn.key()) {
                    Some(Ok(msg)) => {
                        self.refresh_tunnels();
                        self.set_status(msg);
                    }
                    Some(Err(e)) => {
                        self.set_status(format!("could not reopen the tunnel: {e}"));
                        return None;
                    }
                    None => {}
                }
                let argv = conn.connect_argv(&self.settings);
                Some(PendingRun {
                    label: shell_join(&argv),
                    argv,
                    connect: Some(conn),
                })
            }
            // `c` = create, the same key in every view (the tmux convention).
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
                self.confirm = Some(Confirm::new(
                    "delete connection",
                    format!(
                        "Delete '{}' from {}? The database itself is untouched; this only forgets how to reach it.",
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
                if !conn.engine.stores_password() {
                    self.set_status(match conn.engine {
                        Engine::Sqlite => "a sqlite file has no password".to_string(),
                        _ => format!(
                            "{} has no password file to write: it asks you when it opens",
                            conn.engine.default_client()
                        ),
                    });
                    return None;
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
                let cmd = shell_join(&self.selected_conn()?.connect_argv(&self.settings));
                self.set_status(match crate::clip::copy(&cmd) {
                    Ok(tool) => format!("copied '{cmd}' to the clipboard ({tool})"),
                    Err(e) => format!("clipboard: {e}"),
                });
                None
            }
            // `Y` is the same yank one level louder: the URL a GUI, an ORM
            // config or a colleague's chat window wants. Never the password.
            KeyCode::Char('Y') => {
                let url = self.selected_conn()?.url();
                self.set_status(match crate::clip::copy(&url) {
                    Ok(tool) => format!("copied '{url}' to the clipboard ({tool})"),
                    Err(e) => format!("clipboard: {e}"),
                });
                None
            }
            KeyCode::Char('r') => {
                self.refresh_conns();
                self.refresh_creds();
                self.start_probes();
                self.set_status("reloaded every connection file, re-checking ports");
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
            KeyCode::Char('d') | KeyCode::Char('x') => {
                let cred = self.selected_cred()?.clone();
                self.confirm = Some(Confirm::new(
                    "forget password",
                    format!(
                        "Remove the password for {} from {}? easysql never read it and cannot put it back.",
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
                }
                None
            }
            // `o` opens the file itself. Not a convenience: libpq takes the
            // *first* matching line, so which of two overlapping entries wins is
            // decided by their order, and no wizard field can express that. The
            // file is backed up first, because an editor is the one write path
            // easysql does not control.
            KeyCode::Char('o') => {
                let cred = self.selected_cred()?.clone();
                let path = match cred.source {
                    creds::Source::Pgpass(_) => creds::pgpass_path(),
                    creds::Source::MyCnf(_) => engines::mysql::cnf_path(),
                };
                let editor = std::env::var("VISUAL")
                    .or_else(|_| std::env::var("EDITOR"))
                    .unwrap_or_default();
                if editor.is_empty() {
                    self.set_status("set $EDITOR (or $VISUAL) to open it here");
                    return None;
                }
                if let Err(e) = crate::ini::backup(&path) {
                    self.set_status(format!("could not back it up, so not opening it: {e}"));
                    return None;
                }
                // The same split the client commands get, so `EDITOR="code -w"`
                // works the way `psql_command="docker exec -it pg psql"` does.
                let mut argv: Vec<String> = editor.split_whitespace().map(str::to_string).collect();
                argv.push(path.to_string_lossy().into_owned());
                Some(PendingRun {
                    label: shell_join(&argv),
                    argv,
                    connect: None,
                })
            }
            KeyCode::Char('r') => {
                self.refresh_creds();
                self.set_status("reloaded ~/.pgpass and ~/.my.cnf");
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
                let editor = std::env::var("VISUAL")
                    .or_else(|_| std::env::var("EDITOR"))
                    .unwrap_or_default();
                if editor.is_empty() {
                    self.set_status("set $EDITOR (or $VISUAL) to open it here");
                    return None;
                }
                let mut argv: Vec<String> = editor.split_whitespace().map(str::to_string).collect();
                argv.push(s.path.to_string_lossy().into_owned());
                Some(PendingRun {
                    label: shell_join(&argv),
                    argv,
                    connect: None,
                })
            }
            KeyCode::Char('d') | KeyCode::Char('x') => {
                let s = self.selected_snippet()?.clone();
                self.confirm = Some(Confirm::new(
                    "delete snippet",
                    format!("Delete '{}'? The file goes with it.", s.name),
                    ConfirmAction::DeleteSnippet { name: s.name },
                ));
                None
            }
            KeyCode::Char('r') => {
                self.refresh_snippets();
                match crate::snippets::sync_psqlrc() {
                    Ok(p) => self.set_status(format!(
                        "reloaded · rewrote the easysql block in {}",
                        crate::ini::collapse_tilde(&p.to_string_lossy())
                    )),
                    Err(e) => self.set_status(format!("reloaded, but ~/.psqlrc: {e}")),
                }
                None
            }
            _ => None,
        }
    }

    pub(super) fn tunnels_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        match key.code {
            KeyCode::Char('d') | KeyCode::Char('x') => {
                let pid = self.selected_tunnel()?.pid;
                let _ = tunnels::kill(pid);
                self.refresh_tunnels();
                self.set_status(format!("killed tunnel {pid}"));
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
                        let shown = self.selected_setting().map(|r| r.value).unwrap_or_default();
                        let msg = match self.settings.save() {
                            Ok(_) => format!("{} = {shown}", row.label),
                            Err(e) => format!("could not save settings: {e}"),
                        };
                        self.apply_settings();
                        self.set_status(msg);
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
                let _ = self.settings.save();
                self.apply_settings();
                self.set_status(format!("{} back to {}", row.label, row.default));
                None
            }
            KeyCode::Char('r') => {
                self.settings = crate::settings::load();
                self.apply_settings();
                self.set_status(format!("reloaded {}", crate::settings::path().display()));
                None
            }
            _ => None,
        }
    }
}
