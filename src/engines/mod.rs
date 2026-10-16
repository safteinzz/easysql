//! The seam. Above this line nothing knows which database it is about to open;
//! below it, one module per engine knows exactly one thing: which file its
//! client already reads, and which argv that client wants.
//!
//! That is the whole design rule. easysql never invents a store and never holds
//! a password: it writes the file `psql`, `mysql` or `sqlite3` reads on its own,
//! so uninstalling this tool costs you nothing and every script, GUI and
//! colleague's `psql` keeps working.

use crate::settings::Settings;
use anyhow::Result;
use std::path::PathBuf;

pub mod mssql;
pub mod mysql;
pub mod pg;
pub mod sqlite;

/// Which client owns a connection. The order here is the order a merged list
/// falls back to when nothing else separates two entries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Engine {
    Pg,
    MySql,
    Sqlite,
    MsSql,
}

pub const ENGINES: [Engine; 4] = [Engine::Pg, Engine::MySql, Engine::Sqlite, Engine::MsSql];

impl Engine {
    /// What it is called in prose and in the engine column.
    pub fn label(self) -> &'static str {
        match self {
            Engine::Pg => "postgres",
            Engine::MySql => "mysql",
            Engine::Sqlite => "sqlite",
            Engine::MsSql => "sqlserver",
        }
    }

    /// The prefix that disambiguates a name owned by two engines: `pg:prod`.
    pub fn slug(self) -> &'static str {
        match self {
            Engine::Pg => "pg",
            Engine::MySql => "my",
            Engine::Sqlite => "lite",
            Engine::MsSql => "ms",
        }
    }

    /// Position in `ENGINES`, so per-engine answers can be cached in an array
    /// rather than a map keyed on something that would have to be hashable.
    pub fn idx(self) -> usize {
        match self {
            Engine::Pg => 0,
            Engine::MySql => 1,
            Engine::Sqlite => 2,
            Engine::MsSql => 3,
        }
    }

    pub fn from_slug(s: &str) -> Option<Engine> {
        ENGINES
            .into_iter()
            .find(|e| e.slug() == s || e.label() == s)
    }

    /// The program a connect runs, as configured. A wrapper with its own
    /// arguments (`pgcli`, `docker exec -it db psql`) works because this is a
    /// whole argv, not a program name.
    pub fn client_argv(self, s: &Settings) -> Vec<String> {
        if self.uses_usql(s) {
            return vec!["usql".to_string()];
        }
        let words: Vec<String> = match self {
            Engine::Pg => &s.psql_command,
            Engine::MySql => &s.mysql_command,
            Engine::Sqlite => &s.sqlite_command,
            Engine::MsSql => &s.sqlcmd_command,
        }
        .split_whitespace()
        .map(str::to_string)
        .collect();
        if words.is_empty() {
            vec![self.default_client().to_string()]
        } else {
            words
        }
    }

    /// Whether `usql_tui` (in the TUI) or `usql_cli` (everywhere else) moves
    /// this engine's sessions and one-shots to usql, for the engines the
    /// `usql_mssql`, `usql_sqlite`, `usql_pg` and `usql_mysql` answers name.
    pub fn uses_usql(self, s: &Settings) -> bool {
        let on = match s.in_tui {
            true => s.usql_tui,
            false => s.usql_cli,
        };
        on && match self {
            Engine::MsSql => s.usql_mssql,
            Engine::Sqlite => s.usql_sqlite,
            Engine::Pg => s.usql_pg,
            Engine::MySql => s.usql_mysql,
        }
    }

    /// The program a session runs under these settings, by name, for prose.
    pub fn client_name(self, s: &Settings) -> &'static str {
        if self.uses_usql(s) {
            "usql"
        } else {
            self.default_client()
        }
    }

    /// The setting that names this engine's client.
    pub fn client_setting(self) -> &'static str {
        match self {
            Engine::Pg => "psql_command",
            Engine::MySql => "mysql_command",
            Engine::Sqlite => "sqlite_command",
            Engine::MsSql => "sqlcmd_command",
        }
    }

    pub fn default_client(self) -> &'static str {
        match self {
            Engine::Pg => "psql",
            Engine::MySql => "mysql",
            Engine::Sqlite => "sqlite3",
            Engine::MsSql => "sqlcmd",
        }
    }

    /// The port the client assumes when a connection does not name one.
    pub fn default_port(self) -> &'static str {
        match self {
            Engine::Pg => "5432",
            Engine::MySql => "3306",
            Engine::Sqlite => "",
            Engine::MsSql => "1433",
        }
    }

    /// The file this engine's connections actually live in, which is also the
    /// file its own client reads.
    pub fn store(self) -> PathBuf {
        match self {
            Engine::Pg => pg::service_path(),
            Engine::MySql => mysql::cnf_path(),
            Engine::Sqlite => sqlite::store_path(),
            Engine::MsSql => mssql::store_path(),
        }
    }

    /// How this client is handed a query to run and exit. Every one of them can
    /// do it; no two spell it the same way, which is the whole reason a snippet
    /// shortcut has to live in easysql rather than in any one client's rc file.
    /// SQLite takes it as a bare argument, so there is no flag to give.
    pub fn query_flag(self, s: &Settings) -> Option<&'static str> {
        if self.uses_usql(s) {
            return Some("-c");
        }
        match self {
            Engine::Pg => Some("-c"),
            Engine::MySql => Some("-e"),
            Engine::Sqlite => None,
            Engine::MsSql => Some("-Q"),
        }
    }

    /// What `esql --md` appends to the argv so the client prints rows that can
    /// be read back, and the shape they come in. psql's is a `-c` rather than
    /// `--csv` because `~/.psqlrc` runs after the flags and a `\pset format`
    /// there beats them; `-q` keeps a script's `SET` tags out of the rows.
    /// sqlcmd has none: its `-s,` does not quote a comma inside a value, which
    /// usql's `-C` does.
    pub fn table_output(
        self,
        s: &Settings,
    ) -> Option<(&'static [&'static str], crate::markdown::Rows)> {
        use crate::markdown::Rows;
        if self.uses_usql(s) {
            return Some((&["-C"], Rows::Csv));
        }
        match self {
            Engine::Pg => Some((&["-q", "-c", "\\pset format csv"], Rows::Csv)),
            Engine::MySql => Some((&["--batch"], Rows::Tsv)),
            Engine::Sqlite => Some((&["-csv", "-header"], Rows::Csv)),
            Engine::MsSql => None,
        }
    }

    /// The program `esql --dump` runs, which reads the same file the client
    /// does. sqlcmd ships no dump tool, so SQL Server has none.
    pub fn dump_client(self) -> Option<&'static str> {
        match self {
            Engine::Pg => Some("pg_dump"),
            Engine::MySql => Some("mysqldump"),
            Engine::Sqlite => Some("sqlite3"),
            Engine::MsSql => None,
        }
    }

    /// The one line of orientation printed before the terminal is handed over.
    ///
    /// Ordered the way you actually move through a server - databases, then
    /// schemas, then tables, then one object - because that is the sequence
    /// somebody who has forgotten the client is trying to walk. Recall is the
    /// gap here; none of these clients is missing a feature, they just spell
    /// "list the tables" four different ways.
    pub fn hint(self) -> &'static str {
        match self {
            // Angle brackets mark what you type *after* the command, so `\c <db>`
            // reads as "connect to a database" rather than as a command called
            // "name". Running a file is in here because it is the one people
            // reach for and cannot guess: every client spells it differently.
            Engine::Pg => {
                "\\l dbs · \\c <db> · \\dn schemas · \\dt tables · \\d <table> · \\i <file> · \\? help · \\q quit"
            }
            Engine::MySql => {
                "show databases; · use <db>; · show tables; · desc <table>; · source <file> · \\q quit"
            }
            Engine::Sqlite => {
                ".databases · .tables · .schema <table> · .read <file> · .help · .quit"
            }
            // GO leads because nothing typed at sqlcmd's prompt runs until it:
            // a `select` just sits at `2>`, which reads as a hang to anyone who
            // came from psql.
            Engine::MsSql => {
                "GO runs what you typed · sp_databases dbs · use <db> · sp_tables tables · sp_help <table> · :r <file> · exit"
            }
        }
    }

    /// `hint` for a session that opens in usql. `\c <db>` is named only when
    /// `by_name`, because usql's `\c` takes a whole URL unless the database was
    /// named at launch (`mssql::usql_csets`), and a `use <db>` lasts only as
    /// long as the pooled connection it ran on. `;` leads for SQL Server, where
    /// `GO` runs nothing in usql.
    pub fn usql_hint(self, by_name: bool) -> &'static str {
        match (self, by_name) {
            (Engine::Sqlite, _) => "\\dt tables · \\d <table> · \\i <file> · \\? help · \\q quit",
            (_, true) => {
                "; runs what you typed · \\l dbs · \\c <db> · \\dn schemas · \\dt tables · \\d <table> · \\i <file> · \\? help · \\q quit"
            }
            (_, false) => {
                "; runs what you typed · \\l dbs · \\dn schemas · \\dt tables · \\d <table> · \\i <file> · \\? help · \\q quit"
            }
        }
    }

    /// True when this engine has a server to reach, and therefore a port to
    /// probe and a tunnel worth opening. SQLite is a file.
    pub fn networked(self) -> bool {
        self != Engine::Sqlite
    }

    /// True when a connection can be made read-only from its form. MySQL is
    /// not, yet: its client honours `init-command` in the `[clientNAME]` group,
    /// but `mysqldump` reads that group too and refuses to start over the
    /// unknown key, and SQL Server has no session setting that enforces it.
    pub fn offers_read_only(self) -> bool {
        matches!(self, Engine::Pg | Engine::Sqlite)
    }

    /// True when easysql can save this engine's password into a file its client
    /// reads on its own. SQL Server has no such file - the modern `sqlcmd`
    /// keeps an obfuscated copy in its own YAML and the classic one reads
    /// nothing - and `SQLCMDPASSWORD` would mean holding the secret and handing
    /// it to a child, which this crate does not do. So sqlcmd prompts instead,
    /// which is also Microsoft's own advice.
    pub fn stores_password(self) -> bool {
        matches!(self, Engine::Pg | Engine::MySql)
    }

    /// Whether a password for this engine can be saved at all under these
    /// settings: always where the client has a file of its own, and for SQL
    /// Server only once the user has allowed easysql to keep it.
    pub fn keeps_password(self, s: &Settings) -> bool {
        self.stores_password() || (self == Engine::MsSql && s.mssql_passwords)
    }
}

