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
//! only go-sqlcmd reads, so by default easysql keeps nothing and sqlcmd prompts
//! on the terminal we already hand over - Microsoft's own advice over `-P`:
//! "Using -P is insecure. Avoid giving the password on the command line." The
//! `mssql_passwords` setting lets easysql keep one in `~/.esqlpass` instead and
//! hand it over in `SQLCMDPASSWORD`, the one place this crate holds a secret,
//! so it is off until the user turns it on.

use super::{Conn, Engine, NewConn};
use crate::ini;
use anyhow::Result;
use std::path::PathBuf;

const KNOWN: [&str; 4] = ["host", "port", "database", "user"];

pub fn store_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("easysql")
        .join("mssql.conf")
}

pub fn list() -> Vec<Conn> {
    list_in(&store_path())
}

/// Where SQL Server passwords live when the setting allows it: one `[name]`
/// with a `password` key per connection, written through `ini`, which makes
/// every file it writes 0600. In the home folder beside `~/.pgpass` and
/// `~/.my.cnf`, where anyone who knows those looks for it, and deliberately not
/// in `~/.config`, which is what people sync, back up and commit.
pub fn pass_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".esqlpass")
}

/// Whether nobody but the owner can read the password file - the same test
/// libpq applies to `.pgpass` before it will use it. easysql writes it 0600;
/// this is for a copy somebody made or loosened by hand.
pub fn pass_is_private() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(pass_path()).is_ok_and(|m| m.permissions().mode() & 0o077 == 0)
    }
    #[cfg(not(unix))]
    true
}

/// The one read of a stored password anywhere in this crate, for the
/// environment of the child that needs it and nothing else. A file others can
/// read is not used at all, so sqlcmd asks instead.
pub fn password(name: &str) -> Option<String> {
    if !pass_is_private() {
        return None;
    }
    ini::read(&pass_path())
        .into_iter()
        .find(|s| s.name == name)
        .and_then(|s| s.get("password").map(str::to_string))
        .filter(|p| !p.is_empty())
}

pub fn has_password(name: &str) -> bool {
    ini::read(&pass_path())
        .iter()
        .any(|s| s.name == name && s.get("password").is_some())
}

/// Save it, or with `None` forget it.
pub fn set_password(name: &str, password: Option<&str>) -> Result<()> {
    match password {
        Some(p) => ini::upsert(
            &pass_path(),
            None,
            name,
            &[("password".to_string(), p.to_string())],
        ),
        None if has_password(name) => ini::remove(&pass_path(), name),
        None => Ok(()),
    }
}

/// A rename moves the password with the connection, since it is keyed on the name.
pub fn rename_password(from: &str, to: &str) -> Result<()> {
    match password(from) {
        Some(p) => ini::upsert(&pass_path(), Some(from), to, &[("password".to_string(), p)]),
        None => Ok(()),
    }
}

pub fn list_in(path: &std::path::Path) -> Vec<Conn> {
    ini::read(path)
        .into_iter()
        .map(|s| Conn {
            engine: Engine::MsSql,
            host: s.get("host").unwrap_or_default().to_string(),
            port: s.get("port").unwrap_or_default().to_string(),
            database: s.get("database").unwrap_or_default().to_string(),
            user: s.get("user").unwrap_or_default().to_string(),
            // `trust_cert` stays in here, the way `sslmode` does for Postgres:
            // `flags` and the form both read it from `extra`, and the wizard
            // strips it before writing its own answer, so it is never doubled.
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
/// no `SQLCMDPASSWORD` set makes sqlcmd ask, so the probe hands over the stored
/// password when the setting allows one, and an empty one otherwise purely to
/// stop sqlcmd blocking on a prompt nobody can answer.
pub fn probe_argv(c: &Conn, s: &crate::settings::Settings) -> (Vec<String>, Vec<(String, String)>) {
    let mut argv = c.connect_argv(s);
    argv.push("-l".into());
    argv.push(s.probe_timeout.to_string());
    argv.push("-Q".into());
    argv.push("select 1".into());
    let env = c
        .secret_env(s)
        .into_iter()
        .next()
        .unwrap_or_else(|| ("SQLCMDPASSWORD".to_string(), String::new()));
    (argv, vec![env])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn a_saved_trust_cert_reaches_the_sqlcmd_argv() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("easysql-mssql-{}-{stamp}.conf", std::process::id()));
        std::fs::write(
            &path,
            "[selfsigned]\nhost=db.example.com\ndatabase=app\nuser=sa\ntrust_cert=yes\n\n[strict]\nhost=db.example.com\n",
        )
        .unwrap();
        let conns = list_in(&path);
        let _ = std::fs::remove_file(&path);

        let argv = |name: &str| {
            let c = conns.iter().find(|c| c.name == name).expect(name);
            flags(c, None)
        };
        assert!(
            argv("selfsigned").contains(&"-C".to_string()),
            "a certificate the file says to trust must be trusted when read back: {:?}",
            argv("selfsigned")
        );
        assert!(!argv("strict").contains(&"-C".to_string()));
    }
}
