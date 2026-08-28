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

    /// True when this engine has a server to reach, and therefore a port to
    /// probe and a tunnel worth opening. SQLite is a file.
    pub fn networked(self) -> bool {
        self != Engine::Sqlite
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
        let mut argv = self.engine.client_argv(s);
        match self.engine {
            Engine::Pg => argv.push(format!("service={}", self.name)),
            Engine::MySql => argv.push(format!("--defaults-group-suffix={}", self.name)),
            Engine::Sqlite => argv.push(
                crate::ini::expand_tilde(&self.database)
                    .to_string_lossy()
                    .into_owned(),
            ),
            Engine::MsSql => argv.extend(mssql::flags(self)),
        }
        argv
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
pub fn find(needle: &str) -> Result<Conn> {
    let all = list();
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
// `cargo install easysql` gets you the front end and nothing else, because the
// clients are not Rust and never will be. That is not an excuse to hand somebody
// a tool that does not work: easysql works out the one command *this* machine
// needs and offers to run it, rather than printing three distros' worth of
// guesses and calling that help.

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

/// The exact command that would install this engine's client here. `None` on a
/// machine whose package manager we do not recognise, where the honest answer
/// is to name the program and let the user find it.
pub fn install_argv(engine: Engine) -> Option<Vec<String>> {
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

/// Just the package name this machine calls the client, for a row that has no
/// room for the whole command. The name is the part nobody can guess: `psql`
/// lives in `postgresql-client` on Debian and in `postgresql` on Arch.
pub fn install_package(engine: Engine) -> Option<&'static str> {
    package(PackageManager::detect()?, engine)
}

/// How to install the missing client, as one line: the exact command for this
/// machine when we recognise it, and the program to go looking for when we do
/// not. Never a list of distros the reader has to pick themselves out of.
pub fn install_hint(engine: Engine) -> String {
    match install_argv(engine) {
        Some(argv) => argv.join(" "),
        None => match install_note(engine) {
            Some(note) => note.to_string(),
            None => format!("install your distro's {} package", engine.default_client()),
        },
    }
}

/// Where a client comes from when no package manager here has it. Only SQL
/// Server needs this, and saying "install your distro's sqlcmd package" would
/// be worse than useless, because no distro has one.
pub fn install_note(engine: Engine) -> Option<&'static str> {
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
    fn from_slug_accepts_both_the_slug_and_the_label() {
        assert_eq!(Engine::from_slug("pg"), Some(Engine::Pg));
        assert_eq!(Engine::from_slug("postgres"), Some(Engine::Pg));
        assert_eq!(Engine::from_slug("nope"), None);
    }
}
