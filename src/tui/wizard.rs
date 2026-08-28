//! What a submitted wizard does: the command it builds and the file it writes.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::confirm::{Confirm, ConfirmAction};
use super::picker::PickerAction;
use super::*;
use crate::engines::NewConn;

impl App {
    /// Finishing the ssh-host field decides what the far end of the forward
    /// means, so the target is recomputed here rather than only when the value
    /// came from the picker: that field is typed *or* picked, and a hop typed by
    /// hand has to reach the same conclusion as one chosen from the list.
    pub(super) fn leave_field(&mut self) {
        let Some(p) = self.prompt.as_mut() else {
            return;
        };
        if !p.fields[p.idx].label.contains("ssh host") {
            return;
        }
        let via = p.fields[p.idx].value.clone();
        let Some(i) = p
            .fields
            .iter()
            .position(|f| f.label.contains("as that machine sees it"))
        else {
            return;
        };
        let was = p.fields[i].value.clone();
        let now = super::prompt::forward_target(&via, &was);
        if now != was {
            p.fields[i].value = now;
            self.set_status(format!(
                "the database is on {via} itself, so the forward points at its 127.0.0.1"
            ));
        }
    }

    pub(super) fn prompt_key(&mut self, key: KeyEvent) -> Option<PendingRun> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = self.prompt.as_ref().map(|p| p.fields.len()).unwrap_or(0);
        if len == 0 {
            return None;
        }

        // Ctrl-o fills the field from what the app already knows: the ssh hosts
        // in ~/.ssh/config for a tunnel. Typing still works for anything custom,
        // and it is a no-op on every other field.
        if ctrl && key.code == KeyCode::Char('o') {
            let p = self.prompt.as_ref().unwrap();
            let idx = p.idx;
            if p.fields[idx].label.contains("ssh host") {
                let items = crate::sshhosts::aliases();
                if items.is_empty() {
                    self.set_status("no hosts in ~/.ssh/config to tunnel through");
                } else {
                    self.picker = Some(Picker::plain(
                        "Which ssh host does the tunnel go through?",
                        items,
                        PickerAction::FillField { field: idx },
                    ));
                }
            }
            return None;
        }

        // A choice field types nothing, so the horizontal keys are free to switch
        // its answer: h/l, the arrows, and the Ctrl-chords all cycle it. You leave
        // it with Tab, Enter or the vertical keys.
        {
            let p = self.prompt.as_mut().unwrap();
            if p.fields[p.idx].is_choice() {
                let f = &mut p.fields[p.idx];
                let n = match &f.kind {
                    Kind::Choice(options) => options.len(),
                    _ => unreachable!(),
                };
                match key.code {
                    KeyCode::Right | KeyCode::Char('l') => {
                        f.choice = (f.choice + 1) % n;
                        return None;
                    }
                    KeyCode::Left | KeyCode::Char('h') => {
                        f.choice = (f.choice + n - 1) % n;
                        return None;
                    }
                    _ => {}
                }
            }
        }

        // Field navigation. Plain h/j/k/l are *typed* in a text field, so a vim
        // user moves between fields with the Ctrl-chords or the arrows/Tab -
        // exactly the "submenu" rule. Ctrl-Down/Up also carry ctrl, but their arm
        // matches on the code so they land as next/prev too.
        let next = matches!(key.code, KeyCode::Tab | KeyCode::Down)
            || (ctrl
                && matches!(
                    key.code,
                    KeyCode::Char('j') | KeyCode::Char('l') | KeyCode::Right
                ));
        let prev = matches!(key.code, KeyCode::BackTab | KeyCode::Up)
            || (ctrl && matches!(key.code, KeyCode::Char('k') | KeyCode::Left));

        if next {
            self.leave_field();
            let p = self.prompt.as_mut().unwrap();
            p.idx = p.step(1);
            return None;
        }
        if prev {
            self.leave_field();
            let p = self.prompt.as_mut().unwrap();
            p.idx = p.step(-1);
            return None;
        }

