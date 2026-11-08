//! Passwords, kept where each client already looks for them and nowhere else.
//!
//! Postgres reads `~/.pgpass`, one `host:port:database:user:password` line per
//! entry with `*` as a wildcard. MySQL reads the `password` key inside the
//! connection's own `[clientNAME]` group. easysql writes those files and then
//! forgets: no password is ever held in the app, put in an environment variable
//! or passed on a command line, because `ps` shows a command line to every user
//! on the machine. SQL Server is the exception the user has to switch on: its
//! client has no file, so `engines::mssql` keeps one when allowed.
//!
//! Nothing here ever reads a password back out. A row shows which connection it
//! answers for; the secret itself is only ever written.

use crate::engines::{self, Conn, Engine};
use crate::ini;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Where one credential physically lives, which is what a delete has to know.
#[derive(Clone, PartialEq)]
pub enum Source {
    /// The nth non-comment entry of `~/.pgpass`.
    Pgpass(usize),
    /// The `password` key of `[clientNAME]` in `~/.my.cnf`.
    MyCnf(String),
    /// The `password` key of `[NAME]` in easysql's own `~/.esqlpass`.
    MsSql(String),
}

/// One stored password, described by what it unlocks rather than by its value.
#[derive(Clone)]
pub struct Cred {
    pub engine: Engine,
    pub host: String,
    pub port: String,
    pub database: String,
    pub user: String,
    pub source: Source,
}

impl Cred {
    /// The one-line summary: which server, which database, which user.
    pub fn describe(&self) -> String {
        format!(
            "{:<9} {:<28} {:<14} {}",
            self.engine.label(),
            format!("{}:{}", self.host, self.port),
            self.database,
            self.user
        )
    }

    /// Where a delete would go, in words.
    pub fn where_stored(&self) -> String {
        match &self.source {
            Source::Pgpass(_) => ini::collapse_tilde(&pgpass_path().to_string_lossy()),
            Source::MyCnf(name) => format!(
                "{} [{}]",
                ini::collapse_tilde(&engines::mysql::cnf_path().to_string_lossy()),
                engines::mysql::group_of(name)
            ),
            Source::MsSql(name) => format!(
                "{} [{name}]",
                ini::collapse_tilde(&engines::mssql::pass_path().to_string_lossy())
            ),
        }
    }

    /// Does this entry answer for that connection? `*` matches anything, which
    /// is what makes one `.pgpass` line cover a whole cluster.
    pub fn covers(&self, c: &Conn) -> bool {
        if self.engine != c.engine {
            return false;
        }
        match &self.source {
            Source::MyCnf(name) | Source::MsSql(name) => name == &c.name,
            Source::Pgpass(_) => {
                let host = pgpass_host(&c.host);
                field_matches(&self.host, host)
                    && field_matches(&self.port, &c.port_or_default())
                    && field_matches(&self.database, &c.database)
                    && field_matches(&self.user, &c.user)
            }
        }
    }
}

/// A `.pgpass` field matches when it is the wildcard or the exact value. An
/// empty value on the connection's side (no dbname set, say) cannot be checked,
/// so only the wildcard covers it.
fn field_matches(pattern: &str, value: &str) -> bool {
    pattern == "*" || (!value.is_empty() && pattern == value)
}

/// The host a `.pgpass` line names for a connection to `host`, as libpq
/// matches it: `localhost` for no host or a default socket directory, which
/// libpq reads as this machine, and `host` itself otherwise. The socket
/// directories are the ones libpq is built with on the distributions and
/// upstream, since which one this machine's libpq has is not knowable here.
pub fn pgpass_host(host: &str) -> &str {
    const DEFAULT_SOCKETS: [&str; 3] = ["/var/run/postgresql", "/run/postgresql", "/tmp"];
    match host.trim_end_matches('/') {
        "" => "localhost",
        h if DEFAULT_SOCKETS.contains(&h) => "localhost",
        _ => host,
    }
}

