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

/// The rows of the Tunnels tab: what is running, then what a connection
/// remembers and nobody has opened. A sleeping via used to be listed nowhere -
/// the tab showed processes - so the only sign of it was a yellow dot on the
/// connection, which is a state you can see and not act on.
pub fn rows() -> Vec<crate::tunnels::Entry> {
    rows_from(&crate::tunnels::list(), all())
}

fn rows_from(
    live: &[crate::tunnels::Tunnel],
    recorded: Vec<(String, Via)>,
) -> Vec<crate::tunnels::Entry> {
    let mut rows: Vec<crate::tunnels::Entry> = live
        .iter()
        .map(|t| crate::tunnels::Entry {
            owner: t.ports().and_then(|(open, _, _)| owner_in(&recorded, open)),
            kind: t.kind,
            spec: t.spec.clone(),
            host: t.host.clone(),
            live: Some(t.clone()),
        })
        .collect();

    for (key, via) in recorded {
        // Carrying the local port is what counts, exactly as `ensure` decides
        // it: a forward already holding that port is the one this connection
        // will use, whatever spec it was opened with, and it is on screen
        // already as a row of its own.
        if live
            .iter()
            .any(|t| t.kind == 'L' && t.ports().is_some_and(|(open, _, _)| open == via.local))
        {
            continue;
        }
        rows.push(crate::tunnels::Entry {
            owner: Some(key),
            kind: 'L',
            spec: via.spec(),
            host: via.host,
            live: None,
        });
    }
    rows
}

fn owner_in(recorded: &[(String, Via)], local: &str) -> Option<String> {
    recorded
        .iter()
        .find(|(_, v)| v.local == local)
        .map(|(key, _)| key.clone())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnels::Tunnel;

    fn via(local: &str, port: &str) -> Via {
        Via {
            host: "bastion".into(),
            target: "127.0.0.1".into(),
            port: port.into(),
            local: local.into(),
        }
    }

    fn running(spec: &str) -> Tunnel {
        Tunnel {
            pid: 4242,
            kind: 'L',
            spec: spec.into(),
            host: "bastion".into(),
            log: std::path::PathBuf::new(),
        }
    }

    #[test]
    fn a_remembered_forward_is_listed_whether_or_not_it_is_running() {
        let recorded = vec![
            ("pg:prod".to_string(), via("5432", "5432")),
            ("pg:metrics".to_string(), via("5433", "5432")),
        ];
        let rows = rows_from(&[running("5432:127.0.0.1:5432")], recorded);

        // One row each, never two for the same forward: the one that is up is
        // the live row, and the one that is not is still on the list, which is
        // the whole point - an `off` you can see is an `off` you can turn on.
        assert_eq!(rows.len(), 2);
        assert!(rows[0].on());
        assert_eq!(rows[0].owner.as_deref(), Some("pg:prod"));
        assert!(!rows[1].on());
        assert_eq!(rows[1].owner.as_deref(), Some("pg:metrics"));
        assert_eq!(rows[1].spec, "5433:127.0.0.1:5432");
    }

    #[test]
    fn holding_the_port_is_what_counts_as_carrying_it() {
        // `ensure` decides a via is satisfied by whatever holds its local port,
        // whatever spec that forward was opened with. The list has to agree, or
        // it grows a second row for a tunnel that is already there.
        let recorded = vec![("pg:prod".to_string(), via("5432", "5432"))];
        let rows = rows_from(&[running("5432:10.0.0.9:5432")], recorded);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].on());

        // A forward nothing recorded is still a row, just one with no owner to
        // name: it is running, and `d` on it has to mean something.
        let rows = rows_from(&[running("9000:127.0.0.1:80")], Vec::new());
        assert_eq!(rows.len(), 1);
        assert!(rows[0].owner.is_none());
    }
}
