//! SQLite. There is no server, no user and no password: a connection is a path
//! to a file, and `sqlite3 <path>` is the whole story.
//!
//! It is also the one engine whose client has no config file of its own, so
//! this is the only list easysql owns rather than borrows. It lives in the
//! config dir in the same INI shape as the other two, so it reads the same way
//! by hand.

use super::{Conn, Engine, NewConn};
use crate::ini;
use anyhow::Result;
use std::path::PathBuf;

const KNOWN: [&str; 1] = ["path"];

pub fn store_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("easysql")
        .join("sqlite.conf")
}

/// Whether this connection opens its file with `-readonly`, which sqlite3
/// enforces itself: a write fails with "attempt to write a readonly database".
pub fn read_only(extra: &[(String, String)]) -> bool {
    extra
        .iter()
        .any(|(k, v)| k.eq_ignore_ascii_case("readonly") && v == "yes")
}

pub fn list() -> Vec<Conn> {
    ini::read(&store_path())
        .into_iter()
        .map(|s| Conn {
            engine: Engine::Sqlite,
            database: s.get("path").unwrap_or_default().to_string(),
            host: String::new(),
            port: String::new(),
            user: String::new(),
            extra: s.rest(&KNOWN),
            name: s.name,
        })
        .collect()
}

pub fn save(original: Option<&str>, nc: &NewConn) -> Result<()> {
    let mut keys = vec![("path".to_string(), nc.database.trim().to_string())];
    keys.extend(nc.extra.iter().cloned());
    ini::upsert(&store_path(), original, nc.name.trim(), &keys)
}

pub fn delete(name: &str) -> Result<()> {
    ini::remove(&store_path(), name)
}