/// `$PGPASSFILE`, else `~/.pgpass`. libpq's own precedence.
pub fn pgpass_path() -> PathBuf {
    if let Some(p) = std::env::var_os("PGPASSFILE") {
        return PathBuf::from(p);
    }
    dirs::home_dir().unwrap_or_default().join(".pgpass")
}

/// Every stored password, Postgres first.
pub fn list() -> Vec<Cred> {
    let mut out = pgpass_entries();
    for c in engines::mysql::list() {
        if engines::mysql::has_password(&c.name) {
            out.push(Cred {
                engine: Engine::MySql,
                host: if c.host.is_empty() {
                    "localhost".into()
                } else {
                    c.host.clone()
                },
                port: c.port_or_default(),
                database: c.database.clone(),
                user: c.user.clone(),
                source: Source::MyCnf(c.name),
            });
        }
    }
    // Listed whatever the setting says, so a password kept while it was on can
    // still be seen and forgotten after it is switched off.
    for c in engines::mssql::list() {
        if engines::mssql::has_password(&c.name) {
            out.push(Cred {
                engine: Engine::MsSql,
                host: c.host.clone(),
                port: c.port_or_default(),
                database: c.database.clone(),
                user: c.user.clone(),
                source: Source::MsSql(c.name),
            });
        }
    }
    out
}

fn pgpass_entries() -> Vec<Cred> {
    let lines = read_lines(&pgpass_path()).unwrap_or_default();
    lines
        .iter()
        .filter(|l| l.entry)
        .enumerate()
        .filter_map(|(i, l)| {
            let f = l.fields.as_ref()?;
            Some(Cred {
                engine: Engine::Pg,
                host: f[0].clone(),
                port: f[1].clone(),
                database: f[2].clone(),
                user: f[3].clone(),
                source: Source::Pgpass(i),
            })
        })
        .collect()
}

/// One line of `.pgpass`: its bytes as written, whether it is an entry rather
/// than a blank or a comment, and its five fields when it is one we
/// understand. A line that is not UTF-8 is legal to libpq, so it is copied,
/// never decoded and written back.
struct PgLine {
    raw: Vec<u8>,
    entry: bool,
    fields: Option<Vec<String>>,
}

/// The file's lines, none when it does not exist yet. Any other failure is an
/// error, never an empty file to write over.
fn read_lines(path: &Path) -> Result<Vec<PgLine>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    Ok(bytes
        .split_inclusive(|b| *b == b'\n')
        .map(|raw| {
            let line = raw.strip_suffix(b"\n").unwrap_or(raw);
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let text = String::from_utf8_lossy(line);
            let entry = !text.trim().is_empty() && !text.trim_start().starts_with('#');
            // Five fields exactly; anything else is not an entry we understand,
            // and guessing at it is how a password file gets corrupted.
            let fields = entry.then(|| split_pgpass(&text)).filter(|f| f.len() == 5);
            PgLine {
                raw: raw.to_vec(),
                entry,
                fields,
            }
        })
        .collect())
}

/// A `.pgpass` entry no longer in the file with the four fields it was
/// listed with, so nothing was touched.
#[derive(Debug)]
pub struct Gone(String);

impl std::fmt::Display for Gone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} no longer has that entry as it was listed, so nothing was touched",
            self.0
        )
    }
}

impl std::error::Error for Gone {}

/// The position in `lines` of the entry the user was shown: entry number
/// `idx`, counted the way `Source::Pgpass` counts them, while it still holds
/// the four fields `was`, else the first entry that does, since a line added
/// above it moves it without changing it. `Gone` when no entry has them.
fn entry_at(lines: &[PgLine], idx: usize, was: [&str; 4], path: &Path) -> Result<usize> {
    let same = |i: usize| {
        lines[i]
            .fields
            .as_ref()
            .is_some_and(|f| (0..4).all(|k| f[k] == was[k]))
    };
    let mut entries = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.entry)
        .map(|(i, _)| i);
    let at = entries.clone().nth(idx).filter(|&i| same(i));
    match at.or_else(|| entries.find(|&i| same(i))) {
        Some(i) => Ok(i),
        None => Err(Gone(ini::collapse_tilde(&path.to_string_lossy())).into()),
    }
}

