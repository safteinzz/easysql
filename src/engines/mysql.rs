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
    ini::set_key(&cnf_path(), &group_of(name), "password", password)
}

pub fn probe_argv(c: &Conn, s: &crate::settings::Settings) -> (Vec<String>, Vec<(String, String)>) {
    let mut argv = c.connect_argv(s);
    argv.push(format!("--connect-timeout={}", s.probe_timeout));
    argv.push("-e".into());
    argv.push("select 1".into());
    (argv, Vec::new())
}