/// One saved connection, whichever file it came out of.
#[derive(Clone)]
pub struct Conn {
    pub engine: Engine,
    /// The service name / login group / label: what you type after `esql`.
    pub name: String,
    pub host: String,
    pub port: String,
    /// The database, or the file path for SQLite.
    pub database: String,
    pub user: String,
    /// Every other key in the block (`sslmode`, `connect_timeout`, ...), carried
    /// through untouched so a hand-written setting survives an edit here.
    pub extra: Vec<(String, String)>,
}

impl Conn {
    /// The unique handle for history and selection: two engines are allowed to
    /// both call a connection `prod`.
    pub fn key(&self) -> String {
        format!("{}:{}", self.engine.slug(), self.name)
    }

    /// `user@host:port/db`, the second column and the `ls -v` line. SQLite has
    /// no server, so it shows the file instead.
    pub fn target(&self) -> String {
        if self.engine == Engine::Sqlite {
            return crate::ini::collapse_tilde(&self.database);
        }
        let host = if self.host.is_empty() {
            "localhost".to_string()
        } else {
            self.host.clone()
        };
        let mut s = match self.user.is_empty() {
            true => host,
            false => format!("{}@{host}", self.user),
        };
        let port = self.port_or_default();
        if !port.is_empty() {
            s.push(':');
            s.push_str(&port);
        }
        if !self.database.is_empty() {
            s.push('/');
            s.push_str(&self.database);
        }
        s
    }

    /// Whether the block makes every session refuse writes, read from the file
    /// rather than remembered, so a line written by hand counts too.
    pub fn read_only(&self) -> bool {
        match self.engine {
            Engine::Pg => pg::read_only(&self.extra),
            Engine::Sqlite => sqlite::read_only(&self.extra),
            Engine::MySql | Engine::MsSql => false,
        }
    }

