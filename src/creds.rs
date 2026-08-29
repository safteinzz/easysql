//! Passwords, kept where each client already looks for them and nowhere else.
//!
//! Postgres reads `~/.pgpass`, one `host:port:database:user:password` line per
//! entry with `*` as a wildcard. MySQL reads the `password` key inside the
//! connection's own `[clientNAME]` group. easysql writes those files and then
//! forgets: no password is ever held in the app, put in an environment variable
//! or passed on a command line, because `ps` shows a command line to every user
//! on the machine.
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
        }
    }

    /// Does this entry answer for that connection? `*` matches anything, which
    /// is what makes one `.pgpass` line cover a whole cluster.
    pub fn covers(&self, c: &Conn) -> bool {
        if self.engine != c.engine {
            return false;
        }
        match &self.source {
            Source::MyCnf(name) => name == &c.name,
            Source::Pgpass(_) => {
                let host = if c.host.is_empty() {
                    "localhost"
                } else {
                    &c.host
                };
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
    out
}

fn pgpass_entries() -> Vec<Cred> {
    let Ok(text) = fs::read_to_string(pgpass_path()) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .enumerate()
        .filter_map(|(i, line)| {
            let f = split_pgpass(line);
            // Five fields exactly; anything else is not an entry we understand,
            // and guessing at it is how a password file gets corrupted.
            (f.len() == 5).then(|| Cred {
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
    let text = fs::read_to_string(path).unwrap_or_default();
    let entry = [host, port, database, user, password]
        .iter()
        .map(|f| escape_pgpass(f))
        .collect::<Vec<_>>()
        .join(":");

    let mut out: Vec<String> = Vec::new();
    let mut replaced = false;
    for line in text.lines() {
        let f = split_pgpass(line);
        let same = f.len() == 5 && f[0] == host && f[1] == port && f[2] == database && f[3] == user;
        if same && !replaced {
            out.push(entry.clone());
            replaced = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !replaced {
        out.push(entry);
    }
    if path.exists() {
        ini::backup(path)?;
    }
    write_pgpass(path, &out)
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
/// row moves.
pub fn rekey_pg(idx: usize, host: &str, port: &str, database: &str, user: &str) -> Result<()> {
    rekey_pg_in(&pgpass_path(), idx, host, port, database, user)
}

pub fn rekey_pg_in(
    path: &Path,
    idx: usize,
    host: &str,
    port: &str,
    database: &str,
    user: &str,
) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut out: Vec<String> = Vec::new();
    let mut seen = 0usize;
    let mut moved = false;
    for line in text.lines() {
        let blank = line.trim().is_empty() || line.trim_start().starts_with('#');
        if blank {
            out.push(line.to_string());
            continue;
        }
        let f = split_pgpass(line);
        if seen == idx && f.len() == 5 {
            out.push(
                [host, port, database, user, &f[4]]
                    .iter()
                    .map(|s| escape_pgpass(s))
                    .collect::<Vec<_>>()
                    .join(":"),
            );
            moved = true;
        } else {
            out.push(line.to_string());
        }
        seen += 1;
    }
    if !moved {
        anyhow::bail!("that entry is no longer in the file (press r to reload)");
    }
    ini::backup(path)?;
    write_pgpass(path, &out)
}

pub fn delete(cred: &Cred) -> Result<()> {
    match &cred.source {
        Source::MyCnf(name) => engines::mysql::set_password(name, None),
        Source::Pgpass(idx) => {
            let path = pgpass_path();
            let text =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            ini::backup(&path)?;
            // The index counts entries, not lines, so comments and blanks are
            // copied through and cannot shift which row gets removed.
            let mut seen = 0usize;
            let out: Vec<String> = text
                .lines()
                .filter(|line| {
                    let blank = line.trim().is_empty() || line.trim_start().starts_with('#');
                    if blank {
                        return true;
                    }
                    let keep = seen != *idx;
                    seen += 1;
                    keep
                })
                .map(str::to_string)
                .collect();
            write_pgpass(&path, &out)
        }
    }
}

fn write_pgpass(path: &std::path::Path, lines: &[String]) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut body = lines.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    fs::write(path, body).with_context(|| format!("writing {}", path.display()))?;
    // libpq flatly refuses a .pgpass that anyone else can read, so this is not
    // hygiene, it is whether the file works at all.
    ini::harden(path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A throwaway `.pgpass` that deletes itself and its backups. The real one
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
            if let Some(dir) = self.0.parent() {
                let stem = format!("{}.bak.", self.0.file_name().unwrap().to_string_lossy());
                if let Ok(entries) = fs::read_dir(dir) {
                    for e in entries.flatten() {
                        if e.file_name().to_string_lossy().starts_with(&stem) {
                            let _ = fs::remove_file(e.path());
                        }
                    }
                }
            }
        }
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
}
