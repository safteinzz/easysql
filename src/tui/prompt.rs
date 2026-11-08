//! The wizards: a list of fields, what kind each one is, how the prompt steps
//! through them, and how it draws.

use crate::tunnels::Entry;
use ratatui::prelude::*;
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::widgets::{FORM_KEYS, REQUIRED, SEP};
use super::*;

/// One editable line in a wizard: a label in the left column, a value in the
/// right. `default` is what a blank field submits (the semantics differ per
/// action) and stands in as the dim example until something is typed.
pub(crate) struct Field {
    pub(crate) label: String,
    pub(crate) default: String,
    /// The dim example shown in the value column while the field is empty: what
    /// the field wants, not what it is called. It lives here rather than in the
    /// label so every label stays one short noun and the values line up.
    pub(crate) hint: String,
    pub(crate) value: String,
    /// The cursor in `value`, as characters after it (`line_edit::edit`).
    pub(crate) back: usize,
    pub(crate) kind: Kind,
    /// Which option a `Choice` field has selected. Unused by the other kinds.
    pub(crate) choice: usize,
    /// Marked with a red `*`, the form convention everyone already reads. Only
    /// set it on a field the submit path actually refuses to go without, or the
    /// star is a lie: everything unmarked can be left blank.
    pub(crate) required: bool,
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
            hint: String::new(),
            value: String::new(),
            back: 0,
            kind: Kind::Text,
            choice: 0,
            required: false,
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
    /// Builder: the submit path refuses a blank here, so it gets the red `*`.
    pub(super) fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Builder: the dim example shown while the field is empty.
    pub(super) fn hint(mut self, hint: &str) -> Self {
        self.hint = hint.into();
        self
    }

    /// What stands in the value column while the field is empty: what a blank
    /// submits, then what it wants typed. Both are dim, and both are gone the
    /// moment there is a value, so neither can be mistaken for one.
    pub(super) fn placeholder(&self) -> String {
        [self.default.as_str(), self.hint.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join("  ")
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
            // Two answers are a toggle, painted as chips; this is their plain
            // text, so the box is measured against what is drawn.
            Kind::Choice(options) if options.len() == 2 => {
                format!(" {}    {} ", options[0], options[1])
            }
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
        /// The SQLite file as the block names it, kept as written when the
        /// edit leaves it alone.
        database: String,
    },
    /// Write a password into the file that engine's client reads.
    SetPassword {
        engine: Engine,
        /// The connection whose group takes it (MySQL); unused for Postgres,
        /// where a `.pgpass` line stands on its own four fields.
        name: String,
    },
    /// Write a saved query. `original` is set when editing an existing one,
    /// and `sql_was` is the one line the SQL field started with, which leaves
    /// the file as it was when it comes back unchanged.
    Snippet {
        original: Option<String>,
        sql_was: String,
    },
    /// Move a `.pgpass` entry to different match fields, keeping its secret.
    /// `was` is the entry's four fields as listed, which the move checks the
    /// file still holds at `idx`.
    EditPassword {
        idx: usize,
        was: [String; 4],
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
        /// What it does, the form's one line of guidance.
        help: &'static str,
    },
}

/// A modal wizard: a titled stack of fields plus the action to run on submit.
pub(crate) struct Prompt {
    pub(crate) title: String,
    pub(crate) fields: Vec<Field>,
    pub(crate) idx: usize,
    pub(crate) action: Action,
}

pub(super) use crate::engines::pg::SSLMODES;

/// What SQL Server's certificate answer can be. Validating is first because it
/// is `sqlcmd`'s own default under ODBC driver 18, and silently waiving it would
/// be easysql weakening somebody's connection for them.
pub(super) const TRUST_CERT: [&str; 2] = ["validate", "trust it (-C)"];

/// Whether every session on this connection refuses writes. "no" is first so a
/// new connection is writable unless somebody decides otherwise.
pub(super) const READ_ONLY: [&str; 2] = ["no", "yes"];

/// The read-only toggle, starting on what the file already says.
fn read_only_field(from: Option<&Conn>) -> Field {
    Field::choice(
        "Read only",
        &READ_ONLY,
        usize::from(from.is_some_and(Conn::read_only)),
    )
}

/// The answers a connection form starts on for its choice fields, which an
/// edit compares against so only a changed one is written.
pub(super) struct Started {
    pub(super) encryption: usize,
    pub(super) certificate: usize,
}

pub(super) fn choices_of(from: Option<&Conn>) -> Started {
    let extra = |key: &str| {
        from.and_then(|c| c.extra.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)))
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    };
    Started {
        encryption: SSLMODES
            .iter()
            .position(|m| *m == extra("sslmode"))
            .unwrap_or(0),
        certificate: usize::from(extra("trust_cert") == "yes"),
    }
}