        match key.code {
            KeyCode::Esc => {
                self.prompt = None;
                self.set_status("cancelled");
            }
            // Ctrl-C bails out of the wizard rather than typing a literal 'c'.
            KeyCode::Char('c') if ctrl => {
                self.prompt = None;
                self.set_status("cancelled");
            }
            KeyCode::Enter => {
                if self.prompt.as_ref().is_some_and(|p| p.on_last_field()) {
                    return self.submit_prompt();
                }
                self.leave_field();
                let p = self.prompt.as_mut().unwrap();
                p.idx = p.step(1);
            }
            KeyCode::Backspace => {
                let f = self.prompt.as_mut().unwrap().cur_mut();
                if !f.is_choice() {
                    f.value.pop();
                }
            }
            // Everything else with no Ctrl held is literal text - including
            // h/j/k/l. A choice field holds no text, so stray keys must not
            // accumulate in it.
            KeyCode::Char(c) if !ctrl => {
                let f = self.prompt.as_mut().unwrap().cur_mut();
                if !f.is_choice() {
                    f.value.push(c);
                }
            }
            _ => {}
        }
        None
    }

    /// Re-do whatever a changed setting decides, so a new value is visible in
    /// the same frame rather than at the next launch.
    pub(super) fn apply_settings(&mut self) {
        self.sort_conns();
        self.start_probes();
        let n = self.conn_rows().len();
        Self::clamp(&mut self.conn_state, n);
    }

    /// The connection a wizard is editing, so an edit can carry through the keys
    /// the wizard never showed (`sslmode`, `connect_timeout`) instead of
    /// silently dropping a setting somebody wrote by hand.
    fn existing(&self, engine: Engine, name: &str) -> Option<&Conn> {
        self.conns
            .iter()
            .find(|c| c.engine == engine && c.name == name)
    }

    /// Consume the active prompt and carry out its action. Returns a `PendingRun`
    /// for the interactive commands; mutates in place for the rest. On a
    /// validation error it puts the prompt back so the user can fix the field.
    pub(super) fn submit_prompt(&mut self) -> Option<PendingRun> {
        let prompt = self.prompt.take()?;
        let v: Vec<String> = prompt
            .fields
            .iter()
            .map(|f| f.value.trim().to_string())
            .collect();
        // A cycled field holds no text, so its answer is an index rather than
        // anything in `v`. Field 5 is whichever extra that engine owns, and it
        // is read before the match, which moves the prompt.
        let choice_at = prompt.fields.get(5).map(|f| f.choice).unwrap_or(0);
        let action = prompt.action.clone();
        // Bound before the match consumes `action`, so both arms below can share
        // one body: an edit is an add that knows which block it is replacing.
        let original = match &action {
            Action::EditConn { original, .. } => Some(original.clone()),
            _ => None,
        };

        match action {
            Action::AddConn { engine } | Action::EditConn { engine, .. } => {
                if v[0].is_empty() {
                    self.set_status("a name is required");
                    self.prompt = Some(prompt);
                    return None;
                }
                if engine == Engine::Sqlite && v[1].is_empty() {
                    self.set_status("a database file is required");
                    self.prompt = Some(prompt);
                    return None;
                }
                // Anything the wizard did not ask about survives the rewrite.
                let mut extra = original
                    .as_deref()
                    .and_then(|o| self.existing(engine, o))
                    .map(|c| c.extra.clone())
                    .unwrap_or_default();
                // These are extras this wizard *does* ask about, so the field's
                // answer replaces whatever was in the block rather than being
                // carried through. Index 0 means the key is removed entirely.
                match engine {
                    Engine::Pg => {
                        extra.retain(|(k, _)| !k.eq_ignore_ascii_case("sslmode"));
                        if choice_at > 0 {
                            extra.push((
                                "sslmode".to_string(),
                                super::prompt::SSLMODES[choice_at].to_string(),
                            ));
                        }
                    }
                    Engine::MsSql => {
                        extra.retain(|(k, _)| !k.eq_ignore_ascii_case("trust_cert"));
                        if choice_at > 0 {
                            extra.push(("trust_cert".to_string(), "yes".to_string()));
                        }
                    }
                    _ => {}
                }
                let nc = match engine {
                    Engine::Sqlite => NewConn {
                        engine,
                        name: v[0].clone(),
                        host: String::new(),
                        port: String::new(),
                        database: v[1].clone(),
                        user: String::new(),
                        extra,
                    },
                    _ => NewConn {
                        engine,
                        name: v[0].clone(),
                        host: v[1].clone(),
                        port: v[2].clone(),
                        database: v[3].clone(),
                        user: v[4].clone(),
                        extra,
                    },
                };
                let verb = if original.is_some() {
                    "updated"
                } else {
                    "added"
                };
                match engines::save(original.as_deref(), &nc) {
                    Ok(_) => {
                        let key = format!("{}:{}", engine.slug(), v[0]);
                        // A rename changes the connection's identity, so
                        // everything keyed on it has to move too, or the tunnel
                        // it depends on is stranded under a name nothing uses.
                        if let Some(was) = original.as_deref() {
                            let old_key = format!("{}:{}", engine.slug(), was);
                            if let Err(e) = crate::vias::rename(&old_key, &key) {
                                self.set_status(format!(
                                    "saved, but the tunnel note did not move: {e}"
                                ));
                            }
                            crate::history::rename(&old_key, &key);
                        }
                        self.refresh_conns();
                        self.start_probes();
                        self.select_conn(&key);
                        self.set_status(format!(
                            "{verb} '{}' in {} (backed up first)",
                            v[0],
                            crate::ini::collapse_tilde(&engine.store().to_string_lossy())
                        ));
                    }
                    Err(e) => self.set_status(format!("{verb} failed: {e}")),
                }
                None
            }

            Action::SetPassword { engine, name } => {
                // The secret is the last field in both shapes, and it is the only
                // value in this program that is never echoed back anywhere.
                let password = v.last().cloned().unwrap_or_default();
                if password.is_empty() {
                    self.set_status("a password is required (Esc cancels)");
                    self.prompt = Some(prompt);
                    return None;
                }
                let result = match engine {
                    Engine::Pg => creds::set_pg(&v[0], &v[1], &v[2], &v[3], &password),
                    _ => engines::mysql::set_password(&name, Some(&password)),
                };
                match result {
                    Ok(_) => {
                        self.refresh_creds();
                        self.set_status(match engine {
                            Engine::Pg => format!(
                                "saved to {} (chmod 600)",
                                crate::ini::collapse_tilde(&creds::pgpass_path().to_string_lossy())
                            ),
                            _ => format!("saved into [client{name}] in ~/.my.cnf (chmod 600)"),
                        });
                    }
                    Err(e) => self.set_status(format!("could not save the password: {e}")),
                }
                None
            }

            Action::Forward { key } => {
                let (via, db_host, db_port) = (v[0].clone(), v[1].clone(), v[2].clone());
                for (value, what) in [
                    (&via, "an ssh host to tunnel through"),
                    (&db_host, "the database host"),
                    (&db_port, "the database port"),
                ] {
                    if value.is_empty() {
                        self.set_status(format!("tunnel: {what} is required"));
                        self.prompt = Some(prompt);
                        return None;
                    }
                }
                let local = if v[3].is_empty() {
                    db_port.clone()
                } else {
                    v[3].clone()
                };
                let spec = format!("{local}:{db_host}:{db_port}");
                match tunnels::open('L', &spec, &via) {
                    Ok(t) => {
                        self.goto_view(View::Tunnels);
                        self.refresh_tunnels();
                        self.select_tunnel(t.pid);
                        self.set_status(format!(
                            "localhost:{local} is now {db_host}:{db_port} via {via} (pid {})",
                            t.pid
                        ));
                        // Remember it against the connection that asked, so a
                        // reboot costs a keypress instead of an archaeology
                        // session. Recorded even if the repoint below is
                        // declined: what the tunnel *is* does not depend on
                        // whether the connection points at it yet.
                        let v = crate::vias::Via {
                            host: via.clone(),
                            target: db_host.clone(),
                            port: db_port.clone(),
                            local: local.clone(),
                        };
                        if let Err(e) = crate::vias::set(&key, &v) {
                            self.set_status(format!("tunnel open, but not remembered: {e}"));
                        }
                        // The forward exists now, so the connection that asked
                        // for it is one field from working. Say so here rather
                        // than leaving it to be discovered by failing again:
                        // the app dug this tunnel for this connection and knows
                        // exactly which address moved.
                        if let Some(conn) = self.conns.iter().find(|c| c.key() == key).cloned() {
                            self.confirm = Some(Confirm::offer(
                                "the tunnel is open",
                                format!(
                                    "localhost:{local} now reaches {}. The connection still points at {}:{}, which is what was refused. Repoint it now?",
                                    conn.name,
                                    conn.host,
                                    conn.port_or_default(),
                                ),
                                ConfirmAction::RepointConn {
                                    key: conn.key(),
                                    local: local.clone(),
                                },
                            ));
                        }
                    }
                    // ssh refused it - usually the local port is already taken.
                    // Leave the wizard open on the port that failed, so the fix
                    // is editing one number rather than starting again.
                    Err(e) => {
                        self.set_status(format!("tunnel failed: {e}"));
                        self.prompt = Some(prompt);
                    }
                }
                None
            }

            Action::EditSetting { key, label } => {
                // Blank means "back to how it ships", which is the same thing
                // deleting the line from the file does.
                let value = if v[0].is_empty() {
                    prompt.fields[0].default.clone()
                } else {
                    v[0].clone()
                };
                self.settings.set(&key, &value);
                let msg = match self.settings.save() {
                    Ok(_) => format!("{label} = {value}"),
                    Err(e) => format!("could not save settings: {e}"),
                };
                self.apply_settings();
                self.set_status(msg);
                None
            }
        }
    }
}