    /// What a session with this connection needs in its environment on top of
    /// the argv, so every launcher applies the same thing. A read-only
    /// Postgres connection opened in a client other than psql gets PGOPTIONS:
    /// psql applies the service's `options` itself, but a client that reads
    /// the service file on its own can drop them, as pgcli does. A usql
    /// session gets its pager (`usql_pager_env`).
    pub fn connect_env(&self, s: &Settings) -> Vec<(String, String)> {
        match self.engine {
            Engine::Pg if self.read_only() && !speaks_client_flags(self.engine, s) => {
                vec![("PGOPTIONS".to_string(), pg::read_only_pgoptions())]
            }
            _ if self.engine.uses_usql(s) => usql_pager_env(),
            _ => Vec::new(),
        }
    }

    /// The part of a session's environment that is a secret: only a SQL Server
    /// password easysql was allowed to keep, in `SQLCMDPASSWORD` for sqlcmd or
    /// as a passfile of its own for usql. Applied by the two launchers and the
    /// probe, and never by anything that shows or copies a command, which is
    /// why it is not in `connect_env`. An error means the passfile could not
    /// be written.
    pub fn secret_env(&self, s: &Settings) -> Result<Secret> {
        match self.engine {
            Engine::MsSql if s.mssql_passwords && self.engine.uses_usql(s) => {
                let file = mssql::usql_passfile(self)?;
                let env = file
                    .iter()
                    .map(|f| {
                        (
                            "USQLPASS".to_string(),
                            f.path().to_string_lossy().into_owned(),
                        )
                    })
                    .collect();
                Ok(Secret {
                    env,
                    files: file.into_iter().collect(),
                })
            }
            Engine::MySql if self.engine.uses_usql(s) => {
                let mut secret = Secret::plain(Vec::new());
                if let Some(file) = mysql::usql_passfile(self)? {
                    secret.hold("USQLPASS", file);
                }
                Ok(secret)
            }
            Engine::MsSql if s.mssql_passwords => Ok(Secret::plain(
                mssql::password(&self.name)
                    .map(|p| vec![("SQLCMDPASSWORD".to_string(), p)])
                    .unwrap_or_default(),
            )),
            _ => Ok(Secret::plain(Vec::new())),
        }
    }

    /// What a usql run needs beyond `connect_argv` and `secret_env`, which
    /// neither can do because the preview and `y` call them: for a `session`,
    /// the `--cset` flags that make `\c <db>` reach each database by name, and
    /// for Postgres, a copy of the service block aimed at `db` (and at each of
    /// those databases), held in `secret` and named in `PGSERVICEFILE`,
    /// because usql's driver lets the service file win over the URL's dbname.
    /// Asks the server for its databases, so it runs only on the way in.
    pub fn usql_session(
        &self,
        s: &Settings,
        db: Option<&str>,
        session: bool,
        secret: &mut Secret,
    ) -> Result<Vec<String>> {
        if !self.engine.uses_usql(s) {
            return Ok(Vec::new());
        }
        match self.engine {
            Engine::MsSql if session => Ok(mssql::usql_csets(self, s, db, &secret.env)),
            Engine::MySql if session => Ok(mysql::usql_csets(self, s, db, &secret.env)),
            Engine::Pg if session || db.is_some() => {
                let dbs = match session {
                    true => pg::usql_databases(self, s, &secret.env),
                    false => Vec::new(),
                };
                let file = RunFile::write(&pg::usql_service_file(self, db, &dbs))?;
                secret.hold("PGSERVICEFILE", file);
                Ok(pg::usql_csets(&dbs))
            }
            _ => Ok(Vec::new()),
        }
    }

    pub fn port_or_default(&self) -> String {
        if self.port.is_empty() {
            self.engine.default_port().to_string()
        } else {
            self.port.clone()
        }
    }

    /// The exact command Enter runs. Every engine points its client at the
    /// block by name, so no host, user or password is ever on the command line.
    pub fn connect_argv(&self, s: &Settings) -> Vec<String> {
        self.connect_argv_db(s, None)
    }

    /// The same command aimed at another database on the same server, which is
    /// what `esql <name>/<db>` asks for.
    ///
    /// It has to be built here rather than passed through, because each client
    /// spells it in a way that collides with the argument carrying the
    /// connection: psql's `-d` and its bare positional both fill libpq's
    /// dbname and username slots, so `service=` is demoted to a username and
    /// the host, port and user are lost. SQLite has no database to switch to -
    /// another file is another connection - so `db` is refused by the caller
    /// and ignored here.
    pub fn connect_argv_db(&self, s: &Settings, db: Option<&str>) -> Vec<String> {
        let mut argv = self.engine.client_argv(s);
        match self.engine {
            Engine::MsSql if self.engine.uses_usql(s) => argv.extend(mssql::usql_args(self, s, db)),
            Engine::Sqlite if self.engine.uses_usql(s) => {
                argv.push(sqlite::usql_url(&self.database, self.read_only()))
            }
            // `db` reaches usql through the service file `usql_session` writes.
            Engine::Pg if self.engine.uses_usql(s) => argv.push(pg::usql_url(&self.name)),
            Engine::MySql if self.engine.uses_usql(s) => argv.push(mysql::usql_url(self, db)),
            Engine::Pg => argv.push(pg::conninfo(&self.name, db)),
            Engine::MySql => {
                argv.push(format!("--defaults-group-suffix={}", self.name));
                if let Some(db) = db {
                    argv.push(format!("--database={db}"));
                }
            }
            Engine::Sqlite => {
                if self.read_only() {
                    // `-readonly` is sqlite3's own flag and litecli rejects it,
                    // so a read-only connection runs with sqlite3 itself rather
                    // than open writable or not at all.
                    if !speaks_client_flags(self.engine, s) {
                        argv = vec![self.engine.default_client().to_string()];
                    }
                    argv.push("-readonly".into());
                }
                argv.push(
                    crate::ini::expand_tilde(&self.database)
                        .to_string_lossy()
                        .into_owned(),
                )
            }
            Engine::MsSql => argv.extend(mssql::flags(self, db)),
        }
        argv
    }

