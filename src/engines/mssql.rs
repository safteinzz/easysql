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
//! hand it over in `SQLCMDPASSWORD`, or to usql in a passfile that lasts one
//! session, the one place this crate holds a secret, so it is off until the
//! user turns it on.

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
    // Read whatever the file's mode, unlike `password`: a move hands the
    // secret to nobody, and refusing it would leave it under the old name for
    // the next connection called that.
    let kept = ini::read(&pass_path())
        .into_iter()
        .find(|s| s.name == from)
        .and_then(|s| s.get("password").map(str::to_string));
    match kept {
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

/// The connection as a usql URL, which carries no password. `encrypt=true` is
/// what keeps the certificate validated as ODBC driver 18 does: without it
/// go-mssqldb accepts any certificate. `timeout` is a dial deadline in seconds.
pub fn usql_url(c: &Conn, db: Option<&str>, timeout: Option<u64>) -> String {
    let host = if c.host.is_empty() {
        "localhost"
    } else {
        &c.host
    };
    let host = match host.contains(':') && !host.starts_with('[') {
        true => format!("[{host}]"),
        false => host.to_string(),
    };
    let user = match c.user.is_empty() {
        true => String::new(),
        false => format!("{}@", super::percent_encode(&c.user, "")),
    };
    let mut query = Vec::new();
    let database = db.unwrap_or(&c.database);
    if !database.is_empty() {
        query.push(format!("database={}", super::percent_encode(database, "")));
    }
    query.push("encrypt=true".to_string());
    if trusts_cert(c) {
        query.push("TrustServerCertificate=true".to_string());
    }
    if let Some(t) = timeout {
        query.push(format!("dial%20timeout={t}"));
    }
    format!(
        "sqlserver://{user}{host}:{}?{}",
        c.port_or_default(),
        query.join("&")
    )
}

/// What follows `usql` in a connect: the URL, then `-W` unless a kept password
/// will be handed over, because usql never asks on its own and logs in with an
/// empty password instead.
pub fn usql_args(c: &Conn, s: &crate::settings::Settings, db: Option<&str>) -> Vec<String> {
    let mut argv = vec![usql_url(c, db, None)];
    let handed =
        s.mssql_passwords && password(&c.name).is_some_and(|p| passfile_line(c, &p).is_ok());
    if !handed {
        argv.push("-W".into());
    }
    argv
}

/// The `--cset` flags that make `\c <db>` reach every database on the server
/// by name, since usql's `\c` otherwise takes only a whole URL. The server is
/// asked just before the session opens, through the same `secret` env and
/// without a prompt (`-w`), so this is empty whenever it cannot be asked and
/// the session opens without them. usql refuses to start over a name that is
/// not letters, digits and `_`, so any other database is left out.
pub fn usql_csets(
    c: &Conn,
    s: &crate::settings::Settings,
    db: Option<&str>,
    secret: &[(String, String)],
) -> Vec<String> {
    let out = std::process::Command::new("usql")
        .arg(usql_url(c, db, Some(s.probe_timeout)))
        .args([
            "-X",
            "-w",
            "-A",
            "-t",
            "-c",
            "select name from sys.databases",
        ])
        .envs(secret.iter().map(|(k, v)| (k, v)))
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    let Some(out) = out.ok().filter(|o| o.status.success()) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|n| super::is_cset_name(n))
        .flat_map(|n| {
            [
                "--cset".to_string(),
                format!("{n}={}", usql_url(c, Some(n), None)),
            ]
        })
        .collect()
}

