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