    /// What `esql --dump` runs: `Engine::dump_client` aimed at the same block
    /// the client reads, with `args` handed to it verbatim. SQLite's dump is a
    /// dot-command, so there `args` become the words of `.dump` (tables,
    /// `--data-only`), and the file is opened `-readonly` so a wrong path is an
    /// error rather than a new empty database. `None` for SQL Server.
    pub fn dump_argv(&self, db: Option<&str>, args: &[String]) -> Option<Vec<String>> {
        let mut argv = vec![self.engine.dump_client()?.to_string()];
        match self.engine {
            Engine::Pg => argv.push(pg::conninfo(&self.name, db)),
            Engine::MySql => {
                argv.push(format!("--defaults-group-suffix={}", self.name));
                // mysqldump takes the database as a positional, and these flags
                // name their own instead.
                let names_its_own = args
                    .iter()
                    .any(|a| matches!(a.as_str(), "-A" | "--all-databases" | "-B" | "--databases"));
                let db = db.unwrap_or(&self.database);
                if !db.is_empty() && !names_its_own {
                    argv.push(db.to_string());
                }
            }
            Engine::Sqlite => {
                argv.push("-readonly".into());
                argv.push(
                    crate::ini::expand_tilde(&self.database)
                        .to_string_lossy()
                        .into_owned(),
                );
                let mut dot = String::from(".dump");
                for a in args {
                    dot.push(' ');
                    dot.push_str(&sqlite::dot_arg(a));
                }
                argv.push(dot);
                return Some(argv);
            }
            Engine::MsSql => return None,
        }
        argv.extend(args.iter().cloned());
        Some(argv)
    }

    /// A URL for the connection, which is what a GUI, an ORM config or a
    /// colleague's chat window wants. Never carries the password.
    pub fn url(&self) -> String {
        match self.engine {
            Engine::Sqlite => format!("sqlite://{}", self.database),
            _ => {
                let scheme = match self.engine {
                    Engine::Pg => "postgresql",
                    Engine::MsSql => "sqlserver",
                    _ => "mysql",
                };
                let host = if self.host.is_empty() {
                    "localhost"
                } else {
                    &self.host
                };
                let user = match self.user.is_empty() {
                    true => String::new(),
                    false => format!("{}@", self.user),
                };
                let db = match self.database.is_empty() {
                    true => String::new(),
                    false => format!("/{}", self.database),
                };
                format!("{scheme}://{user}{host}:{}{db}", self.port_or_default())
            }
        }
    }
}

/// What `Conn::secret_env` hands a launcher: the variables, and the passfile
/// one of them may name, which is deleted when this is dropped. Hold it until
/// the child has exited, and drop it before `process::exit`, which runs no
/// destructors.
pub struct Secret {
    pub env: Vec<(String, String)>,
    files: Vec<RunFile>,
}

impl Secret {
    fn plain(env: Vec<(String, String)>) -> Self {
        Self {
            env,
            files: Vec::new(),
        }
    }

    /// Keep `file` alive for the run and name it to the child in `key`.
    fn hold(&mut self, key: &str, file: RunFile) {
        self.env
            .push((key.to_string(), file.path().to_string_lossy().into_owned()));
        self.files.push(file);
    }

    /// Whether a file has to outlive the child, so the CLI cannot `exec`.
    pub fn holds_file(&self) -> bool {
        !self.files.is_empty()
    }
}

/// A private file easysql wrote for one usql run (a passfile, a service file),
/// deleted when dropped.
pub struct RunFile(PathBuf);

impl RunFile {
    pub fn path(&self) -> &std::path::Path {
        &self.0
    }

    /// Write `contents` to a fresh 0600 file in the first of `run_file_dirs`
    /// that will take one. A file left by an esql that was killed before it
    /// could delete its own is removed on the way.
    pub fn write(contents: &str) -> Result<RunFile> {
        let mut first_error = None;
        for dir in run_file_dirs() {
            match Self::write_in(&dir, contents) {
                Ok(file) => return Ok(file),
                Err(e) => {
                    first_error.get_or_insert(e);
                }
            }
        }
        Err(first_error.unwrap_or_else(|| anyhow::anyhow!("no folder to write it in")))
    }

    fn write_in(dir: &std::path::Path, contents: &str) -> Result<RunFile> {
        use anyhow::Context;
        use std::io::Write;
        let dir = dir.to_path_buf();
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            builder.mode(0o700);
            builder
                .create(&dir)
                .with_context(|| format!("creating {}", dir.display()))?;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .with_context(|| format!("making {} private", dir.display()))?;
        }
        #[cfg(not(unix))]
        builder
            .create(&dir)
            .with_context(|| format!("creating {}", dir.display()))?;
        sweep_run_files(&dir);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let path = dir.join(format!("{}-{stamp}", std::process::id()));
        let mut open = std::fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.mode(0o600);
        }
        let mut f = open
            .open(&path)
            .with_context(|| format!("creating {}", path.display()))?;
        let file = RunFile(path);
        f.write_all(contents.as_bytes())
            .with_context(|| format!("writing {}", file.path().display()))?;
        Ok(file)
    }
}

impl Drop for RunFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Where run files may go, best first: `$XDG_RUNTIME_DIR`, which is the
/// user's alone and cleared at logout, then the state dir, each in a folder
/// only the owner can enter.
fn run_file_dirs() -> Vec<PathBuf> {
    let state = dirs::state_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state"));
    dirs::runtime_dir()
        .into_iter()
        .chain([state])
        .map(|d| d.join("easysql").join("pass"))
        .collect()
}

