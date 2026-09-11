//! Postgres. Connections live in `~/.pg_service.conf`, the connection service
//! file `psql` (and libpq, and therefore nearly every Postgres client and
//! driver) already reads: `psql "service=prod"` is the whole handoff.
//!
//! Passwords are not here. They belong in `~/.pgpass`, which libpq reads by
//! itself, which is why easysql never has to hold one or put one on a command
//! line where `ps` would show it.

use super::{Conn, Engine, NewConn};
use crate::ini;
use anyhow::Result;
use std::path::PathBuf;

/// The keys the wizard owns. Anything else in a block (`sslmode`, `options`,
/// `connect_timeout`) is carried through an edit untouched.
const KNOWN: [&str; 4] = ["host", "port", "dbname", "user"];

/// `$PGSERVICEFILE`, else `~/.pg_service.conf`. Same precedence libpq uses, so
/// we always read and write the file the client will.
pub fn service_path() -> PathBuf {
    if let Some(p) = std::env::var_os("PGSERVICEFILE") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".pg_service.conf")
}

/// The server setting that makes a session refuse writes. It lives in the
/// block's `options` - libpq hands those to the server for every session, so
/// psql, pgAdmin and every driver reading this service get it too - alongside
/// whatever other `-c` the user already put there.
const READ_ONLY: &str = "default_transaction_read_only";

/// The `options` tokens that set `READ_ONLY`, in the three spellings the server
/// accepts: `-c name=v`, `-cname=v` and `--name=v` (dashes or underscores).
fn read_only_values(options: &str) -> Vec<(usize, usize, String)> {
    let words: Vec<&str> = options.split_whitespace().collect();
    let mut hits = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let (setting, span) = match words[i] {
            "-c" if i + 1 < words.len() => (words[i + 1], 2),
            w if w.starts_with("-c") => (&w[2..], 1),
            w if w.starts_with("--") => (&w[2..], 1),
            _ => ("", 1),
        };
        if let Some((name, value)) = setting.split_once('=')
            && name.replace('-', "_").eq_ignore_ascii_case(READ_ONLY)
        {
            hits.push((i, span, value.to_ascii_lowercase()));
        }
        i += span;
    }
    hits
}

/// `PGOPTIONS` for a read-only session: whatever the environment already had,
/// with the read-only setting appended so it wins. pgcli builds its own
/// connection from the service file and drops `options` (verified: a pgcli
/// session showed `default_transaction_read_only = off` and took a write), but
/// every libpq client reads this variable.
pub fn read_only_pgoptions() -> String {
    let ours = format!("-c {READ_ONLY}=on");
    match std::env::var("PGOPTIONS") {
        Ok(had) if !had.trim().is_empty() => format!("{} {ours}", had.trim()),
        _ => ours,
    }
}

/// Whether this block makes every session read-only, however that line got
/// there. The last setting wins, the way the server reads them.
pub fn read_only(extra: &[(String, String)]) -> bool {
    extra
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("options"))
        .and_then(|(_, v)| read_only_values(v).pop())
        .is_some_and(|(_, _, v)| truthy(&v))
}

/// A Postgres boolean that means on: `on`, `1`, or any prefix of `true` or
/// `yes`, which is how the server reads `t`, `y` and `tru` as well.
fn truthy(v: &str) -> bool {
    v == "on" || v == "1" || (!v.is_empty() && ("true".starts_with(v) || "yes".starts_with(v)))
}

/// Set or clear read-only in the block's `options`, keeping every other token
/// the user wrote there in order, and dropping the key when nothing is left.
pub fn set_read_only(extra: &mut Vec<(String, String)>, on: bool) {
    let at = extra
        .iter()
        .position(|(k, _)| k.eq_ignore_ascii_case("options"));
    let old = at.map(|i| extra[i].1.clone()).unwrap_or_default();
    let words: Vec<&str> = old.split_whitespace().collect();
    let mut drop = vec![false; words.len()];
    for (i, span, _) in read_only_values(&old) {
        drop[i..i + span].iter_mut().for_each(|d| *d = true);
    }
    let mut kept: Vec<String> = words
        .iter()
        .zip(&drop)
        .filter(|(_, d)| !**d)
        .map(|(w, _)| w.to_string())
        .collect();
    if on {
        kept.push("-c".into());
        kept.push(format!("{READ_ONLY}=on"));
    }
    let new = kept.join(" ");
    match (at, new.is_empty()) {
        (Some(i), true) => {
            extra.remove(i);
        }
        (Some(i), false) => extra[i].1 = new,
        (None, false) => extra.push(("options".into(), new)),
        (None, true) => {}
    }
}