/// Split on unescaped colons. `\:` is a literal colon and `\\` a literal
/// backslash, which is how a password containing either survives the format.
fn split_pgpass(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(next) = chars.next() {
                    out.last_mut().unwrap().push(next);
                }
            }
            ':' => out.push(String::new()),
            _ => out.last_mut().unwrap().push(c),
        }
    }
    out
}

/// The inverse: a value going into a `.pgpass` field.
fn escape_pgpass(value: &str) -> String {
    value.replace('\\', "\\\\").replace(':', "\\:")
}

/// Add or replace a Postgres password. An entry with the same first four fields
/// is replaced in place rather than appended, so re-answering the wizard fixes
/// a password instead of stacking a second line libpq would never reach.
pub fn set_pg(host: &str, port: &str, database: &str, user: &str, password: &str) -> Result<()> {
    set_pg_in(&pgpass_path(), host, port, database, user, password)
}

/// `set_pg` against a given file. The same seam `pg::list_in` and `vias::set_in`
/// have, so the escaping and the replace can be exercised without a real
/// `~/.pgpass` anywhere near it.
pub fn set_pg_in(
    path: &Path,
    host: &str,
    port: &str,
    database: &str,
    user: &str,
    password: &str,
) -> Result<()> {
    let new = [host, port, database, user].map(widen);
    let mut lines = read_lines(path)?;
    let entry = format!(
        "{}\n",
        [new[0], new[1], new[2], new[3], password]
            .iter()
            .map(|f| escape_pgpass(f))
            .collect::<Vec<_>>()
            .join(":")
    );
    let old = lines.iter().position(|l| {
        l.fields
            .as_ref()
            .is_some_and(|f| (0..4).all(|k| f[k] == new[k]))
    });
    if let Some(i) = old {
        lines.remove(i);
    }
    // libpq takes the first line that matches, so the entry goes in front of
    // the first line that matches every connection it does, or no connection
    // would ever reach it.
    let shadow = lines.iter().position(|l| {
        l.fields
            .as_ref()
            .is_some_and(|f| (0..4).all(|k| f[k] == "*" || f[k] == new[k]))
    });
    let at = match (shadow, old) {
        (Some(s), Some(o)) => s.min(o),
        (Some(at), None) | (None, Some(at)) => at,
        (None, None) => lines.len(),
    };
    lines.insert(
        at,
        PgLine {
            raw: entry.into_bytes(),
            entry: true,
            fields: None,
        },
    );
    if path.exists() {
        ini::backup(path)?;
    }
    write_pgpass(path, &lines)
}

/// A blank matching field as `*`: libpq always has a user, a host and a port,
/// so an empty field would match nothing and the password would never be used.
fn widen(field: &str) -> &str {
    if field.is_empty() { "*" } else { field }
}

/// Move an entry to a different host, port, database or user, keeping whatever
/// password it already holds.
///
/// The secret is copied from the old line to the new one and never leaves this
/// function: not shown, not previewed, not logged, not passed to a child. That
/// is the same handling every write already gives it, since rewriting the file
/// at all means reading the other entries through memory - the invariant is
/// that easysql never *surfaces* a stored password, not that it never touches
/// the bytes.
///
/// `idx` counts entries, not lines, so comments and blanks cannot shift which
/// row moves, and `was` is the entry's four fields as the user saw them: a
/// file edited since then is refused rather than another entry moved. The
/// moved line goes in front of the first line that matches every connection
/// it now does, as a new one would, and a move onto the four fields of
/// another entry is refused.
pub fn rekey_pg(
    idx: usize,
    was: [&str; 4],
    host: &str,
    port: &str,
    database: &str,
    user: &str,
) -> Result<()> {
    let path = pgpass_path();
    let lines = read_lines(&path)?;
    let at = entry_at(&lines, idx, was, &path)?;
    // Where the entry is now, counted as `rekey_pg_in` counts.
    let idx = lines[..at].iter().filter(|l| l.entry).count();
    rekey_pg_in(&path, idx, host, port, database, user)
}