/// Remove the run files whose esql is gone, named `<pid>-<stamp>`. Only where
/// `/proc` says which pids are alive, or every other run's file would look dead.
fn sweep_run_files(dir: &std::path::Path) {
    if !std::path::Path::new("/proc/self").exists() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some((pid, _)) = name.to_str().and_then(|n| n.split_once('-')) else {
            continue;
        };
        if pid.parse::<u32>().is_ok() && !std::path::Path::new("/proc").join(pid).exists() {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// usql pages with plain `less` when nothing names a pager, which wraps a wide
/// table and opens even for one row. It runs `USQL_PAGER` or `PAGER` as one
/// program name and never splits it (`less -S` is "executable file not
/// found"), so the flags travel in `LESS`: `-S` scrolls sideways, `-F` prints
/// a result that fits without opening the pager. A pager the user named is
/// left alone.
fn usql_pager_env() -> Vec<(String, String)> {
    let set = |k: &str| std::env::var_os(k).is_some();
    if set("USQL_PAGER") || set("PAGER") || !on_path("less") {
        return Vec::new();
    }
    let mut env = vec![("USQL_PAGER".to_string(), "less".to_string())];
    if !set("LESS") {
        env.push(("LESS".to_string(), "-FRS".to_string()));
    }
    env
}

/// Whether usql takes `n` as a `--cset` name. It aborts at startup over any
/// other, so a database named otherwise is left out of `\c` by name.
pub fn is_cset_name(n: &str) -> bool {
    !n.is_empty() && n.chars().all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

/// `s` percent-encoded for a URL, keeping only the unreserved characters and
/// those in `keep`.
pub fn percent_encode(s: &str, keep: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || "-._~".contains(c) || keep.contains(c) {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// What a wizard collects. Empty strings mean "not set" and are left out of the
/// written block rather than written empty.
pub struct NewConn {
    pub engine: Engine,
    pub name: String,
    pub host: String,
    pub port: String,
    pub database: String,
    pub user: String,
    /// Keys we did not ask about, carried over from the block being edited.
    pub extra: Vec<(String, String)>,
}

/// Every saved connection, from every engine, sorted by engine then name. The
/// TUI re-sorts by history; `esql ls` keeps this order, because a script's
/// output must not depend on what you did yesterday.
pub fn list() -> Vec<Conn> {
    let mut all = pg::list();
    all.extend(mysql::list());
    all.extend(sqlite::list());
    all.extend(mssql::list());
    // By engine in the order `ENGINES` declares, then by name. Sorting on the
    // slug instead would order them by an abbreviation nobody asked to read.
    all.sort_by(|a, b| (a.engine.idx(), a.name.as_str()).cmp(&(b.engine.idx(), b.name.as_str())));
    all
}

/// Look a name up the way the CLI does: bare `prod`, or `pg:prod` when two
/// engines both have one. An ambiguous bare name is an error, never a guess.
fn find_among(all: Vec<Conn>, needle: &str) -> Result<Conn> {
    if let Some((slug, name)) = needle.split_once(':')
        && let Some(engine) = Engine::from_slug(slug)
    {
        return all
            .into_iter()
            .find(|c| c.engine == engine && c.name == name)
            .ok_or_else(|| anyhow::anyhow!("no {} connection named '{name}'", engine.label()));
    }
    let hits: Vec<Conn> = all.into_iter().filter(|c| c.name == needle).collect();
    match hits.len() {
        0 => anyhow::bail!("no saved connection named '{needle}' (try `esql ls`)"),
        1 => Ok(hits.into_iter().next().unwrap()),
        _ => {
            let names: Vec<String> = hits.iter().map(|c| c.key()).collect();
            anyhow::bail!(
                "'{needle}' is defined for several engines: {}. Name one of them.",
                names.join(", ")
            )
        }
    }
}

/// What the CLI is handed: a name, or `<name>/<db>` to point that connection at
/// another database on the same server. The whole string is tried as a name
/// first, so a connection whose own name contains a slash still resolves.
pub fn find_target(needle: &str) -> Result<(Conn, Option<String>)> {
    find_target_among(list(), needle)
}

fn find_target_among(all: Vec<Conn>, needle: &str) -> Result<(Conn, Option<String>)> {
    match find_among(all.clone(), needle) {
        Ok(c) => Ok((c, None)),
        // An ambiguous name is refused like any other, never reread as a
        // different connection plus a database: that runs on the wrong host.
        Err(e) if all.iter().any(|c| c.name == needle) => Err(e),
        Err(e) => match needle.rsplit_once('/') {
            Some((name, db)) if !name.is_empty() && !db.is_empty() => {
                Ok((find_among(all, name)?, Some(db.to_string())))
            }
            _ => Err(e),
        },
    }
}

/// Create or rewrite a connection. `original` is the name being replaced, which
/// differs from `nc.name` only when an edit renamed it.
pub fn save(original: Option<&str>, nc: &NewConn) -> Result<()> {
    match nc.engine {
        Engine::Pg => pg::save(original, nc),
        Engine::MySql => mysql::save(original, nc),
        Engine::Sqlite => sqlite::save(original, nc),
        Engine::MsSql => mssql::save(original, nc),
    }
}

pub fn delete(c: &Conn) -> Result<()> {
    match c.engine {
        Engine::Pg => pg::delete(&c.name),
        Engine::MySql => mysql::delete(&c.name),
        Engine::Sqlite => sqlite::delete(&c.name),
        Engine::MsSql => mssql::delete(&c.name),
    }
}

/// Write the form's `sslmode` answer into a Postgres block's extras, `None`
/// removing the key. It replaces whatever was there rather than adding to it.
pub fn set_sslmode(extra: &mut Vec<(String, String)>, mode: Option<&str>) {
    extra.retain(|(k, _)| !k.eq_ignore_ascii_case("sslmode"));
    if let Some(mode) = mode {
        extra.push(("sslmode".to_string(), mode.to_string()));
    }
}

/// Write SQL Server's certificate answer into its extras, off removing the key.
pub fn set_trust_cert(extra: &mut Vec<(String, String)>, on: bool) {
    extra.retain(|(k, _)| !k.eq_ignore_ascii_case("trust_cert"));
    if on {
        extra.push(("trust_cert".to_string(), "yes".to_string()));
    }
}

/// Make every session on the connection refuse writes, or stop doing so. Call
/// it only when the answer changed, so a line spelled by hand is not respelled.
/// A no-op for an engine where `Engine::offers_read_only` is false.
pub fn set_read_only(engine: Engine, extra: &mut Vec<(String, String)>, on: bool) {
    match engine {
        Engine::Pg => pg::set_read_only(extra, on),
        Engine::Sqlite => {
            extra.retain(|(k, _)| !k.eq_ignore_ascii_case("readonly"));
            if on {
                extra.push(("readonly".to_string(), "yes".to_string()));
            }
        }
        Engine::MySql | Engine::MsSql => {}
    }
}

/// After a rename is saved, move everything else keyed on `Conn::key()` or on
/// the name: the remembered tunnel, the history and a kept SQL Server password.
/// Each is tried even when one before it failed, and the first failure is
/// returned, saying which of them did not move.
pub fn carry_rename(engine: Engine, from: &str, to: &str) -> Result<()> {
    let old_key = format!("{}:{from}", engine.slug());
    let new_key = format!("{}:{to}", engine.slug());
    let tunnel = crate::vias::rename(&old_key, &new_key);
    crate::history::rename(&old_key, &new_key);
    let password = match engine {
        Engine::MsSql => mssql::rename_password(from, to),
        _ => Ok(()),
    };
    match (tunnel, password) {
        (Err(e), _) => Err(e.context("its tunnel record did not move")),
        (_, Err(e)) => Err(e.context("its password did not move")),
        _ => Ok(()),
    }
}

/// Delete a connection and what easysql keeps for it: the remembered tunnel,
/// so the file does not fill with forwards for nothing, and a kept SQL Server
/// password, so a new connection reusing the name does not inherit it. A
/// `.pgpass` line is the client's and stays.
pub fn delete_with_keyed(c: &Conn) -> Result<()> {
    delete(c).map_err(|e| e.context("it was not deleted"))?;
    crate::vias::remove(&c.key())
        .map_err(|e| e.context("deleted, but its tunnel record was not forgotten"))?;
    if c.engine == Engine::MsSql {
        mssql::set_password(&c.name, None)
            .map_err(|e| e.context("deleted, but its kept password was not forgotten"))?;
    }
    Ok(())
}

/// One connection by the name the CLI is given: bare `prod`, or `pg:prod` when
/// two engines both have one. Never split on `/`, unlike `find_target`.
pub fn find(needle: &str) -> Result<Conn> {
    find_among(list(), needle)
}

/// Is this engine's client actually installed? A row whose client is missing is
/// shown greyed with an install hint rather than failing at the moment you press
/// Enter on it.
pub fn client_installed(engine: Engine, s: &Settings) -> bool {
    let argv = engine.client_argv(s);
    on_path(&argv[0])
}

pub fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).is_file();
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

// ---- installing a missing client ------------------------------------------
//
// The clients are not Rust, so easysql works out the one command this machine
// needs and offers to run it, rather than printing three distros' guesses.

/// The package managers we know how to drive. An enum rather than a string, so
/// the table below is checked by the compiler instead of by whoever last read it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PackageManager {
    Apt,
    Pacman,
    Dnf,
    Zypper,
    Apk,
}

impl PackageManager {
    /// In the order we look for them.
    const ALL: [PackageManager; 5] = [
        PackageManager::Apt,
        PackageManager::Pacman,
        PackageManager::Dnf,
        PackageManager::Zypper,
        PackageManager::Apk,
    ];

    /// The program to look for, which is also the one to run.
    fn program(self) -> &'static str {
        match self {
            PackageManager::Apt => "apt-get",
            PackageManager::Pacman => "pacman",
            PackageManager::Dnf => "dnf",
            PackageManager::Zypper => "zypper",
            PackageManager::Apk => "apk",
        }
    }

    fn verb(self) -> &'static str {
        match self {
            PackageManager::Pacman => "-S",
            PackageManager::Apk => "add",
            _ => "install",
        }
    }

    /// Which one this machine has. We probe PATH rather than reading
    /// `/etc/os-release`, because what is installed is the fact that matters:
    /// the distro a machine claims to be is not always the thing managing it.
    fn detect() -> Option<Self> {
        Self::ALL.into_iter().find(|m| on_path(m.program()))
    }
}

/// What each engine's client is called, per manager. `None` means we do not
/// know for certain, and naming a package that does not exist is worse than
/// admitting that.
///
/// Deliberately exhaustive with no catch-all arm: adding an engine or a manager
/// is then a compile error listing exactly which cells are still owed, rather
/// than a feature that silently ships half-wired.
fn package(manager: PackageManager, engine: Engine) -> Option<&'static str> {
    use Engine::{MsSql, MySql, Pg, Sqlite};
    use PackageManager::{Apk, Apt, Dnf, Pacman, Zypper};
    match (manager, engine) {
        (Apt, Pg) => Some("postgresql-client"),
        (Apt, MySql) => Some("mariadb-client"),
        (Apt, Sqlite) => Some("sqlite3"),
        // Arch ships the client inside the server package; there is no split.
        (Pacman, Pg) => Some("postgresql"),
        (Pacman, MySql) => Some("mariadb-clients"),
        (Pacman, Sqlite) => Some("sqlite"),
        (Dnf, Pg) => Some("postgresql"),
        (Dnf, MySql) => Some("mariadb"),
        (Dnf, Sqlite) => Some("sqlite"),
        (Zypper, Pg) => Some("postgresql"),
        (Zypper, MySql) => Some("mariadb-client"),
        (Zypper, Sqlite) => Some("sqlite3"),
        (Apk, Pg) => Some("postgresql-client"),
        (Apk, MySql) => Some("mariadb-client"),
        (Apk, Sqlite) => Some("sqlite"),
        // Microsoft ships sqlcmd from its own repo and from GitHub releases, and
        // it is in no distro's archive. There is no package name to name here,
        // and inventing one would send somebody to a package that does not
        // exist; `install_note` says where it actually comes from instead.
        (Apt, MsSql) | (Pacman, MsSql) | (Dnf, MsSql) | (Zypper, MsSql) | (Apk, MsSql) => None,
    }
}

