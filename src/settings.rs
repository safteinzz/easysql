//! The handful of choices that are yours rather than the clients', kept in
//! `~/.config/easysql/settings` as `key = value` lines you can read and edit by
//! hand. Nothing here changes what psql, mysql or sqlite3 do; it changes what
//! easysql assumes before you type anything.
//!
//! Every setting has a default that works with no file at all, so a fresh
//! install writes nothing until you change something. An unknown key is left
//! alone rather than dropped, so an older build never eats a newer one's file.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// How the Connections tab is ordered.
#[derive(Clone, Copy, PartialEq)]
pub enum ConnOrder {
    /// Most recently opened first: the database you were in is the one you want.
    Recent,
    /// Grouped by engine, then by name inside each - the order `esql ls` uses.
    /// This used to be called "alphabetical", which is what it is *within* an
    /// engine and misleading about the grouping, so the two are now separate
    /// answers rather than one that quietly does both.
    Engine,
    /// By name alone, ignoring which engine it belongs to.
    Name,
}

pub struct Settings {
    /// Whether to check each server's port in the background.
    pub probe: bool,
    /// How long a probe waits, in seconds. Also the deadline on the `select 1`
    /// that makes a failed connect explain itself.
    pub probe_timeout: u64,
    pub conn_order: ConnOrder,
    /// What runs an interactive Postgres session. `psql`, or a wrapper such as
    /// `pgcli` or `docker exec -it db psql`.
    pub psql_command: String,
    pub mysql_command: String,
    pub sqlite_command: String,
    pub sqlcmd_command: String,
    pub hints: bool,
    /// The ssh host a tunnel wizard offers first, for the bastion you always use.
    pub tunnel_host: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            probe: true,
            probe_timeout: 2,
            conn_order: ConnOrder::Recent,
            psql_command: "psql".into(),
            mysql_command: "mysql".into(),
            sqlite_command: "sqlite3".into(),
            sqlcmd_command: "sqlcmd".into(),
            hints: true,
            tunnel_host: String::new(),
        }
    }
}

/// The two kinds of setting, which are not the same promise: one decides what
/// easysql does, the other only decides what a wizard offers you before you
/// overwrite it.
#[derive(PartialEq, Clone, Copy)]
pub enum Group {
    /// Always in force.
    Behaviour,
    /// Pre-fills a wizard field; every use can still say otherwise.
    Default,
}

impl Group {
    pub fn label(&self) -> &'static str {
        match self {
            Group::Behaviour => "behaviour",
            Group::Default => "defaults",
        }
    }
}

/// One editable line in the Settings tab. `choices` is what makes it a cycled
/// answer rather than a typed one.
pub struct Row {
    pub key: &'static str,
    pub group: Group,
    pub label: &'static str,
    /// What it changes, in the words of the thing it changes.
    pub help: &'static str,
    pub value: String,
    pub default: String,
    pub choices: Option<&'static [&'static str]>,
}

impl Row {
    pub fn is_default(&self) -> bool {
        self.value == self.default
    }
}

const PROBE_CHOICES: &[&str] = &["on", "off"];
const TIMEOUT_CHOICES: &[&str] = &["1", "2", "3", "5", "10"];
const ORDER_CHOICES: &[&str] = &["recent", "engine", "name"];

pub fn path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("easysql")
        .join("settings")
}

pub fn load() -> Settings {
    load_from(&path())
}

/// Parse the file over the defaults. A missing file, an unreadable one or a
/// line that makes no sense all mean "keep the default", never an error: a
/// broken settings file must not stop you opening a database.
pub fn load_from(path: &std::path::Path) -> Settings {
    let mut s = Settings::default();
    let Ok(text) = fs::read_to_string(path) else {
        return s;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        s.set(key.trim(), value.trim());
    }
    s
}

impl Settings {
    /// Apply one `key = value`, ignoring anything we do not recognise.
    pub fn set(&mut self, key: &str, value: &str) {
        match key {
            "probe" => self.probe = value != "off",
            "probe_timeout" => {
                if let Ok(n) = value.parse::<u64>() {
                    self.probe_timeout = n.clamp(1, 60);
                }
            }
            "conn_order" => {
                self.conn_order = match value {
                    // "alphabetical" is the old spelling of what is now
                    // "engine", kept so an existing settings file still means
                    // what it meant before this was split in two.
                    "engine" | "alphabetical" | "alpha" => ConnOrder::Engine,
                    "name" => ConnOrder::Name,
                    _ => ConnOrder::Recent,
                }
            }
            "psql_command" => self.psql_command = non_empty(value, "psql"),
            "mysql_command" => self.mysql_command = non_empty(value, "mysql"),
            "sqlite_command" => self.sqlite_command = non_empty(value, "sqlite3"),
            "sqlcmd_command" => self.sqlcmd_command = non_empty(value, "sqlcmd"),
            "hints" => self.hints = value != "off",
            "tunnel_host" => self.tunnel_host = value.to_string(),
            _ => {}
        }
    }

    /// Put one setting back the way it ships.
    pub fn reset(&mut self, key: &str) {
        let d = Settings::default();
        let value = d.rows().into_iter().find(|r| r.key == key).map(|r| r.value);
        if let Some(value) = value {
            self.set(key, &value);
        }
        // A setting whose default is empty cannot be restored through `set`,
        // which reads an empty value as "leave it alone".
        if key == "tunnel_host" {
            self.tunnel_host = String::new();
        }
    }