pub fn rekey_pg_in(
    path: &Path,
    idx: usize,
    host: &str,
    port: &str,
    database: &str,
    user: &str,
) -> Result<()> {
    let new = [host, port, database, user].map(widen);
    let mut lines = read_lines(path)?;
    let at = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.entry)
        .nth(idx)
        .map(|(i, _)| i);
    let Some((at, f)) = at.and_then(|i| Some((i, lines[i].fields.clone()?))) else {
        anyhow::bail!("that entry is no longer in the file (press r to reload)");
    };
    let same = |g: &[String]| (0..4).all(|k| g[k] == new[k]);
    if same(&f) {
        return Ok(());
    }
    // libpq uses only the first of two entries for the same fields, and
    // replacing the other one would lose its password.
    if lines.iter().any(|l| l.fields.as_deref().is_some_and(same)) {
        anyhow::bail!(
            "{} already has an entry for {}: edit or delete that one instead",
            ini::collapse_tilde(&path.to_string_lossy()),
            new.join(":")
        );
    }
    let moved = [new[0], new[1], new[2], new[3], &f[4]]
        .iter()
        .map(|s| escape_pgpass(s))
        .collect::<Vec<_>>()
        .join(":");
    lines.remove(at);
    // In front of the first line that matches every connection it now does,
    // as `set_pg_in` places a new one, or libpq would never reach it.
    let shadow = lines.iter().position(|l| {
        l.fields
            .as_ref()
            .is_some_and(|g| (0..4).all(|k| g[k] == "*" || g[k] == new[k]))
    });
    lines.insert(
        shadow.map_or(at, |s| s.min(at)),
        PgLine {
            raw: format!("{moved}\n").into_bytes(),
            entry: true,
            fields: None,
        },
    );
    ini::backup(path)?;
    write_pgpass(path, &lines)
}

pub fn delete(cred: &Cred) -> Result<()> {
    match &cred.source {
        Source::MyCnf(name) => engines::mysql::set_password(name, None),
        Source::MsSql(name) => engines::mssql::set_password(name, None),
        Source::Pgpass(idx) => {
            let path = pgpass_path();
            let mut lines = read_lines(&path)?;
            // The index counts entries, not lines, so comments and blanks are
            // copied through and cannot shift which row gets removed.
            let was = [&cred.host, &cred.port, &cred.database, &cred.user].map(String::as_str);
            let at = entry_at(&lines, *idx, was, &path)?;
            ini::backup(&path)?;
            lines.remove(at);
            write_pgpass(&path, &lines)
        }
    }
}