/// The exact command that would install the client this engine's sessions run
/// under these settings. `None` on a machine whose package manager we do not
/// recognise, where the honest answer is to name the program and let the user
/// find it, and for usql, whose distro packages can lack the SQL Server driver.
pub fn install_argv(engine: Engine, s: &Settings) -> Option<Vec<String>> {
    if engine.uses_usql(s) {
        return None;
    }
    let manager = PackageManager::detect()?;
    let pkg = package(manager, engine)?;
    // No `-y`: the modal already asked whether to do this, and the package
    // manager showing what it is about to pull in is information, not noise.
    Some(vec![
        "sudo".to_string(),
        manager.program().to_string(),
        manager.verb().to_string(),
        pkg.to_string(),
    ])
}

/// The hint plus this machine's snippets, when the client can expand them
/// itself. Only psql can, so only psql is told about them - offering `:tables`
/// to a client that will read it as a syntax error is worse than saying nothing.
/// `by_name` says usql was handed the server's databases, so `\c <db>` works.
pub fn hint_line(engine: Engine, s: &Settings, by_name: bool) -> String {
    hint_line_from(engine, s, &crate::snippets::list(), by_name)
}

/// True when the configured command really is this engine's own client, even
/// wrapped: `docker exec -it db psql` still takes psql's flags, `pgcli` does
/// not. Every one-shot form is spelled in the real client's flags, so this is
/// the gate in front of them - a `-c` handed to pgcli is a usage error, and one
/// invented for it would be a guess.
pub fn speaks_client_flags(engine: Engine, s: &Settings) -> bool {
    engine.uses_usql(s)
        || engine.client_argv(s).iter().any(|w| {
            std::path::Path::new(w)
                .file_name()
                .is_some_and(|f| f == engine.default_client())
        })
}

