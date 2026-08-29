//! The wizards: a list of fields, what kind each one is, how the prompt steps
//! through them, and how it draws.

use crate::tunnels::Tunnel;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::*;

/// One editable line in a wizard. `default` is shown in brackets and used when
/// the field is left blank on submit (the semantics differ per action).
pub(crate) struct Field {
    pub(crate) label: String,
    pub(crate) default: String,
    pub(crate) value: String,
    pub(crate) kind: Kind,
    /// Which option a `Choice` field has selected. Unused by the other kinds.
    pub(crate) choice: usize,
}

pub(crate) enum Kind {
    Text,
    /// A fixed set of answers cycled in place with `h`/`l` or the arrows. Nothing
    /// is typed here, which is what frees up plain `h`/`l` inside a wizard.
    Choice(Vec<String>),
    /// A typed value that is never painted back: the one field in easysql that
    /// carries a password. It goes straight into the file the client reads and
    /// is never echoed, never previewed and never put in an argv.
    Secret,
}

impl Field {
    pub(super) fn new(label: &str, default: &str) -> Self {
        Self {
            label: label.into(),
            default: default.into(),
            value: String::new(),
            kind: Kind::Text,
            choice: 0,
        }
    }
    /// A field that starts pre-filled with `value` - for the edit wizard.
    pub(super) fn filled(label: &str, value: &str) -> Self {
        Self {
            value: value.into(),
            ..Self::new(label, "")
        }
    }
    pub(super) fn secret(label: &str) -> Self {
        Self {
            kind: Kind::Secret,
            ..Self::new(label, "")
        }
    }
    /// A cycled answer, starting on the option at `at`.
    pub(super) fn choice(label: &str, options: &[&str], at: usize) -> Self {
        let options: Vec<String> = options.iter().map(|o| o.to_string()).collect();
        Self {
            choice: at.min(options.len().saturating_sub(1)),
            kind: Kind::Choice(options),
            ..Self::new(label, "")
        }
    }

    /// What the wizard paints for this field's value.
    pub(super) fn display(&self) -> String {
        match &self.kind {
            Kind::Text => self.value.clone(),
            Kind::Choice(options) => format!("‹ {} ›", options[self.choice]),
            Kind::Secret => "•".repeat(self.value.chars().count()),
        }
    }

    pub(super) fn is_choice(&self) -> bool {
        matches!(self.kind, Kind::Choice(_))
    }
}

/// What a wizard does once submitted. Cloned out before we move the prompt, so
/// each variant owns whatever it needs.
#[derive(Clone)]
pub(crate) enum Action {
    AddConn {
        engine: Engine,
    },
    EditConn {
        engine: Engine,
        original: String,
    },
    /// Write a password into the file that engine's client reads.
    SetPassword {
        engine: Engine,
        /// The connection whose group takes it (MySQL); unused for Postgres,
        /// where a `.pgpass` line stands on its own four fields.
        name: String,
    },
    /// Write a saved query. `original` is set when renaming an existing one.
    Snippet {
        original: Option<String>,
    },
    /// Move a `.pgpass` entry to different match fields, keeping its secret.
    EditPassword {
        idx: usize,
    },
    /// Open an `ssh -L` to a database that is not routable from here. Carries
    /// the connection it was opened for, because once the forward is up that
    /// connection is one field away from working and should not have to fail a
    /// second time to be told so.
    Forward {
        key: String,
    },
    /// Change one typed setting; cycled ones never open a wizard.
    EditSetting {
        key: String,
        label: String,
    },
}

/// A modal wizard: a titled stack of fields plus the action to run on submit.
pub(crate) struct Prompt {
    pub(crate) title: String,
    pub(crate) fields: Vec<Field>,
    pub(crate) idx: usize,
    pub(crate) action: Action,
}

/// What `sslmode` can be, in libpq's own words, with "leave it alone" first so
/// a connection that never mentioned SSL does not silently gain an opinion.
/// This is exactly the kind of value nobody remembers the spelling of, which is
/// why it is picked rather than typed.
pub(super) const SSLMODES: [&str; 6] = [
    "(unset)",
    "prefer",
    "require",
    "verify-ca",
    "verify-full",
    "disable",
];