    /// Move a cycled setting to its next (or previous) answer. A no-op on a
    /// typed setting, which is edited in a field instead.
    pub fn cycle(&mut self, key: &str, delta: i32) {
        let Some(row) = self.rows().into_iter().find(|r| r.key == key) else {
            return;
        };
        let Some(choices) = row.choices else {
            return;
        };
        let at = choices.iter().position(|c| *c == row.value).unwrap_or(0) as i32;
        let next = (at + delta).rem_euclid(choices.len() as i32) as usize;
        self.set(key, choices[next]);
    }

    /// Everything the Settings tab shows, in the order it shows it.
    pub fn rows(&self) -> Vec<Row> {
        let d = Settings::default();
        // Behaviour first, then the wizard defaults: what easysql always does
        // reads differently from what it merely suggests.
        vec![
            Row {
                key: "probe",
                group: Group::Behaviour,
                label: "Check ports",
                help: "ask each server's port whether it answers, for the up/down dot",
                value: on_off(self.probe).into(),
                default: on_off(d.probe).into(),
                choices: Some(PROBE_CHOICES),
            },
            Row {
                key: "probe_timeout",
                group: Group::Behaviour,
                label: "Check waits",
                help: "seconds a check waits before calling a server down",
                value: self.probe_timeout.to_string(),
                default: d.probe_timeout.to_string(),
                choices: Some(TIMEOUT_CHOICES),
            },
            Row {
                key: "conn_order",
                group: Group::Behaviour,
                label: "List order",
                help: "recent = last opened first · engine = grouped, like esql ls · name = ignoring engine",
                value: match self.conn_order {
                    ConnOrder::Recent => "recent".into(),
                    ConnOrder::Engine => "engine".into(),
                    ConnOrder::Name => "name".into(),
                },
                default: "recent".into(),
                choices: Some(ORDER_CHOICES),
            },
            Row {
                key: "psql_command",
                group: Group::Behaviour,
                label: "Postgres with",
                help: "what opens a postgres session: psql, or a wrapper such as `pgcli`",
                value: self.psql_command.clone(),
                default: d.psql_command.clone(),
                choices: None,
            },
            Row {
                key: "mysql_command",
                group: Group::Behaviour,
                label: "MySQL with",
                help: "what opens a mysql session: mysql, or `mariadb`",
                value: self.mysql_command.clone(),
                default: d.mysql_command.clone(),
                choices: None,
            },
            Row {
                key: "sqlite_command",
                group: Group::Behaviour,
                label: "SQLite with",
                help: "what opens a sqlite file: sqlite3, or a wrapper such as `litecli`",
                value: self.sqlite_command.clone(),
                default: d.sqlite_command.clone(),
                choices: None,
            },
            Row {
                key: "sqlcmd_command",
                group: Group::Behaviour,
                label: "SQL Server with",
                help: "what opens a sql server session: sqlcmd, or a full path under /opt/mssql-tools18/bin",
                value: self.sqlcmd_command.clone(),
                default: d.sqlcmd_command.clone(),
                choices: None,
            },
            Row {
                key: "hints",
                group: Group::Behaviour,
                label: "Prompt hints",
                help: "print how to list tables in that client before handing the terminal over",
                value: on_off(self.hints).into(),
                default: on_off(d.hints).into(),
                choices: Some(&["on", "off"]),
            },
            Row {
                key: "tunnel_host",
                group: Group::Default,
                label: "Tunnel through",
                help: "the ssh host a tunnel offers first, for the bastion you always use",
                value: self.tunnel_host.clone(),
                default: d.tunnel_host.clone(),
                choices: None,
            },
        ]
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&path())
    }

    /// Write every setting, defaults included, so the file doubles as the list
    /// of what can be changed.
    pub fn save_to(&self, path: &std::path::Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let mut body = String::from(
            "# easysql settings - `key = value`, one per line. Delete a line\n# to go back to its default. The Settings tab edits this file.\n\n",
        );
        for row in self.rows() {
            body.push_str(&format!("{} = {}\n", row.key, row.value));
        }
        fs::write(path, body).with_context(|| format!("writing {}", path.display()))
    }
}

fn on_off(v: bool) -> &'static str {
    if v { "on" } else { "off" }
}

fn non_empty(value: &str, fallback: &str) -> String {
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_order_separates_grouping_by_engine_from_sorting_by_name() {
        // These used to be one answer called "alphabetical", which grouped by
        // engine without saying so. Somebody who wants plain alphabetical and
        // somebody who wants engines kept together now get different orders.
        let mut s = Settings::default();
        s.set("conn_order", "name");
        assert!(matches!(s.conn_order, ConnOrder::Name));
        s.set("conn_order", "engine");
        assert!(matches!(s.conn_order, ConnOrder::Engine));

        // The old spelling still means what it always meant, so an existing
        // settings file does not silently change somebody's list.
        s.set("conn_order", "alphabetical");
        assert!(matches!(s.conn_order, ConnOrder::Engine));

        // Anything unrecognised falls back to the default rather than erroring.
        s.set("conn_order", "nonsense");
        assert!(matches!(s.conn_order, ConnOrder::Recent));
    }
}