/// The half that decides, kept apart from the half that reads the disk, so it
/// can be exercised without a real `~/.config/easysql/snippets` anywhere near
/// it - the same seam `pg::list_in` and `creds::set_pg_in` have.
fn hint_line_from(
    engine: Engine,
    s: &Settings,
    snips: &[crate::snippets::Snippet],
    by_name: bool,
) -> String {
    if engine.uses_usql(s) {
        return engine.usql_hint(by_name).to_string();
    }
    let base = engine.hint().to_string();
    // Only psql reads `~/.psqlrc`, so pgcli or a docker wrapper would answer
    // `:name` with a syntax error. Same gate as the install offer: default
    // client, or nothing.
    let is_psql = engine
        .client_argv(s)
        .first()
        .and_then(|p| std::path::Path::new(p).file_name())
        .is_some_and(|p| p == engine.default_client());
    if engine != Engine::Pg || !is_psql {
        return base;
    }
    let names: Vec<String> = snips
        .iter()
        .filter(|s| crate::snippets::psql_name(&s.name))
        .map(|s| format!(":{}", s.name))
        .collect();
    match names.is_empty() {
        true => base,
        false => format!("{base}\n  {}", names.join(" · ")),
    }
}

/// Just the package name this machine calls the client, for a row that has no
/// room for the whole command. The name is the part nobody can guess: `psql`
/// lives in `postgresql-client` on Debian and in `postgresql` on Arch.
pub fn install_package(engine: Engine, s: &Settings) -> Option<&'static str> {
    if engine.uses_usql(s) {
        return None;
    }
    package(PackageManager::detect()?, engine)
}

/// How to install the missing client, as one line: the exact command for this
/// machine when we recognise it, and the program to go looking for when we do
/// not. Never a list of distros the reader has to pick themselves out of.
pub fn install_hint(engine: Engine, s: &Settings) -> String {
    match install_argv(engine, s) {
        Some(argv) => argv.join(" "),
        None => match install_note(engine, s) {
            Some(note) => note.to_string(),
            None => format!("install your distro's {} package", engine.default_client()),
        },
    }
}