/// Every line ends in a line break. libpq flatly refuses a `.pgpass` that
/// anyone else can read, so it is written 0600 whatever its name, as
/// `$PGPASSFILE` may give it any.
fn write_pgpass(path: &Path, lines: &[PgLine]) -> Result<()> {
    let mut body = Vec::new();
    for l in lines {
        body.extend_from_slice(&l.raw);
        if !l.raw.ends_with(b"\n") {
            body.push(b'\n');
        }
    }
    ini::write_secret_bytes(path, &body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A throwaway `.pgpass` that deletes itself. The real one
    /// is never opened by this suite.
    struct Temp(PathBuf);

    impl Temp {
        fn new(body: &str) -> Temp {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("easysql-pgpass-{}-{stamp}", std::process::id()));
            fs::write(&path, body).unwrap();
            Temp(path)
        }

        fn lines(&self) -> Vec<String> {
            fs::read_to_string(&self.0)
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect()
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn a_blank_pgpass_field_is_written_as_a_wildcard() {
        // libpq always has a user, so an empty field would match nothing and
        // the password would never be used.
        let f = Temp::new("");
        set_pg_in(&f.0, "db.example.com", "5432", "*", "", "pw").unwrap();
        assert_eq!(f.lines(), vec!["db.example.com:5432:*:*:pw".to_string()]);
    }

    #[test]
    fn a_colon_or_backslash_in_a_field_is_escaped() {
        // `.pgpass` is colon-separated with `\` as the escape, so an unescaped
        // `:` in a password would silently shift every field after it.
        let f = Temp::new("");
        set_pg_in(&f.0, "db.example.com", "5432", "app", "me", r"pa:ss\word").unwrap();
        assert_eq!(
            f.lines(),
            vec![r"db.example.com:5432:app:me:pa\:ss\\word".to_string()]
        );
    }

    #[test]
    fn an_entry_with_the_same_first_four_fields_is_replaced_in_place() {
        // libpq takes the first match, so an appended second line for the same
        // host/port/database/user would never be reached.
        let f = Temp::new("db.example.com:5432:app:me:old\nother.example.com:5432:app:me:keep\n");
        set_pg_in(&f.0, "db.example.com", "5432", "app", "me", "new").unwrap();
        assert_eq!(
            f.lines(),
            vec![
                "db.example.com:5432:app:me:new".to_string(),
                "other.example.com:5432:app:me:keep".to_string(),
            ]
        );
    }

    #[test]
    fn a_different_user_on_the_same_database_appends_instead() {
        let f = Temp::new("db.example.com:5432:app:me:mine\n");
        set_pg_in(&f.0, "db.example.com", "5432", "app", "you", "yours").unwrap();
        assert_eq!(
            f.lines(),
            vec![
                "db.example.com:5432:app:me:mine".to_string(),
                "db.example.com:5432:app:you:yours".to_string(),
            ]
        );
    }

    #[test]
    fn moving_an_entry_keeps_its_password_and_everything_else_in_the_file() {
        // The point of `e` on the Passwords tab: correct the host or widen the
        // database without being asked for a secret easysql cannot show you.
        let f =
            Temp::new("# mine\n\nold.example.com:5432:app:bob:s3cret\nother:5432:*:eve:hunter2\n");
        rekey_pg_in(&f.0, 0, "new.example.com", "5433", "*", "bob").unwrap();
        let lines = f.lines();
        assert!(lines.contains(&"new.example.com:5433:*:bob:s3cret".to_string()));
        assert!(lines.contains(&"# mine".to_string()));
        assert!(lines.contains(&"other:5432:*:eve:hunter2".to_string()));
    }

    #[test]
    fn the_file_is_chmod_600() {
        // Not hygiene: libpq flatly refuses a world-readable `.pgpass`, so the
        // file simply does not work without it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let f = Temp::new("");
            set_pg_in(&f.0, "db.example.com", "5432", "app", "me", "s3cret").unwrap();
            let mode = fs::metadata(&f.0).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "libpq ignores a .pgpass that is not 0600");
        }
    }

    #[test]
    fn a_new_entry_goes_in_front_of_the_line_that_would_hide_it() {
        let t = Temp::new("*:*:*:bob:old\n");
        set_pg_in(&t.0, "db.example.com", "5432", "app", "bob", "new").unwrap();
        assert_eq!(
            t.lines(),
            ["db.example.com:5432:app:bob:new", "*:*:*:bob:old"],
            "libpq takes the first line that matches, so behind the wildcard it is never used"
        );
    }

    #[test]
    fn a_moved_entry_goes_in_front_of_the_line_that_would_hide_it() {
        let t = Temp::new("*:*:*:bob:pw-one\nhost1:5432:*:alice:pw-two\n");
        rekey_pg_in(&t.0, 1, "host1", "5432", "*", "bob").unwrap();
        assert_eq!(
            t.lines(),
            ["host1:5432:*:bob:pw-two", "*:*:*:bob:pw-one"],
            "the moved entry keeps its password and is not left behind the wildcard"
        );
    }
}