/// The one line of a usql passfile for this connection. Its format has no
/// escaping, ends a line at `#`, trims the line and refuses an empty field, so
/// a value it cannot carry is refused here with the reason. The database is
/// `*` because usql matches on protocol, host and port alone and then logs in
/// as the line's user, which is why the file holds this one line and nothing
/// else. Also MySQL's, whose protocol word is `mysql`.
pub(super) fn passfile_line(c: &Conn, password: &str) -> std::result::Result<String, &'static str> {
    let protocol = match c.engine {
        Engine::MySql => "mysql",
        _ => "sqlserver",
    };
    let host = if c.host.is_empty() {
        "localhost"
    } else {
        &c.host
    };
    if c.user.is_empty() {
        return Err("it names no user, which usql's password file needs");
    }
    let unfit = |v: &str| v.contains([':', '#', '\n', '\r']);
    if unfit(host) || unfit(&c.user) || unfit(password) {
        return Err(
            "its host, user or password holds a `:`, a `#` or a line break, which usql's password file cannot carry",
        );
    }
    if password.trim_end() != password {
        return Err("its password ends in a space, which usql's password file trims");
    }
    Ok(format!(
        "{protocol}:{host}:{}:*:{}:{password}\n",
        c.port_or_default(),
        c.user
    ))
}

/// Why usql gets no password, for one line on stderr before it runs. usql
/// asks only at the start of a session, never under `-c` or `-f`, so a
/// `one_shot` without a kept password names the commands that keep one.
/// `None` when the password is handed over, or a session will ask for it.
pub fn usql_password_note(
    c: &Conn,
    s: &crate::settings::Settings,
    one_shot: bool,
) -> Option<String> {
    if !c.engine.uses_usql(s) {
        return None;
    }
    if c.engine == Engine::MySql {
        return super::mysql::usql_password_note(c);
    }
    if c.engine != Engine::MsSql {
        return None;
    }
    let kept = password(&c.name).filter(|_| s.mssql_passwords);
    match (kept.map(|p| passfile_line(c, &p)), one_shot) {
        (Some(Ok(_)), _) => None,
        (Some(Err(reason)), false) => Some(format!(
            "the kept password for `{}` is not handed to usql, so it asks: {reason}",
            c.name
        )),
        (Some(Err(reason)), true) => Some(format!(
            "the kept password for `{}` is not handed to usql, which cannot ask in a one-shot: {reason}",
            c.name
        )),
        (None, false) => None,
        (None, true) => Some(format!(
            "usql cannot ask for a password in a one-shot, so keep one: {}`esql pw set {}`",
            match s.mssql_passwords {
                true => "",
                false => "`esql set mssql_passwords on`, then ",
            },
            c.name
        )),
    }
}

/// Write the kept password for this connection into a fresh passfile, for
/// `USQLPASS`. `None` when no password is kept, or it cannot be written in
/// that format, in which case `usql_args` passed `-W`.
pub fn usql_passfile(c: &Conn) -> Result<Option<super::RunFile>> {
    let Some(line) = password(&c.name).and_then(|p| passfile_line(c, &p).ok()) else {
        return Ok(None);
    };
    super::RunFile::write(&line).map(Some)
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

/// The non-interactive question after a failed open, with what has to stay
/// alive until it has run. `-l` is sqlcmd's login timeout in seconds, and
/// sqlcmd has no "never prompt" flag: omitting `-P` with no `SQLCMDPASSWORD`
/// set makes it ask, so the probe hands over the stored password when the
/// setting allows one, and an empty one otherwise purely to stop sqlcmd
/// blocking on a prompt nobody can answer. usql has one, `-w`.
pub fn probe_argv(c: &Conn, s: &crate::settings::Settings) -> (Vec<String>, super::Secret) {
    let secret = c
        .secret_env(s)
        .unwrap_or_else(|_| super::Secret::plain(Vec::new()));
    if c.engine.uses_usql(s) {
        let argv = vec![
            "usql".to_string(),
            usql_url(c, None, Some(s.probe_timeout)),
            "-w".into(),
            "-c".into(),
            "select 1".into(),
        ];
        return (argv, secret);
    }
    let mut argv = c.connect_argv(s);
    argv.push("-l".into());
    argv.push(s.probe_timeout.to_string());
    argv.push("-Q".into());
    argv.push("select 1".into());
    let mut secret = secret;
    if secret.env.is_empty() {
        secret
            .env
            .push(("SQLCMDPASSWORD".to_string(), String::new()));
    }
    (argv, secret)
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