/// What SQL Server's certificate answer can be. Validating is first because it
/// is `sqlcmd`'s own default under ODBC driver 18, and silently waiving it would
/// be easysql weakening somebody's connection for them.
pub(super) const TRUST_CERT: [&str; 2] = [
    "validate the certificate",
    "trust it (-C, for a self-signed server)",
];

/// The fields that describe a connection, which differ by engine because a
/// SQLite database is a file with no host, port, user or transport to secure.
fn conn_fields(engine: Engine, from: Option<&Conn>) -> Vec<Field> {
    let get = |pick: fn(&Conn) -> &String| from.map(pick).cloned().unwrap_or_default();
    if engine == Engine::Sqlite {
        return vec![
            Field::filled("Name (what you type after esql)", &get(|c| &c.name)),
            Field::filled("Database file", &get(|c| &c.database)),
        ];
    }
    let port = Field {
        default: engine.default_port().to_string(),
        ..Field::filled("Port", &get(|c| &c.port))
    };
    let mut fields = vec![
        Field::filled("Name (what you type after esql)", &get(|c| &c.name)),
        Field::filled("Host (IP or DNS name)", &get(|c| &c.host)),
        port,
        Field::filled("Database", &get(|c| &c.database)),
        Field::filled("User", &get(|c| &c.user)),
    ];
    // Start on whatever the block already says, so an edit that does not touch
    // this field cannot change how the connection is encrypted.
    let extra = |key: &str| {
        from.and_then(|c| c.extra.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)))
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    };
    match engine {
        Engine::Pg => {
            let at = SSLMODES
                .iter()
                .position(|m| *m == extra("sslmode"))
                .unwrap_or(0);
            fields.push(Field::choice("Encryption (sslmode)", &SSLMODES, at));
        }
        Engine::MsSql => {
            let at = usize::from(extra("trust_cert") == "yes");
            fields.push(Field::choice("Certificate", &TRUST_CERT, at));
        }
        _ => {}
    }
    fields
}

impl Prompt {
    pub(super) fn cur_mut(&mut self) -> &mut Field {
        &mut self.fields[self.idx]
    }

    pub(super) fn add_conn(engine: Engine) -> Self {
        Self {
            title: format!(
                "Add a {} connection to {}",
                engine.label(),
                crate::ini::collapse_tilde(&engine.store().to_string_lossy())
            ),
            idx: 0,
            action: Action::AddConn { engine },
            fields: conn_fields(engine, None),
        }
    }

    pub(super) fn edit_conn(c: &Conn) -> Self {
        Self {
            title: format!("Edit {} connection '{}'", c.engine.label(), c.name),
            idx: 0,
            action: Action::EditConn {
                engine: c.engine,
                original: c.name.clone(),
            },
            fields: conn_fields(c.engine, Some(c)),
        }
    }

    /// The password wizard. Postgres gets the four fields a `.pgpass` line
    /// matches on, pre-filled from the connection, because one line can be made
    /// to cover a whole cluster by widening a field to `*`. MySQL keeps its
    /// password inside the connection's own group, so there is nothing to match
    /// and nothing to ask.
    pub(super) fn password(c: &Conn) -> Self {
        let fields = match c.engine {
            Engine::Pg => vec![
                Field::filled(
                    "Host (* matches any)",
                    if c.host.is_empty() {
                        "localhost"
                    } else {
                        &c.host
                    },
                ),
                Field::filled("Port (* matches any)", &c.port_or_default()),
                // `*` even when the connection names a database, because a
                // Postgres password belongs to the *role* and roles are
                // cluster-wide: the same secret unlocks every database on that
                // server, and narrowing it here only means `\c elsewhere`
                // prompts. It stays editable, because behind a pooler
                // (pgbouncer, an RDS proxy) the database name really does
                // select a different backend with different credentials.
                Field::filled("Database (* = every database on this server)", "*"),
                Field::filled("User (* matches any)", &c.user),
                Field::secret("Password (never shown, never in an argv)"),
            ],
            _ => vec![Field::secret("Password (never shown, never in an argv)")],
        };
        Self {
            title: format!("Save the password for '{}'", c.name),
            idx: 0,
            action: Action::SetPassword {
                engine: c.engine,
                name: c.name.clone(),
            },
            fields,
        }
    }

