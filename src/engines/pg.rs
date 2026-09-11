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
            && name.replace('-', "_") == READ_ONLY
        {
            hits.push((i, span, value.to_ascii_lowercase()));
        }
        i += span;
    }
    hits
}

/// Whether this block makes every session read-only, however that line got
/// there. The last setting wins, the way the server reads them.
pub fn read_only(extra: &[(String, String)]) -> bool {
    extra
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("options"))
        .and_then(|(_, v)| read_only_values(v).pop())
        .is_some_and(|(_, _, v)| matches!(v.as_str(), "on" | "true" | "yes" | "1"))
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

/// The one argument psql is handed: the service block, plus an explicit
/// `dbname` when another database on the same server was asked for. Keywords
/// given here win over the service file's own, verified against a real server,
/// and it is the only place a database can be swapped - `-d` and the bare
/// positional land in libpq's dbname and username slots instead.
pub fn conninfo(name: &str, db: Option<&str>) -> String {
    match db {
        None => format!("service={name}"),
        Some(db) => format!("service={name} dbname={}", quote(db)),
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
