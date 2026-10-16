//! MySQL and MariaDB. Connections live in `~/.my.cnf` as `[clientNAME]` option
//! groups, which `mysql --defaults-group-suffix=NAME` reads on top of the plain
//! `[client]` group: the same "point the client at a named block" handoff the
//! Postgres service file gives us.
//!
//! Named groups on purpose, never `[client]` itself: that one is read by every
//! MySQL tool on the machine, so writing a host into it would silently redirect
//! `mysqldump` and everything else.
//!
//! Unlike Postgres, the password lives in the group rather than a file of its
//! own, so `~/.my.cnf` is chmod 0600 on every write. That is also why nothing
//! here ever passes `-p<password>`: MySQL itself warns that the command line is
//! visible to every user on the box.

use super::{Conn, Engine, NewConn};
use crate::ini;
use anyhow::Result;
use std::path::PathBuf;

/// The keys the wizard owns. `password` is deliberately in the list: it is
/// carried by the Credentials tab, not by a connection edit, so a rewrite here
/// must not resurrect an old one, and `rest()` must not hand it back as an extra.
const KNOWN: [&str; 5] = ["host", "port", "database", "user", "password"];

/// The prefix that makes a group a `--defaults-group-suffix` target.
const PREFIX: &str = "client";

pub fn cnf_path() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".my.cnf")
}

/// The group a connection called `prod` lives in.
pub fn group_of(name: &str) -> String {
    format!("{PREFIX}{name}")
}

/// The connection name a group belongs to, or `None` for `[client]` itself and
/// for every group that is not ours (`[mysqld]`, `[mysqldump]`).
pub fn name_of(group: &str) -> Option<String> {
    let rest = group.strip_prefix(PREFIX)?;
    (!rest.is_empty()).then(|| rest.to_string())
}

pub fn list() -> Vec<Conn> {
    list_in(&cnf_path())
}

pub fn list_in(path: &std::path::Path) -> Vec<Conn> {
    ini::read(path)
        .into_iter()
        .filter_map(|s| {
            let name = name_of(&s.name)?;
            Some(Conn {
                engine: Engine::MySql,
                name,
                host: s.get("host").unwrap_or_default().to_string(),
                port: s.get("port").unwrap_or_default().to_string(),
                database: s.get("database").unwrap_or_default().to_string(),
                user: s.get("user").unwrap_or_default().to_string(),
                extra: s.rest(&KNOWN),
            })
        })
        .collect()
}

/// Does this connection already carry a password in its group? The value is
/// never read out, only its presence: that is all the Credentials tab shows.
pub fn has_password(name: &str) -> bool {
    ini::read(&cnf_path())
        .iter()
        .any(|s| s.name == group_of(name) && s.get("password").is_some())
}

/// The group's password, for usql alone: its driver never reads `~/.my.cnf`,
/// so a usql session has nothing to log in with unless it is handed this,
/// which is why it is one of the two reads of a stored password in the crate.
/// Read the way mysql reads it (`option_read`).
pub fn password(name: &str) -> Option<String> {
    let raw = ini::read(&cnf_path())
        .into_iter()
        .find(|s| s.name == group_of(name))?
        .get("password")?
        .to_string();
    Some(option_read(&raw))
}