/// The database's own address for a connection: the host it names, or, for
/// one pointed at its forward, the far address that forward's record kept.
pub(super) fn far_of(c: &Conn) -> String {
    match crate::vias::get(&c.key()) {
        Some(v) if crate::vias::points_at_it(c, &v) => crate::vias::far_address(&v),
        _ => c.host.clone(),
    }
}

/// The fields that describe a connection, which differ by engine because a
/// SQLite database is a file with no host, port, user or transport to secure.
fn conn_fields(engine: Engine, from: Option<&Conn>) -> Vec<Field> {
    let get = |pick: fn(&Conn) -> &String| from.map(pick).cloned().unwrap_or_default();
    if engine == Engine::Sqlite {
        return vec![
            Field::filled("Name", &get(|c| &c.name))
                .required()
                .hint("what you type after esql"),
            Field::filled("Database file", &get(|c| &c.database)).required(),
            read_only_field(from),
        ];
    }
    let port = Field {
        default: engine.default_port().to_string(),
        ..Field::filled("Port", &get(|c| &c.port))
    };
    let mut fields = vec![
        Field::filled("Name", &get(|c| &c.name))
            .required()
            .hint("what you type after esql"),
        Field::filled("Host", &get(|c| &c.host)).hint("IP or DNS name"),
        port,
        Field::filled("Database", &get(|c| &c.database)),
        Field::filled("User", &get(|c| &c.user)),
    ];
    // Start on whatever the block already says, so an edit that does not touch
    // this field cannot change how the connection is encrypted.
    let started = choices_of(from);
    match engine {
        Engine::Pg => {
            fields.push(Field::choice("Encryption", &SSLMODES, started.encryption));
            fields.push(read_only_field(from));
        }
        Engine::MsSql => {
            fields.push(Field::choice(
                "Certificate",
                &TRUST_CERT,
                started.certificate,
            ));
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
                "add a {} connection to {}",
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
            title: format!("edit {} connection '{}'", c.engine.label(), c.name),
            idx: 0,
            action: Action::EditConn {
                engine: c.engine,
                original: c.name.clone(),
                database: c.database.clone(),
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
                    "Host",
                    if c.host.is_empty() {
                        "localhost"
                    } else {
                        &c.host
                    },
                ),
                Field::filled("Port", &c.port_or_default()),
                // A Postgres password belongs to the role, which is cluster-wide, so the same
                // secret unlocks every database on that server. Editable, because behind a
                // pooler the database name really can select a different backend.
                Field::filled("Database", "*"),
                Field::filled("User", &c.user),
                Field::secret("Password")
                    .required()
                    .hint("never shown, never in an argv"),
            ],
            _ => vec![
                Field::secret("Password")
                    .required()
                    .hint("never shown, never in an argv"),
            ],
        };
        Self {
            title: format!("save the password for '{}'", c.name),
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
        // A connection already pointed at its forward asks for 127.0.0.1 and
        // the local port, so the far end is the one its record kept.
        let recorded = crate::vias::get(&c.key()).filter(|v| crate::vias::points_at_it(c, v));
        let db_host = crate::vias::forward_target(&via, &far_of(c));
        let (db_port, local) = match &recorded {
            Some(v) => (v.port.clone(), v.local.clone()),
            None => (c.port_or_default(), String::new()),
        };
        Self {
            title: format!("reach {} through an ssh host (ssh -L)", c.name),
            idx: 0,
            action: Action::Forward { key: c.key() },
            fields: vec![
                Field::filled("Tunnel through", &via)
                    .required()
                    .hint("ctrl-o to pick · or type an ssh host"),
                Field::filled("Database host", &db_host).required(),
                Field::filled("Database port", &db_port).required(),
                Field {
                    default: "= database port".into(),
                    ..Field::filled("Local port", &local).hint("where you'll reach it")
                },
            ],
        }
    }
}

