//! Which tunnel a connection needs, remembered across reboots.
//!
//! A forward is a process, so it dies with the machine, and a connection that
//! was repointed at `127.0.0.1` is then pointing at nothing. Before this, the
//! only cure was to remember what you built last week and rebuild it by hand,
//! which is exactly the kind of thing this tool exists to stop.
//!
//! It lives in easysql's own file rather than the connection's, and that is
//! forced rather than chosen: libpq validates every keyword in
//! `~/.pg_service.conf` and refuses the whole file over one it does not know
//! (`syntax error in service file`), so a `via=` key there would break the
//! connection for psql, for pgAdmin and for every driver on the machine. Ours
//! is the only safe place to put it.
//!
//! Sections are keyed by `Conn::key()` (`pg:raspi`), because two engines are
//! allowed to have a connection of the same name.

use crate::ini;
use anyhow::Result;
use std::path::PathBuf;

/// The forward a connection depends on, in the four parts `ssh -L` needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Via {
    /// The ssh host to hop through, as named in `~/.ssh/config`.
    pub host: String,
    /// The database address *as the ssh host sees it*, usually `127.0.0.1`.
    pub target: String,
    /// The port on that far side.
    pub port: String,
    /// The port opened here, which is what the connection points at.
    pub local: String,
}

impl Via {
    pub fn spec(&self) -> String {
        format!("{}:{}:{}", self.local, self.target, self.port)
    }

    /// The command that rebuilds it, for a preview or a status line.
    pub fn command(&self) -> String {
        format!("ssh -N -L {} {}", self.spec(), self.host)
    }
}

pub fn store_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
        .join("easysql")
        .join("vias")
}

fn load_from(path: &std::path::Path) -> Vec<(String, Via)> {
    ini::read(path)
        .into_iter()
        .filter_map(|s| {
            let via = Via {
                host: s.get("host")?.to_string(),
                target: s.get("target")?.to_string(),
                port: s.get("port")?.to_string(),
                local: s.get("local")?.to_string(),
            };
            // A half-written section is not a tunnel we can rebuild, and
            // guessing the missing half would open a forward to the wrong
            // place, so it is dropped rather than repaired.
            (!via.host.is_empty() && !via.target.is_empty()).then_some((s.name, via))
        })
        .collect()
}

/// The tunnel this connection needs, if it was told about one.
pub fn get(key: &str) -> Option<Via> {
    get_in(&store_path(), key)
}

pub fn get_in(path: &std::path::Path, key: &str) -> Option<Via> {
    load_from(path)
        .into_iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v)
}

/// Remember the forward that was just opened for this connection.
pub fn set(key: &str, via: &Via) -> Result<()> {
    set_in(&store_path(), key, via)
}

pub fn set_in(path: &std::path::Path, key: &str, via: &Via) -> Result<()> {
    ini::upsert(
        path,
        Some(key),
        key,
        &[
            ("host".to_string(), via.host.clone()),
            ("target".to_string(), via.target.clone()),
            ("port".to_string(), via.port.clone()),
            ("local".to_string(), via.local.clone()),
        ],
    )
}

/// Carry a via across a rename. A connection is identified by `Conn::key()`, so
/// renaming it changes its identity: without this the via is stranded under the
/// old key and the connection silently forgets the tunnel it depends on, which
/// looks like the feature simply not working.
pub fn rename(old_key: &str, new_key: &str) -> Result<()> {
    if old_key == new_key {
        return Ok(());
    }
    let Some(via) = get(old_key) else {
        return Ok(());
    };
    set(new_key, &via)?;
    remove(old_key)
}

/// Forget it, which is what deleting the connection has to do too, or the file
/// slowly fills with tunnels for things that no longer exist.
pub fn remove(key: &str) -> Result<()> {
    remove_in(&store_path(), key)
}

pub fn remove_in(path: &std::path::Path, key: &str) -> Result<()> {
    if get_in(path, key).is_none() {
        return Ok(());
    }
    ini::remove(path, key)
}

/// Does this connection actually point at the near end of its own forward?
///
/// Declining the repoint leaves the two disagreeing: the via is recorded, so
/// `ensure` dutifully opens a forward, and the connection still asks for the far
/// address that was refused in the first place. Nothing here guesses which half
/// is wrong - that is the user's call - but the mismatch is worth saying out
/// loud rather than leaving as a tunnel that opens and is never used.
pub fn points_at_it(c: &crate::engines::Conn, via: &Via) -> bool {
    let loopback = matches!(c.host.as_str(), "127.0.0.1" | "localhost" | "::1");
    loopback && c.port_or_default() == via.local
}

/// Make a connection's remembered tunnel real, if it has one and nothing is
/// already carrying it. Returns what happened, for a status line or a printed
/// notice; `None` when there was nothing to do, which is the common case.
///
/// Called on the way into a session from both the CLI and the TUI, because a
/// connection pointed at a forward is simply broken without it, and the two
/// ways in must not disagree about that.
pub fn ensure(key: &str) -> Option<Result<String>> {
    let via = get(key)?;
    if crate::tunnels::carrying(&via.local).is_some() {
        return None;
    }
    Some(match crate::tunnels::open('L', &via.spec(), &via.host) {
        Ok(t) => Ok(format!(
            "reopened the tunnel through {} (pid {})",
            via.host, t.pid
        )),
        Err(e) => Err(e),
    })
}

/// Which connection asked for the forward on this local port, if any. The
/// Tunnels tab is otherwise a list of ports with no idea what they are for,
/// which makes `d` on it a guess.
pub fn owner_of(local: &str) -> Option<String> {
    load_from(&store_path())
        .into_iter()
        .find(|(_, v)| v.local == local)
        .map(|(key, _)| key)
}

/// Every recorded via, for the checks that have to look at all of them.
pub fn all() -> Vec<(String, Via)> {
    load_from(&store_path())
}

/// The ssh host to start the tunnel wizard on, when the `tunnel_host` setting
/// has not been given one. In order: the host this very connection was last
/// tunnelled through, then the only host every other via uses, and otherwise
/// nothing - because guessing between two bastions is worse than an empty field
/// you were going to fill in anyway.
pub fn default_host(key: &str) -> Option<String> {
    let all = all();
    if let Some((_, v)) = all.iter().find(|(k, _)| k == key) {
        return Some(v.host.clone());
    }
    let mut hosts: Vec<&str> = all.iter().map(|(_, v)| v.host.as_str()).collect();
    hosts.sort_unstable();
    hosts.dedup();
    match hosts.as_slice() {
        [only] => Some((*only).to_string()),
        _ => None,
    }
}