/// An option value as mysql reads it, checked against the real client's
/// `--print-defaults`: matching quotes are dropped and keep what is inside
/// them, an unquoted value ends at `#`, and `\b \t \n \r \s \\ \" \'` are
/// escapes, while a backslash before anything else stays a backslash.
fn option_read(raw: &str) -> String {
    let quoted = ['"', '\'']
        .into_iter()
        .find(|q| raw.len() >= 2 && raw.starts_with(*q) && raw.ends_with(*q));
    let body = match quoted {
        Some(_) => &raw[1..raw.len() - 1],
        None => raw.split('#').next().unwrap_or_default().trim(),
    };
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('b') => out.push('\u{8}'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('s') => out.push(' '),
            Some(e @ ('\\' | '"' | '\'')) => out.push(e),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// `password` written so mysql reads it back unchanged: as it is when nothing
/// in it means anything to the option-file reader, else in double quotes with
/// `\` and `"` escaped, which keeps a `#`, a quote and edge spaces intact.
fn option_written(password: &str) -> String {
    let plain = !password.contains(['#', '"', '\'', '\\']) && password.trim() == password;
    if plain {
        return password.to_string();
    }
    format!(
        "\"{}\"",
        password.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

/// The connection as a usql URL, which carries no password: the database is
/// the path, so `/db` and `\c <db>` can aim it anywhere.
pub fn usql_url(c: &Conn, db: Option<&str>) -> String {
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
    format!(
        "mysql://{user}{host}:{}/{}",
        c.port_or_default(),
        super::percent_encode(db.unwrap_or(&c.database), "")
    )
}

/// The group's password in a fresh passfile for `USQLPASS`, or `None` when
/// the group has none or it cannot be written in that format.
pub fn usql_passfile(c: &Conn) -> Result<Option<super::RunFile>> {
    let Some(line) = password(&c.name).and_then(|p| super::mssql::passfile_line(c, &p).ok()) else {
        return Ok(None);
    };
    super::RunFile::write(&line).map(Some)
}

/// Why the group's password is not handed to usql, when it has one that
/// cannot be: usql then logs in with none and the server refuses.
pub fn usql_password_note(c: &Conn) -> Option<String> {
    if !has_password(&c.name) {
        return None;
    }
    let reason = super::mssql::passfile_line(c, &password(&c.name)?).err()?;
    Some(format!(
        "the password for `{}` in ~/.my.cnf is not handed to usql: {reason}",
        c.name
    ))
}

/// The `--cset` flags that make `\c <db>` reach each database by name, asked
/// of the server with `show databases` through the same `secret` env, and
/// empty when it cannot be asked without a prompt.
pub fn usql_csets(
    c: &Conn,
    s: &crate::settings::Settings,
    db: Option<&str>,
    secret: &[(String, String)],
) -> Vec<String> {
    let out = std::process::Command::new("usql")
        .arg(format!("{}?timeout={}s", usql_url(c, db), s.probe_timeout))
        .args(["-X", "-w", "-A", "-t", "-c", "show databases"])
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
                format!("{n}={}", usql_url(c, Some(n))),
            ]
        })
        .collect()
}

pub fn save(original: Option<&str>, nc: &NewConn) -> Result<()> {
    let path = cnf_path();
    let group = group_of(nc.name.trim());
    let from = original.map(group_of);

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
    // An edit must not drop the password that was already in the group; it is
    // not a field this wizard shows, so it is preserved rather than rewritten.
    let existing = from.as_deref().unwrap_or(&group);
    if let Some(sec) = ini::read(&path).into_iter().find(|s| s.name == existing)
        && let Some(pw) = sec.get("password")
    {
        keys.push(("password".to_string(), pw.to_string()));
    }
    keys.extend(nc.extra.iter().cloned());
    ini::upsert(&path, from.as_deref(), &group, &keys)
}

pub fn delete(name: &str) -> Result<()> {
    ini::remove(&cnf_path(), &group_of(name))
}

/// Write or clear the group's password. The only place in easysql that touches
/// a secret, and it goes straight into the file the client reads.
pub fn set_password(name: &str, password: Option<&str>) -> Result<()> {
    let written = password.map(option_written);
    ini::set_key(&cnf_path(), &group_of(name), "password", written.as_deref())
}

pub fn probe_argv(c: &Conn, s: &crate::settings::Settings) -> (Vec<String>, Vec<(String, String)>) {
    let mut argv = c.connect_argv(s);
    argv.push(format!("--connect-timeout={}", s.probe_timeout));
    argv.push("-e".into());
    argv.push("select 1".into());
    (argv, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn a_mysql_password_reads_back_exactly_as_typed() {
        // Each of these was read back through mysql's own `--print-defaults`:
        // unquoted, it ends the value at `#`, trims it and reads `\` escapes.
        for typed in ["plain", "ab#cd", r"a\b", "x\"y", "a'b", "  lead ", r"q\qz"] {
            let written = option_written(typed);
            assert_eq!(
                option_read(&written),
                typed,
                "saved as `password={written}`"
            );
        }
    }

    #[test]
    fn client_and_foreign_groups_are_never_ours() {
        // `[client]` is read by every MySQL tool on the machine, so writing a
        // host into it would silently redirect mysqldump and everything else.
        assert_eq!(name_of("client"), None);
        assert_eq!(name_of("mysqld"), None);
        assert_eq!(name_of("mysqldump"), None);
        // Ours round-trip.
        assert_eq!(name_of(&group_of("prod")), Some("prod".to_string()));
    }

    #[test]
    fn listing_skips_every_group_that_is_not_ours() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("easysql-mycnf-{}-{stamp}", std::process::id()));
        fs::write(
            &path,
            "\
[client]
host=shared.example.com

[mysqld]
port=3306

[clientprod]
host=db.example.com
port=3306
database=app
user=me
connect_timeout=5
",
        )
        .unwrap();

        let conns = list_in(&path);
        let _ = fs::remove_file(&path);

        assert_eq!(conns.len(), 1, "only [clientNAME] groups are connections");
        assert_eq!(conns[0].name, "prod");
        assert_eq!(conns[0].host, "db.example.com");
        // A key no wizard field owns is carried through rather than dropped.
        assert_eq!(
            conns[0].extra,
            vec![("connect_timeout".to_string(), "5".to_string())]
        );
    }
}