/// Where a client comes from when no package manager here has it: sqlcmd,
/// which no distro has, and usql, whose Debian package is built without the
/// SQL Server driver.
pub fn install_note(engine: Engine, s: &Settings) -> Option<&'static str> {
    if engine.uses_usql(s) {
        return Some(match s.in_tui {
            true => {
                "get usql's static build from https://github.com/xo/usql/releases (a distro's usql may lack the SQL Server driver), or `esql set usql_tui off`"
            }
            false => {
                "get usql's static build from https://github.com/xo/usql/releases (a distro's usql may lack the SQL Server driver), or `esql set usql_cli off`"
            }
        });
    }
    match engine {
        Engine::MsSql => Some(
            "sqlcmd is not in any distro's repos: get it from https://aka.ms/go-sqlcmd, or add Microsoft's repo for mssql-tools18 (which installs to /opt/mssql-tools18/bin, not on PATH)",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(engine: Engine, name: &str) -> Conn {
        Conn {
            engine,
            name: name.to_string(),
            host: "db.example.com".into(),
            port: "5432".into(),
            database: "app".into(),
            user: "me".into(),
            extra: Vec::new(),
        }
    }

    #[test]
    fn key_is_slug_and_name_so_two_engines_may_share_a_name() {
        assert_eq!(conn(Engine::Pg, "prod").key(), "pg:prod");
        assert_eq!(conn(Engine::MySql, "prod").key(), "my:prod");
        assert_eq!(conn(Engine::Sqlite, "prod").key(), "lite:prod");
        assert_eq!(conn(Engine::MsSql, "prod").key(), "ms:prod");
    }

    #[test]
    fn each_engine_is_opened_through_the_file_its_client_already_reads() {
        // The whole design rule: we never pass a host or a password on the
        // command line, we name the block the client reads it out of.
        let s = Settings::default();
        assert_eq!(
            conn(Engine::Pg, "prod").connect_argv(&s),
            vec!["psql", "service=prod"]
        );
        assert_eq!(
            conn(Engine::MySql, "prod").connect_argv(&s),
            vec!["mysql", "--defaults-group-suffix=prod"]
        );
    }

    #[test]
    fn sqlite_is_handed_an_expanded_path_because_children_run_without_a_shell() {
        let mut c = conn(Engine::Sqlite, "notes");
        c.database = "~/notes.sqlite".into();
        let argv = c.connect_argv(&Settings::default());
        let home = dirs::home_dir().unwrap_or_default();
        assert_eq!(argv[0], "sqlite3");
        assert_eq!(argv[1], home.join("notes.sqlite").to_string_lossy());
        assert!(
            !argv[1].starts_with('~'),
            "a literal ~ reaches sqlite3 as a directory name"
        );
    }

    #[test]
    fn sqlcmd_takes_host_and_port_separated_by_a_comma_not_a_colon() {
        let argv = conn(Engine::MsSql, "prod").connect_argv(&Settings::default());
        assert_eq!(
            argv,
            vec![
                "sqlcmd",
                "-S",
                "db.example.com,5432",
                "-d",
                "app",
                "-U",
                "me"
            ]
        );
        // No `-P`: Microsoft's own advice is to let sqlcmd prompt.
        assert!(!argv.iter().any(|a| a == "-P"));
    }

    #[test]
    fn a_wrapper_client_with_its_own_arguments_survives_the_split() {
        // Somebody who points psql_command at a docker wrapper meant it.
        let s = Settings {
            psql_command: "docker exec -it db psql".into(),
            ..Settings::default()
        };
        assert_eq!(
            conn(Engine::Pg, "prod").connect_argv(&s),
            vec!["docker", "exec", "-it", "db", "psql", "service=prod"]
        );
    }

    #[test]
    fn only_sql_server_refuses_to_store_a_password() {
        assert!(Engine::Pg.stores_password());
        assert!(Engine::MySql.stores_password());
        assert!(!Engine::MsSql.stores_password());
    }

    #[test]
    fn snippets_are_only_advertised_to_a_client_that_can_expand_them() {
        // `:name` is a psql variable, expanded from the `\set` block easysql
        // writes into `~/.psqlrc`. pgcli reads neither, so offering `:name`
        // there advertises something that answers with a syntax error - which
        // is exactly what it did before this gate existed.
        let snips = vec![crate::snippets::Snippet {
            name: "tables".into(),
            sql: "select 1;".into(),
            path: std::path::PathBuf::new(),
        }];
        let line = |cmd: &str| {
            let s = Settings {
                psql_command: cmd.into(),
                ..Default::default()
            };
            hint_line_from(Engine::Pg, &s, &snips, false)
        };

        // The real psql, by name or by full path, gets the shortcuts - and so
        // does an empty command, because `client_argv` reads that as "the
        // default", which is psql.
        assert!(line("psql").contains(":tables"));
        assert!(line("/usr/bin/psql").contains(":tables"));
        assert!(line("").contains(":tables"));

        // Anything else does not, however it is spelled.
        assert_eq!(line("pgcli"), Engine::Pg.hint());
        assert_eq!(line("docker exec -it db psql"), Engine::Pg.hint());

        // And the other three never get the line at all, because none of them
        // has variables of any kind.
        let s = Settings::default();
        for e in [Engine::MySql, Engine::Sqlite, Engine::MsSql] {
            assert_eq!(hint_line_from(e, &s, &snips, false), e.hint());
        }
    }

    #[test]
    fn from_slug_accepts_both_the_slug_and_the_label() {
        assert_eq!(Engine::from_slug("pg"), Some(Engine::Pg));
        assert_eq!(Engine::from_slug("postgres"), Some(Engine::Pg));
        assert_eq!(Engine::from_slug("nope"), None);
    }

    fn read_only(mut c: Conn) -> Conn {
        match c.engine {
            Engine::Pg => pg::set_read_only(&mut c.extra, true),
            _ => c.extra.push(("readonly".into(), "yes".into())),
        }
        c
    }

    fn client(engine: Engine, command: &str) -> Settings {
        let mut s = Settings::default();
        s.set(engine.client_setting(), command);
        s
    }

    #[test]
    fn a_wrapper_around_psql_speaks_its_flags_and_pgcli_does_not() {
        for speaks in ["psql", "/usr/bin/psql", "docker exec -it db psql"] {
            assert!(
                speaks_client_flags(Engine::Pg, &client(Engine::Pg, speaks)),
                "`{speaks}` takes psql's -c"
            );
        }
        for foreign in ["pgcli", "usql"] {
            assert!(
                !speaks_client_flags(Engine::Pg, &client(Engine::Pg, foreign)),
                "`{foreign}` would be handed a flag it rejects"
            );
        }
    }

    #[test]
    fn pgoptions_goes_to_a_client_other_than_psql_and_only_on_read_only() {
        let ro = read_only(conn(Engine::Pg, "prod"));
        let env = ro.connect_env(&client(Engine::Pg, "pgcli"));
        assert!(
            env.iter()
                .any(|(k, v)| k == "PGOPTIONS" && v.contains("default_transaction_read_only=on")),
            "pgcli drops the service's options, so the setting must travel in PGOPTIONS: {env:?}"
        );
        assert!(
            ro.connect_env(&client(Engine::Pg, "psql")).is_empty(),
            "psql applies the service's options itself"
        );
        assert!(
            conn(Engine::Pg, "prod")
                .connect_env(&client(Engine::Pg, "pgcli"))
                .is_empty(),
            "a writable connection gets nothing"
        );
    }

    #[test]
    fn a_read_only_sqlite_connection_runs_sqlite3_whatever_client_is_set() {
        let mut c = read_only(conn(Engine::Sqlite, "notes"));
        c.database = "/tmp/notes.db".into();
        let argv = c.connect_argv(&client(Engine::Sqlite, "litecli"));
        assert_eq!(
            argv,
            vec!["sqlite3", "-readonly", "/tmp/notes.db"],
            "litecli rejects -readonly, so it must not be the one handed it"
        );
        let mut writable = conn(Engine::Sqlite, "notes");
        writable.database = "/tmp/notes.db".into();
        assert_eq!(
            writable.connect_argv(&client(Engine::Sqlite, "litecli"))[0],
            "litecli",
            "a writable connection still opens in the configured client"
        );
    }

    #[test]
    fn an_ambiguous_name_is_refused_rather_than_split() {
        let named = |engine, name: &str, host: &str| Conn {
            host: host.into(),
            ..conn(engine, name)
        };
        let all = vec![
            named(Engine::Pg, "team", "other.example"),
            named(Engine::Pg, "team/app", "right.example"),
            named(Engine::Sqlite, "team/app", ""),
        ];
        assert!(
            find_target_among(all.clone(), "team/app").is_err(),
            "two engines own `team/app`, so it must not become `team` plus database `app`"
        );
        let (c, db) = find_target_among(all, "pg:team/app").expect("the engine settles it");
        assert_eq!((c.host.as_str(), db), ("right.example", None));

        let (c, db) = find_target_among(vec![conn(Engine::Pg, "prod")], "prod/reporting")
            .expect("an unambiguous name still takes a database");
        assert_eq!(
            (c.name.as_str(), db.as_deref()),
            ("prod", Some("reporting"))
        );
    }
}