    /// The tunnel wizard, opened against a connection so every field already
    /// knows its answer: the only thing you normally type is nothing at all.
    pub(super) fn forward(c: &Conn, s: &Settings) -> Self {
        // The setting wins when it is set, because somebody who named a bastion
        // meant it; otherwise fall back to the host this connection, or the rest
        // of the machine, already tunnels through.
        let via = match s.tunnel_host.is_empty() {
            true => crate::vias::default_host(&c.key()).unwrap_or_default(),
            false => s.tunnel_host.clone(),
        };
        let db_host = forward_target(&via, &c.host);
        Self {
            title: format!("Reach {} through an ssh host (ssh -L)", c.name),
            idx: 0,
            action: Action::Forward { key: c.key() },
            fields: vec![
                Field::filled("Tunnel through (ssh host, Ctrl-o to pick)", &via),
                Field::filled("Database host, as that machine sees it", &db_host),
                Field::filled("Database port", &c.port_or_default()),
                Field::new("Local port (where you'll reach it)", "= database port"),
            ],
        }
    }
}

/// What the far end of a `-L` should say, which is *not* always the address you
/// use from here. The host in the spec is resolved by the ssh host, so when the
/// database sits on that very machine the answer is its loopback: a server bound
/// to `127.0.0.1`, the usual reason a connection was refused in the first place,
/// will not answer on its own LAN address even from itself. Anywhere else the
/// address stays as it was, because then the ssh host really is a hop.
pub(super) fn forward_target(via: &str, db_host: &str) -> String {
    if db_host.is_empty() {
        return "localhost".to_string();
    }
    if crate::sshhosts::is_same_machine(via, db_host) {
        return "127.0.0.1".to_string();
    }
    db_host.to_string()
}

impl Prompt {
    /// Change which connection a saved password answers for, without ever
    /// asking for the password again: easysql cannot show you the one on file,
    /// but it can carry it across to the corrected line.
    pub(super) fn edit_password(cred: &crate::creds::Cred, idx: usize) -> Self {
        Self {
            title: format!("Which connection is {}'s password for?", cred.user),
            idx: 0,
            action: Action::EditPassword { idx },
            fields: vec![
                Field::filled("Host (* matches any)", &cred.host),
                Field::filled("Port (* matches any)", &cred.port),
                Field::filled(
                    "Database (* = every database on this server)",
                    &cred.database,
                ),
                Field::filled("User (* matches any)", &cred.user),
            ],
        }
    }

    /// Name a saved query and give it its SQL. One line here on purpose: this
    /// is for writing a short one quickly, and anything longer is what `o` and
    /// a real editor are for.
    pub(super) fn snippet(from: Option<&crate::snippets::Snippet>) -> Self {
        Self {
            title: match from {
                Some(s) => format!("Edit the snippet '{}'", s.name),
                None => "New saved query".to_string(),
            },
            idx: 0,
            action: Action::Snippet {
                original: from.map(|s| s.name.clone()),
            },
            fields: vec![
                Field::filled(
                    "Name (what you type after the connection, as :name)",
                    from.map(|s| s.name.as_str()).unwrap_or(""),
                ),
                Field::filled(
                    "SQL (o opens it in $EDITOR afterwards, for anything longer)",
                    &from.map(|s| s.summary()).unwrap_or_default(),
                ),
            ],
        }
    }