/// What the server says, asked just before a read-only session, about whether
/// that session will really refuse writes.
pub enum ReadOnlyCheck {
    Holds,
    /// It answered `off`: something between here and the server dropped the
    /// setting on the way in.
    Writable,
    /// The setting was refused outright, in the refuser's own words.
    Refused(String),
    /// No answer to go on - no psql, a password it would have to prompt for, a
    /// timeout - in the client's own words.
    Unknown(String),
}

impl ReadOnlyCheck {
    /// What to say when this answer means not going in: a headline and what to
    /// do about it. `None` for the answers that let the session go ahead.
    pub fn refusal(&self, name: &str) -> Option<(String, String)> {
        const FIX: &str = "a pooler's admin can let it through with \
                           `track_extra_parameters = default_transaction_read_only` in pgbouncer";
        match self {
            ReadOnlyCheck::Holds | ReadOnlyCheck::Unknown(_) => None,
            ReadOnlyCheck::Writable => Some((
                format!(
                    "`{name}` is marked read-only, but the server says this session could \
                     write, so easysql will not go in."
                ),
                format!(
                    "something between here and the server dropped the setting - pgbouncer does \
                     with `options` in its `ignore_startup_parameters`; {FIX}."
                ),
            )),
            ReadOnlyCheck::Refused(said) => Some((
                format!("the read-only setting was refused on the way in: {said}"),
                format!("that is a connection pooler, which refuses it by default; {FIX}."),
            )),
        }
    }
}

/// Ask the server, the way the failure probe does - psql, `-w` so nothing can
/// prompt, a deadline - whether a session on this read-only connection really
/// refuses writes. The service's `options` are only a request: pgbouncer drops
/// it silently when `options` is in `ignore_startup_parameters` (verified), so
/// without asking, the blue name could be on a connection that writes.
pub fn check_read_only(c: &Conn, db: Option<&str>, timeout: u64) -> ReadOnlyCheck {
    let out = std::process::Command::new("psql")
        .args([
            conninfo(&c.name, db).as_str(),
            "-X",
            "-w",
            "-tAc",
            "show default_transaction_read_only",
        ])
        .env("PGCONNECT_TIMEOUT", timeout.to_string())
        .env_remove("PGOPTIONS")
        .output();
    let Ok(out) = out else {
        return ReadOnlyCheck::Unknown("`psql` is not installed to ask".to_string());
    };
    let said = |bytes: &[u8]| String::from_utf8_lossy(bytes).trim().to_string();
    if out.status.success() {
        return match said(&out.stdout).as_str() {
            "on" => ReadOnlyCheck::Holds,
            _ => ReadOnlyCheck::Writable,
        };
    }
    let err = said(&out.stderr);
    let line = err.lines().next().unwrap_or_default().to_string();
    if err.contains("unsupported startup parameter in options") {
        ReadOnlyCheck::Refused(line)
    } else {
        ReadOnlyCheck::Unknown(line)
    }
}

/// The one argument psql is handed: the service block, plus an explicit
/// `dbname` when another database on the same server was asked for. Keywords
/// given here win over the service file's own, verified against a real server,
/// and it is the only place a database can be swapped - `-d` and the bare
/// positional land in libpq's dbname and username slots instead.
pub fn conninfo(name: &str, db: Option<&str>) -> String {
    match db {
        None => format!("service={}", quote(name)),
        Some(db) => format!("service={} dbname={}", quote(name), quote(db)),
    }
}