impl Prompt {
    /// Change which connection a saved password answers for, without ever
    /// asking for the password again: easysql cannot show you the one on file,
    /// but it can carry it across to the corrected line.
    pub(super) fn edit_password(cred: &crate::creds::Cred, idx: usize) -> Self {
        Self {
            title: format!("which connection is {}'s password for?", cred.user),
            idx: 0,
            action: Action::EditPassword {
                idx,
                was: [&cred.host, &cred.port, &cred.database, &cred.user].map(|f| f.to_string()),
            },
            fields: vec![
                Field::filled("Host", &cred.host),
                Field::filled("Port", &cred.port),
                Field::filled("Database", &cred.database),
                Field::filled("User", &cred.user),
            ],
        }
    }

    /// Name a saved query and give it its SQL. One line here on purpose: this
    /// is for writing a short one quickly, and anything longer is what `o` and
    /// a real editor are for.
    pub(super) fn snippet(from: Option<&crate::snippets::Snippet>) -> Self {
        let sql = from.map(|s| s.summary()).unwrap_or_default();
        Self {
            title: match from {
                Some(s) => format!("edit the snippet '{}'", s.name),
                None => "new saved query".to_string(),
            },
            idx: 0,
            action: Action::Snippet {
                original: from.map(|s| s.name.clone()),
                sql_was: sql.clone(),
            },
            fields: vec![
                Field::filled("Name", from.map(|s| s.name.as_str()).unwrap_or(""))
                    .required()
                    .hint("typed after the connection, as :name"),
                Field::filled("SQL", &sql)
                    .required()
                    .hint("one line - o opens $EDITOR for a longer one"),
            ],
        }
    }

    /// A one-field wizard for a typed setting, pre-filled with what it is now.
    pub(super) fn edit_setting(row: &settings::Row) -> Self {
        let mut field = Field::filled(row.label, &row.value);
        field.default = row.default.clone();
        Self {
            title: format!("setting: {}", row.label),
            idx: 0,
            action: Action::EditSetting {
                key: row.key.to_string(),
                label: row.label.to_string(),
                help: row.help,
            },
            fields: vec![field],
        }
    }

    /// The one line of guidance a form carries under its fields, dim, or empty
    /// for a form that needs none. It lives here rather than in a label because
    /// what it explains is true of the whole form and stays true once a field
    /// is filled in - a parenthetical in a pre-filled label is invisible where
    /// it is needed most, and four of them are a wall.
    pub(super) fn note(&self) -> &'static str {
        match &self.action {
            Action::EditSetting { help, .. } => help,
            Action::SetPassword { .. } | Action::EditPassword { .. } if self.fields.len() > 1 => {
                "* in a field matches any host, port, database or user"
            }
            Action::Forward { .. } => {
                "ctrl-o picks the ssh host · the database host is what that machine sees"
            }
            _ => "",
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
    pub(super) fn resolves_to(&self) -> Option<(String, String, String)> {
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
        Some((out, host.to_string(), port.to_string()))
    }

    /// The connection this form would save, as far as what runs is concerned:
    /// the typed fields plus the choices that change the argv or the
    /// environment. Encryption lives only in the service block, so it is left
    /// out rather than faked.
    fn draft_conn(&self) -> Option<Conn> {
        let (Action::AddConn { engine } | Action::EditConn { engine, .. }) = &self.action else {
            return None;
        };
        let v = |i: usize| self.fields[i].value.trim().to_string();
        let chose = |label: &str| self.fields.iter().any(|f| f.label == label && f.choice > 0);
        let name = v(0);
        if name.is_empty() {
            return None;
        }
        let mut extra = Vec::new();
        if *engine == Engine::MsSql && chose("Certificate") {
            extra.push(("trust_cert".to_string(), "yes".to_string()));
        }
        if chose("Read only") {
            match engine {
                Engine::Pg => crate::engines::pg::set_read_only(&mut extra, true),
                Engine::Sqlite => extra.push(("readonly".to_string(), "yes".to_string())),
                Engine::MySql | Engine::MsSql => {}
            }
        }
        // Saved absolute unless an edit left the path as the block wrote it,
        // as `submit_prompt` does.
        let file = match &self.action {
            Action::EditConn { database, .. } if *database == v(1) => v(1),
            _ => crate::engines::sqlite::absolute(&v(1)),
        };
        let (host, port, database, user) = match engine {
            Engine::Sqlite => (String::new(), String::new(), file, String::new()),
            _ => (v(1), v(2), v(3), v(4)),
        };
        Some(Conn {
            engine: *engine,
            name,
            host,
            port,
            database,
            user,
            extra,
        })
    }

    /// What the preview line is: a command that runs, or a line a file gets.
    pub(super) fn preview_label(&self) -> &'static str {
        match &self.action {
            Action::SetPassword { .. } | Action::EditPassword { .. } => "writes",
            _ => "runs",
        }
    }

