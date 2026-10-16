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

/// The file as a usql URL. A `file:` URI is the one form whose `mode` the
/// driver honours: `ro` refuses every write, and `rw` refuses a missing file
/// where a bare path would create an empty one. The path is percent-encoded,
/// or a `#` or `?` in it would end the path.
pub fn usql_url(path: &str, read_only: bool) -> String {
    let path = ini::expand_tilde(path).to_string_lossy().into_owned();
    let mode = if read_only { "ro" } else { "rw" };
    format!(
        "sqlite3:file:{}?mode={mode}",
        super::percent_encode(&path, "/")
    )
}

/// One argument of a dot-command, which sqlite3 splits on spaces unless it is
/// quoted.
pub fn dot_arg(a: &str) -> String {
    if !a.is_empty() && !a.contains(|c: char| c.is_whitespace() || c == '"') {
        return a.to_string();
    }
    format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\""))
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

/// A typed path as it should be saved: relative to where it was typed, since
/// the client is later started from wherever `esql` happens to run, and
/// sqlite3 creates an empty database at a path that does not exist. `~` and
/// absolute paths are kept as written.
pub fn absolute(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() || path.starts_with('~') || std::path::Path::new(path).is_absolute() {
        return path.to_string();
    }
    match std::env::current_dir() {
        Ok(dir) => dir.join(path).to_string_lossy().into_owned(),
        Err(_) => path.to_string(),
    }
}

pub fn save(original: Option<&str>, nc: &NewConn) -> Result<()> {
    let mut keys = vec![("path".to_string(), nc.database.trim().to_string())];
    keys.extend(nc.extra.iter().cloned());
    ini::upsert(&store_path(), original, nc.name.trim(), &keys)
}

pub fn delete(name: &str) -> Result<()> {
    ini::remove(&store_path(), name)
}