    /// A one-field wizard for a typed setting, pre-filled with what it is now.
    pub(super) fn edit_setting(row: &settings::Row) -> Self {
        let mut field = Field::filled(row.help, &row.value);
        field.default = row.default.clone();
        Self {
            title: format!("Setting: {}", row.label),
            idx: 0,
            action: Action::EditSetting {
                key: row.key.to_string(),
                label: row.label.to_string(),
            },
            fields: vec![field],
        }
    }

    /// The next field in `dir` (+1/-1), wrapping.
    pub(super) fn step(&self, dir: isize) -> usize {
        let len = self.fields.len() as isize;
        (self.idx as isize + dir).rem_euclid(len) as usize
    }

    /// True on the last field, so Enter submits rather than moving on.
    pub(super) fn on_last_field(&self) -> bool {
        self.idx + 1 == self.fields.len()
    }

    /// The exact command this wizard will run, or the exact line it will write,
    /// rebuilt from the current field values so it updates live as you type.
    /// Resolution mirrors `submit_prompt`, or the preview lies.
    /// What that command will actually open, spelled out from the fields as
    /// they stand. `psql "service=raspi"` is the honest argv - libpq reads the
    /// host, port, database and user out of the service file itself, and passing
    /// them again would override the file rather than describe it - but an argv
    /// that never changes while you edit looks broken. This is the line that
    /// moves, so the wizard shows both what runs and what it means.
    pub(super) fn resolves_to(&self) -> Option<(String, String)> {
        let v = |i: usize| self.fields[i].value.trim();
        let (Action::AddConn { engine } | Action::EditConn { engine, .. }) = &self.action else {
            return None;
        };
        let (host, port, db, user) = match engine {
            Engine::Sqlite => return None,
            _ => (v(1), v(2), v(3), v(4)),
        };
        if host.is_empty() {
            return None;
        }
        let port = if port.is_empty() {
            engine.default_port()
        } else {
            port
        };
        let mut out = String::new();
        if !user.is_empty() {
            out.push_str(user);
            out.push('@');
        }
        out.push_str(host);
        out.push(':');
        out.push_str(port);
        if !db.is_empty() {
            out.push('/');
            out.push_str(db);
        }
        Some((out, port.to_string()))
    }

    pub(super) fn command_preview(&self) -> Option<String> {
        let v = |i: usize| self.fields[i].value.trim();
        match &self.action {
            // What you will type afterwards is the point of saving it at all,
            // so the preview is the connect command, not the file we write.
            Action::AddConn { engine } | Action::EditConn { engine, .. } => {
                let name = v(0);
                if name.is_empty() {
                    return None;
                }
                Some(match engine {
                    Engine::Pg => format!("esql {name}   →   psql \"service={name}\""),
                    Engine::MySql => {
                        format!("esql {name}   →   mysql --defaults-group-suffix={name}")
                    }
                    Engine::Sqlite => format!("esql {name}   →   sqlite3 {}", v(1)),
                    // SQL Server has no named block to point at, so the preview
                    // is the flags themselves.
                    Engine::MsSql => {
                        let port = if v(2).is_empty() { "1433" } else { v(2) };
                        let host = if v(1).is_empty() { "localhost" } else { v(1) };
                        format!(
                            "esql {name}   →   sqlcmd -S {host},{port} -d {} -U {}",
                            v(3),
                            v(4)
                        )
                    }
                })
            }
            // The shape of the line, never its secret: the point is to teach the
            // file's format so you can read and edit it yourself afterwards.
            Action::Snippet { .. } => {
                let name = v(0);
                (!name.is_empty()).then(|| format!("esql <connection> :{name}"))
            }
            Action::EditPassword { .. } => Some(format!(
                "~/.pgpass   {}:{}:{}:{}:•••• (kept)",
                v(0),
                v(1),
                v(2),
                v(3)
            )),
            Action::SetPassword { engine, name } => Some(match engine {
                Engine::Pg => format!("~/.pgpass   {}:{}:{}:{}:••••", v(0), v(1), v(2), v(3)),
                _ => format!("~/.my.cnf   [client{name}]  password=••••"),
            }),
            Action::Forward { .. } => {
                let via = v(0);
                let db_host = v(1);
                let db_port = v(2);
                let local = if v(3).is_empty() { db_port } else { v(3) };
                if via.is_empty() {
                    return None;
                }
                Some(format!("ssh -N -L {local}:{db_host}:{db_port} {via}"))
            }
            Action::EditSetting { .. } => None,
        }
    }
}