/// libpq conninfo quoting: single quotes around a value with whitespace in it,
/// a backslash before a quote or a backslash.
fn quote(v: &str) -> String {
    if !v.is_empty() && !v.contains(|c: char| c.is_whitespace() || c == '\'' || c == '\\') {
        return v.to_string();
    }
    let mut out = String::from("'");
    for c in v.chars() {
        if c == '\'' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('\'');
    out
}

pub fn list() -> Vec<Conn> {
    list_in(&service_path())
}

pub fn list_in(path: &std::path::Path) -> Vec<Conn> {
    ini::read(path)
        .into_iter()
        .map(|s| Conn {
            engine: Engine::Pg,
            host: s.get("host").unwrap_or_default().to_string(),
            port: s.get("port").unwrap_or_default().to_string(),
            database: s.get("dbname").unwrap_or_default().to_string(),
            user: s.get("user").unwrap_or_default().to_string(),
            extra: s.rest(&KNOWN),
            name: s.name,
        })
        .collect()
}

pub fn save(original: Option<&str>, nc: &NewConn) -> Result<()> {
    let mut keys: Vec<(String, String)> = Vec::new();
    for (k, v) in [
        ("host", &nc.host),
        ("port", &nc.port),
        ("dbname", &nc.database),
        ("user", &nc.user),
    ] {
        let v = v.trim();
        if !v.is_empty() {
            keys.push((k.to_string(), v.to_string()));
        }
    }
    keys.extend(nc.extra.iter().cloned());
    ini::upsert(&service_path(), original, nc.name.trim(), &keys)
}

pub fn delete(name: &str) -> Result<()> {
    ini::remove(&service_path(), name)
}

/// The non-interactive question that makes a failed connect explain itself:
/// one `select 1` over the same service, with a deadline so a black-holed host
/// answers in seconds instead of hanging the probe.
pub fn probe_argv(c: &Conn, s: &crate::settings::Settings) -> (Vec<String>, Vec<(String, String)>) {
    let mut argv = c.connect_argv(s);
    argv.push("-w".into()); // never prompt: an unanswerable prompt is the bug we are diagnosing
    argv.push("-tAc".into());
    argv.push("select 1".into());
    let env = vec![("PGCONNECT_TIMEOUT".to_string(), s.probe_timeout.to_string())];
    (argv, env)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_options(options: &str) -> Vec<(String, String)> {
        vec![
            ("sslmode".to_string(), "require".to_string()),
            ("options".to_string(), options.to_string()),
        ]
    }

    fn options(extra: &[(String, String)]) -> Option<&str> {
        extra
            .iter()
            .find(|(k, _)| k == "options")
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn read_only_keeps_every_other_option_and_drops_an_empty_key() {
        let mut extra = with_options("-c statement_timeout=5000");
        set_read_only(&mut extra, true);
        assert!(read_only(&extra), "switching it on must read back as on");
        assert!(
            options(&extra).is_some_and(|o| o.contains("statement_timeout=5000")),
            "a setting somebody wrote by hand survives: {extra:?}"
        );

        set_read_only(&mut extra, false);
        assert!(!read_only(&extra));
        assert_eq!(options(&extra), Some("-c statement_timeout=5000"));

        let mut only = with_options("-c default_transaction_read_only=on");
        set_read_only(&mut only, false);
        assert_eq!(
            options(&only),
            None,
            "an emptied `options` key goes: {only:?}"
        );
        assert_eq!(only.len(), 1, "and every other key stays: {only:?}");
    }

    #[test]
    fn every_spelling_and_boolean_the_server_reads_as_on_counts_as_read_only() {
        for on in [
            "-c default_transaction_read_only=on",
            "-cdefault_transaction_read_only=on",
            "--default-transaction-read-only=on",
            "-c DEFAULT_TRANSACTION_READ_ONLY=on",
            "-c default_transaction_read_only=t",
            "-c default_transaction_read_only=y",
            "-c default_transaction_read_only=1",
        ] {
            assert!(read_only(&with_options(on)), "the server enforces `{on}`");
        }
        for off in [
            "-c default_transaction_read_only=off",
            "-c default_transaction_read_only=on -c default_transaction_read_only=off",
            "-c statement_timeout=5000",
        ] {
            assert!(
                !read_only(&with_options(off)),
                "the server allows writes with `{off}`"
            );
        }
    }

    #[test]
    fn service_and_database_names_are_quoted_the_way_libpq_reads_them() {
        assert_eq!(conninfo("prod", None), "service=prod");
        assert_eq!(conninfo("prod", Some("app")), "service=prod dbname=app");
        assert_eq!(
            conninfo("my db", Some("it's")),
            r"service='my db' dbname='it\'s'",
            "a space or a quote would otherwise split libpq's parse"
        );
    }
}