    pub(super) fn command_preview(
        &self,
        settings: &crate::settings::Settings,
        conns: &[Conn],
    ) -> Option<String> {
        let v = |i: usize| self.fields[i].value.trim();
        // A blank matching field is written `*`, as `creds::set_pg` widens it.
        let w = |i: usize| match v(i) {
            "" => "*",
            value => value,
        };
        match &self.action {
            // What you will type afterwards is the point of saving it at all,
            // so the preview is the connect command, not the file we write -
            // built by `connect_argv`, the same call Enter makes.
            Action::AddConn { .. } | Action::EditConn { .. } => {
                let c = self.draft_conn()?;
                let others: Vec<Conn> = conns
                    .iter()
                    .filter(|o| match &self.action {
                        Action::EditConn {
                            engine, original, ..
                        } => !(o.engine == *engine && o.name == *original),
                        _ => true,
                    })
                    .cloned()
                    .collect();
                Some(format!(
                    "esql {}   →   {}",
                    crate::engines::typed_name(&c, &others),
                    super::widgets::with_env(
                        &c.connect_env(settings),
                        super::widgets::shell_join_display(&c.connect_argv(settings))
                    )
                ))
            }
            // The shape of the line, never its secret: the point is to teach the
            // file's format so you can read and edit it yourself afterwards.
            Action::Snippet { .. } => {
                let name = v(0);
                (!name.is_empty()).then(|| format!("esql <connection> :{name}"))
            }
            Action::EditPassword { .. } => Some(format!(
                "~/.pgpass   {}:{}:{}:{}:•••• (kept)",
                w(0),
                w(1),
                w(2),
                w(3)
            )),
            Action::SetPassword { engine, name } => Some(match engine {
                Engine::Pg => format!("~/.pgpass   {}:{}:{}:{}:••••", w(0), w(1), w(2), w(3)),
                Engine::MsSql => format!(
                    "{}   [{name}]  password=••••",
                    crate::ini::collapse_tilde(
                        &crate::engines::mssql::pass_path().to_string_lossy()
                    )
                ),
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

/// The column a wizard's values start in, measured from the label's first
/// character. Fixed rather than measured off the longest label: a form whose
/// widest label decides the column moves every value the moment that label
/// changes. A longer label simply pushes its own row rather than dragging the
/// whole column out with it.
const LABEL_COL: usize = 14;
/// How far a wide form may push that column before the labels are the ones that
/// give way, so a single long label cannot shove every value off the box.
const LABEL_COL_MAX: usize = 22;

/// The columns a label cell eats: the label, its star, and the colon.
fn label_width(field: &Field) -> usize {
    field.label.chars().count() + usize::from(field.required) + 1
}

/// Where this form's values start.
fn value_column(fields: &[Field]) -> usize {
    let widest = fields.iter().map(label_width).max().unwrap_or(0) + 2;
    widest.clamp(LABEL_COL, LABEL_COL_MAX)
}

/// The wizard box: labels in one column, values in another, and nothing that
/// appears or disappears with the cursor. The key for a choice row used to be
/// printed on whichever row was focused, which made every row grow and shrink
/// as you moved through the form to repeat what the key line already says. The
/// preview rows are reserved before there is anything to preview, so typing
/// the first letter does not resize the box, and a form taller than the screen
/// scrolls to keep the field you are on in sight, its key row fixed below.
pub(super) fn render_prompt(
    f: &mut Frame,
    area: Rect,
    p: &Prompt,
    tunnels: &[Entry],
    settings: &crate::settings::Settings,
    conns: &[Conn],
) {
    // No leading blank: the box's own top padding is that row.
    let mut lines: Vec<Line> = Vec::new();
    // Plain text of every line, kept alongside so the box can be sized against
    // what the lines wrap to rather than how many there are.
    let mut texts: Vec<String> = Vec::new();
    let dim = Style::default().add_modifier(Modifier::DIM);
    let col = value_column(&p.fields);
    for (i, field) in p.fields.iter().enumerate() {
        let active = i == p.idx;
        // Required-ness is a property of the field, not of where the cursor is,
        // so the star keeps its colour while the label around it dims.
        let star = if field.required { "*" } else { "" };
        let label_style = if active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::DIM)
        };
        let pad = " ".repeat(col.saturating_sub(label_width(field)).max(1));
        let mut spans = vec![
            Span::raw(if active { "▸ " } else { "  " }),
            Span::styled(field.label.clone(), label_style),
            Span::styled(star, Style::default().fg(Color::Red)),
            Span::styled(format!(":{pad}"), label_style),
        ];
        let value = field.display();
        // What the row really draws, cursor included, so it is measured as drawn.
        let drawn = if let Kind::Choice(options) = &field.kind
            && options.len() == 2
        {
            // A two-option toggle is the house buttons with the picked one
            // filled; guillemets are kept for a one-of-many pick.
            for (n, option) in options.iter().enumerate() {
                if n > 0 {
                    spans.push(Span::raw("  "));
                }
                spans.push(super::widgets::chip(option, n == field.choice));
            }
            value.clone()
        } else if let Kind::Choice(options) = &field.kind {
            // A cycled answer, in the colour a form uses for what it will
            // submit, between dim guillemets that say it can be stepped.
            let mut style = Style::default().fg(Color::Cyan);
            if active {
                style = style.add_modifier(Modifier::BOLD);
            }
            spans.push(Span::styled("‹ ", dim));
            spans.push(Span::styled(options[field.choice].clone(), style));
            spans.push(Span::styled(" ›", dim));
            value.clone()
        } else if value.is_empty() {
            let example = field.placeholder();
            spans.push(Span::raw(if active { "█ " } else { "" }));
            spans.push(Span::styled(example.clone(), dim));
            match active {
                true => format!("█ {example}"),
                false => example,
            }
        } else if active {
            spans.extend(line_edit::with_cursor(&value, field.back, Style::default()));
            format!("{value}█")
        } else {
            spans.push(Span::raw(value.clone()));
            value.clone()
        };
        texts.push(format!(
            "{}{}{}:{pad}{drawn}",
            if active { "▸ " } else { "  " },
            field.label,
            star,
        ));
        lines.push(Line::from(spans));
    }
    let mut keys = FORM_KEYS.to_vec();
    if p.fields.iter().any(|f| f.required) {
        keys.push(REQUIRED);
    }
    let key_row = keys.join(SEP);
    // A form with a live preview takes the widest box from the start, so the
    // command it builds fits and typing never resizes the box under you.
    let can_preview = !matches!(p.action, Action::EditSetting { .. });
    let widest = match can_preview {
        true => usize::MAX,
        false => super::widgets::widest(
            &p.title,
            texts.iter().map(String::as_str).chain([key_row.as_str()]),
        ),
    };
    // The rows above the field you are on, to scroll it into sight.
    let width = box_width(area.width, widest);
    let inner = box_inner_width(width);
    let active_at: usize = texts[..p.idx]
        .iter()
        .map(|t| wrapped_line_count(t, inner))
        .sum();
    let active_rows = wrapped_line_count(&texts[p.idx], inner);

    // One line of guidance for the whole form, where four parentheticals used
    // to sit in four labels that were pre-filled anyway.
    if !p.note().is_empty() {
        lines.push(Line::raw(""));
        texts.push(String::new());
        lines.push(Line::from(Span::styled(p.note(), dim)));
        texts.push(p.note().to_string());
    }
    // Live command preview: shows the exact command being built as you type, so
    // the wizard teaches the underlying tool instead of hiding it. A form that
    // can have one keeps its rows from the start.
    if can_preview {
        let label = format!("  {}  ", p.preview_label());
        lines.push(Line::raw(""));
        texts.push(String::new());
        match p.command_preview(settings, conns) {
            Some(cmd) => {
                texts.push(format!("{label}{cmd}"));
                lines.push(Line::from(vec![
                    Span::styled(label, dim),
                    Span::styled(cmd, Style::default().fg(Color::Green)),
                ]));
            }
            None => {
                texts.push(format!("{label}…"));
                lines.push(Line::styled(format!("{label}…"), dim));
            }
        }
    }
    // What that command opens, which is the half that moves while you edit. A
    // connection pointed at a forward is only good while the forward is alive,
    // so say which one is carrying it rather than letting that be discovered the
    // next time it is not.
    if matches!(p.action, Action::AddConn { .. } | Action::EditConn { .. })
        && !matches!(
            p.action,
            Action::AddConn {
                engine: Engine::Sqlite
            } | Action::EditConn {
                engine: Engine::Sqlite,
                ..
            }
        )
    {
        let line = match p.resolves_to() {
            Some((target, host, port)) => {
                // Only a forward that is up, on the port a loopback address
                // asks for, carries the connection: a far host is reached
                // directly whatever listens here.
                let loopback = matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1");
                let via = tunnels.iter().find_map(|t| {
                    let (open, _, _) = t.ports()?;
                    (loopback && t.on() && t.kind == 'L' && open == port).then(|| t.host.clone())
                });
                match via {
                    Some(host) => format!("        {target}   · through the tunnel to {host}"),
                    None => format!("        {target}"),
                }
            }
            None => String::new(),
        };
        texts.push(line.clone());
        lines.push(Line::from(Span::styled(line, dim)));
    }

    // Size to the *wrapped* content: a preview can be far wider than the box,
    // and counting lines instead of rows pushes the keys out through the
    // bottom border.
    let rows: usize = texts.iter().map(|t| wrapped_line_count(t, inner)).sum();
    // The body, a blank, the keys.
    let rect = box_area(area, width, box_height(rows as u16 + 2, area.height));
    f.render_widget(Clear, rect);
    let block = super::widgets::box_block(Color::Cyan, &p.title);
    let body_area = block.inner(rect);
    f.render_widget(block, rect);
    let shown = body_area.height.saturating_sub(2) as usize;
    let top = match active_at + active_rows > shown {
        true => (active_at + active_rows).saturating_sub(shown),
        false => 0,
    };
    let body = Rect {
        height: shown as u16,
        ..body_area
    };
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((top as u16, 0)),
        body,
    );
    let keys_at = Rect {
        y: body_area.y + body_area.height.saturating_sub(1),
        height: 1,
        ..body_area
    };
    f.render_widget(Paragraph::new(box_hint(&keys)), keys_at);
    super::widgets::vscrollbar(f, rect, rows, top, shown);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn the_form_preview_is_the_argv_enter_runs() {
        let s = Settings::default();
        let file = Conn {
            engine: Engine::Sqlite,
            name: "notes".into(),
            host: String::new(),
            port: String::new(),
            database: "/tmp/notes.db".into(),
            user: String::new(),
            extra: vec![("readonly".into(), "yes".into())],
        };
        let server = Conn {
            engine: Engine::MsSql,
            name: "selfsigned".into(),
            host: "db.example.com".into(),
            port: String::new(),
            database: "app".into(),
            user: "sa".into(),
            extra: vec![("trust_cert".into(), "yes".into())],
        };
        for c in [file, server] {
            let enter = super::super::widgets::shell_join_display(&c.connect_argv(&s));
            let preview = Prompt::edit_conn(&c)
                .command_preview(&s, &[])
                .expect("a named connection has a preview");
            assert!(
                preview.ends_with(&enter),
                "the preview must be what Enter runs\n preview: {preview}\n   enter: {enter}"
            );
        }
    }
}
