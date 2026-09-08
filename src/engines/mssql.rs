//! Microsoft SQL Server. The odd one out, twice over.
//!
//! There is no file both clients read. The modern `go-sqlcmd` keeps contexts in
//! `~/.sqlcmd/sqlconfig`, which is YAML and which the classic ODBC `sqlcmd` from
//! `mssql-tools18` ignores completely; the classic one has no config file at
//! all. So this is the second engine, after SQLite, whose list easysql owns
//! rather than borrows - kept in the config dir in the same INI shape as the
//! rest, and turned into plain `-S/-d/-U` flags that *both* builds understand.
//!
//! And there is no password file. `sqlconfig` stores an obfuscated copy that
//! only go-sqlcmd reads, and the alternative, `SQLCMDPASSWORD`, would mean
//! holding a secret and handing it to a child process, which this crate does
//! not do. Omitting `-P` makes sqlcmd prompt for the password itself, on the
//! terminal we already hand over, which is also Microsoft's own advice: "Using
//! -P is insecure. Avoid giving the password on the command line."

use super::{Conn, Engine, NewConn};
use crate::ini;
use anyhow::Result;
use std::path::PathBuf;

const KNOWN: [&str; 5] = ["host", "port", "database", "user", "trust_cert"];

pub fn store_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("easysql")
        .join("mssql.conf")
}

pub fn list() -> Vec<Conn> {
    ini::read(&store_path())
        .into_iter()
        .map(|s| Conn {
            engine: Engine::MsSql,
            host: s.get("host").unwrap_or_default().to_string(),
            port: s.get("port").unwrap_or_default().to_string(),
            database: s.get("database").unwrap_or_default().to_string(),
            user: s.get("user").unwrap_or_default().to_string(),
            // `trust_cert` is ours and is rebuilt from the wizard's choice on
            // every save, so it must not come back as a leftover extra too.
            extra: s.rest(&KNOWN),
            name: s.name,
        })
        .collect()
}

/// Whether this connection waives certificate validation. Kept in `extra` like
/// Postgres' `sslmode`, so the `Conn` struct stays engine-blind.
pub fn trusts_cert(c: &Conn) -> bool {
    c.extra
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("trust_cert") && v == "yes")
}

/// The connection flags, which are the whole handoff for this engine. `db`
/// overrides the saved database when one was asked for.
///
/// `-S host,port` is sqlcmd's own syntax: a comma, not a colon. `-P` is never
/// passed, so sqlcmd asks for the password itself.
pub fn flags(c: &Conn, db: Option<&str>) -> Vec<String> {
    let host = if c.host.is_empty() {
        "localhost"
    } else {
        &c.host
    };
    let mut argv = vec!["-S".to_string(), format!("{host},{}", c.port_or_default())];
    let database = db.unwrap_or(&c.database);
    if !database.is_empty() {
        argv.push("-d".into());
        argv.push(database.to_string());
    }
    if !c.user.is_empty() {
        argv.push("-U".into());
        argv.push(c.user.clone());
    }
    // ODBC driver 18 encrypts and validates the certificate by default, so a
    // self-signed server refuses until you say you trust it. A choice, not a
    // default: waiving validation silently would weaken the connection for you.
    if trusts_cert(c) {
        argv.push("-C".into());
    }
    argv
}

pub fn save(original: Option<&str>, nc: &NewConn) -> Result<()> {
    let mut keys: Vec<(String, String)> = Vec::new();
    for (k, v) in [
        ("host", &nc.host),
        ("port", &nc.port),
        ("database", &nc.database),
        ("user", &nc.user),
    ] {
        let v = v.trim();
        if !v.is_empty() {
            keys.push((k.to_string(), v.to_string()));
        }
    }
    keys.extend(nc.extra.iter().cloned());
    ini::upsert(&store_path(), original, nc.name.trim(), &keys)
}

pub fn delete(name: &str) -> Result<()> {
    ini::remove(&store_path(), name)
}

/// The non-interactive question after a failed open. `-l` is the login timeout
/// in seconds, and there is no "never prompt" flag to add: omitting `-P` with
/// no `SQLCMDPASSWORD` set makes sqlcmd ask, so the probe supplies an empty one
/// through the environment purely to stop it blocking on a prompt nobody can
/// answer. That empty value is not a stored password and is never read back.
pub fn probe_argv(c: &Conn, s: &crate::settings::Settings) -> (Vec<String>, Vec<(String, String)>) {
    let mut argv = c.connect_argv(s);
    argv.push("-l".into());
    argv.push(s.probe_timeout.to_string());
    argv.push("-Q".into());
    argv.push("select 1".into());
    (argv, vec![("SQLCMDPASSWORD".to_string(), String::new())])
}