/// Wizard box width, as the percentage of the area `centered` takes. A preview
/// line can be wider than the box, so the height has to be counted against the
/// wrapped width, not the line count.
pub(super) const PROMPT_PCT: u16 = 72;

pub(super) fn render_prompt(f: &mut Frame, area: Rect, p: &Prompt, tunnels: &[Tunnel]) {
    let mut lines: Vec<Line> = vec![Line::raw("")];
    // Plain text of every line, kept alongside so the box can be sized against
    // what the lines wrap to rather than how many there are.
    let mut texts: Vec<String> = vec![String::new()];
    for (i, field) in p.fields.iter().enumerate() {
        let active = i == p.idx;
        let head = if field.default.is_empty() {
            format!("{}: ", field.label)
        } else {
            format!("{} [{}]: ", field.label, field.default)
        };
        let label_style = if active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::DIM)
        };
        // A choice has no text cursor; it shows its options key instead, so the
        // way to change it is on screen rather than something you must know.
        let (value_style, tail) = if field.is_choice() {
            let style = if active {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            (style, if active { "   h/l or ←/→" } else { "" })
        } else {
            (Style::default(), if active { "█" } else { "" })
        };
        texts.push(format!(
            "{}{}{}{}",
            if active { "▸ " } else { "  " },
            head,
            field.display(),
            tail
        ));
        lines.push(Line::from(vec![
            Span::raw(if active { "▸ " } else { "  " }),
            Span::styled(head, label_style),
            Span::styled(field.display(), value_style),
            Span::styled(
                tail.to_string(),
                Style::default().add_modifier(Modifier::DIM),
            ),
        ]));
    }
    // Live command preview: shows the exact command being built as you type, so
    // the wizard teaches the underlying tool instead of hiding it.
    if let Some(cmd) = p.command_preview() {
        lines.push(Line::raw(""));
        texts.push(String::new());
        texts.push(format!("  runs  {cmd}"));
        lines.push(Line::from(vec![
            Span::styled("  runs  ", Style::default().add_modifier(Modifier::DIM)),
            Span::styled(cmd, Style::default().fg(Color::Green)),
        ]));
    }
    // What that command opens, which is the half that moves while you edit. A
    // connection pointed at a forward is only good while the forward is alive,
    // so say which one is carrying it rather than letting that be discovered the
    // next time it is not.
    if let Some((target, port)) = p.resolves_to() {
        let via = tunnels.iter().find_map(|t| {
            let (open, _, _) = t.ports()?;
            (t.kind == 'L' && open == port).then(|| t.host.clone())
        });
        let line = match via {
            Some(host) => format!("        {target}   · through the tunnel to {host}"),
            None => format!("        {target}"),
        };
        texts.push(line.clone());
        lines.push(Line::from(Span::styled(
            line,
            Style::default().add_modifier(Modifier::DIM),
        )));
    }
    let hint = "  Enter next/submit   Ctrl-j/k · Ctrl-↑↓ · Tab move field · Esc cancel";
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().add_modifier(Modifier::DIM),
    )));
    texts.push(String::new());
    texts.push(hint.to_string());

    // Size to the *wrapped* content: a preview can be far wider than the box,
    // and counting lines instead of rows pushes the hint out of the border.
    let inner = (area.width * PROMPT_PCT / 100).saturating_sub(2) as usize;
    let rows: usize = texts.iter().map(|t| wrapped_line_count(t, inner)).sum();
    let height = ((rows + 2) as u16).min(area.height);
    let rect = centered(area, PROMPT_PCT, height);
    f.render_widget(Clear, rect);

    let para = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", p.title))
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });
    f.render_widget(para, rect);
}
