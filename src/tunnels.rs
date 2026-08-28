//! Port-forward tracking: how you reach a database that is not routable from
//! here. A production Postgres almost never listens to the open internet, so
//! the real connection is `ssh -L 5432:db.internal:5432 bastion` and then
//! `psql` at localhost - and nobody remembers that argument order. Worse, once
//! you background a tunnel with `-f` you lose the PID and cannot find or kill
//! it later. So we spawn our own detached `ssh -N`, record its PID and what it
//! does in a tiny state file, and let you list and kill them.
//!
//! This is the one part of easysql that is not about a database client at all.
//! It is here because it is the step between you and the database, and having
//! it in the same window as the connection it serves is the whole point.
//!
//! We capture each tunnel's stderr to its own log and, after spawning, wait a
//! beat to confirm it actually stayed up - so a bad port or auth failure surfaces
//! as an error instead of a "tunnel" that was already dead on arrival.

use anyhow::{Context, Result, bail};
use std::fs::{self, File};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A live (or once-live) forward. `spec` is the raw ssh forward argument, e.g.
/// `8080:localhost:80`; `kind` is 'L' (local) or 'R' (reverse/remote).
#[derive(Clone)]
pub struct Tunnel {
    pub pid: u32,
    pub kind: char,
    pub spec: String,
    pub host: String,
    /// File that captured this tunnel's stderr (for diagnosing failures).
    pub log: PathBuf,
}

impl Tunnel {
    /// One-line human summary for the listing / TUI.
    pub fn describe(&self) -> String {
        // The host used to be printed twice ("dbhost (local → dbhost)"), which
        // said nothing the spec did not and pushed the row off the pane. The
        // arrow carries the direction on its own.
        let arrow = if self.kind == 'L' { "→" } else { "←" };
        format!(
            "pid {:>7}  -{} {}  {} {}",
            self.pid, self.kind, self.spec, arrow, self.host
        )
    }

    /// The three parts of a `-L`/`-R` spec: the port opened on this side, and
    /// the `host:port` the far side connects onward to. `None` for a spec we
    /// did not write (a bind address in front makes it four fields).
    pub fn ports(&self) -> Option<(&str, &str, &str)> {
        let mut f = self.spec.split(':');
        match (f.next(), f.next(), f.next(), f.next()) {
            (Some(open), Some(target), Some(port), None) => Some((open, target, port)),
            _ => None,
        }
    }

    /// What this forward does, in the words of what you would use it for.
    pub fn explain(&self) -> String {
        let Some((open, target, port)) = self.ports() else {
            return format!("-{} {}", self.kind, self.spec);
        };
        // The middle field is resolved on the *far* side, so a bare `localhost`
        // there means the server itself, not this machine.
        let local_target = matches!(target, "localhost" | "127.0.0.1");
        match (self.kind, local_target) {
            ('L', true) => format!("localhost:{open} here is {}'s own {port}", self.host),
            ('L', false) => format!(
                "localhost:{open} here is {target}:{port}, reached from {}",
                self.host
            ),
            (_, true) => format!("{}:{open} there is this machine's {port}", self.host),
            (_, false) => format!(
                "{}:{open} there is {target}:{port}, reached from here",
                self.host
            ),
        }
    }

    /// The command that opened it, exactly as it was run.
    pub fn command(&self) -> String {
        format!("ssh -N -{} {} {}", self.kind, self.spec, self.host)
    }

    /// When it was opened: the log file is created as the tunnel is spawned, so
    /// its mtime is the tunnel's birth time. `None` if the file has gone.
    pub fn started_at(&self) -> Option<SystemTime> {
        fs::metadata(&self.log).ok()?.modified().ok()
    }

    /// Whatever ssh wrote to stderr and kept running through - a warning about
    /// a port already in use, say. Empty for a forward that is simply working.
    pub fn stderr(&self) -> String {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// A tunnel is alive while its PID still has a `/proc` entry (Linux).
    pub fn alive(&self) -> bool {
        proc_alive(self.pid)
    }
}

fn proc_alive(pid: u32) -> bool {
    PathBuf::from(format!("/proc/{pid}")).exists()
}

/// `~/.local/state/easysql/` - persists tunnel state + logs across `esql` runs.
fn state_dir() -> PathBuf {
    dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("easysql")
}

fn state_path() -> PathBuf {
    state_dir().join("tunnels.tsv")
}

fn log_path() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    state_dir().join("tunnels").join(format!("{stamp}.log"))
}

/// Read all tracked tunnels, dropping any whose process has died, and rewrite
/// the state file so it self-prunes. TSV: `pid\tkind\tspec\thost\tlog`.
pub fn list() -> Vec<Tunnel> {
    let Ok(text) = fs::read_to_string(state_path()) else {
        return Vec::new();
    };

    let mut live = Vec::new();
    for line in text.lines() {
        let mut f = line.split('\t');
        let (Some(pid), Some(kind), Some(spec), Some(host)) =
            (f.next(), f.next(), f.next(), f.next())
        else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        // `log` is optional so a state file from an older build still parses.
        let log = f.next().map(PathBuf::from).unwrap_or_default();
        let t = Tunnel {
            pid,
            kind: kind.chars().next().unwrap_or('L'),
            spec: spec.to_string(),
            host: host.to_string(),
            log,
        };
        if t.alive() {
            live.push(t);
        } else if !t.log.as_os_str().is_empty() {
            let _ = fs::remove_file(&t.log); // clean the log of a tunnel that has died
        }
    }

    // Persist the pruned set (best effort - a stale line just reappears next run).
    let _ = save(&live);
    live
}

fn save(tunnels: &[Tunnel]) -> Result<()> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body: String = tunnels
        .iter()
        .map(|t| {
            format!(
                "{}\t{}\t{}\t{}\t{}\n",
                t.pid,
                t.kind,
                t.spec,
                t.host,
                t.log.display()
            )
        })
        .collect();
    fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Is a live `-L` already opening this local port? A remembered tunnel must
/// never be dug twice: the second ssh would fail on the bound port and leave a
/// confusing error in front of a connection that was about to work.
pub fn carrying(local: &str) -> Option<Tunnel> {
    list()
        .into_iter()
        .find(|t| t.kind == 'L' && t.ports().is_some_and(|(open, _, _)| open == local))
}

/// Open a forward: spawn `ssh -N -{L|R} <spec> <host>` fully detached (its own
/// process group, stderr to a log file) so it outlives us, wait ~1s to confirm
/// it connected, then record it. If ssh bailed (bad port, auth, DNS) the captured
/// stderr becomes the returned error instead of a phantom tunnel.
/// Assumes key/agent auth - a background tunnel can't answer a password prompt.
pub fn open(kind: char, spec: &str, host: &str) -> Result<Tunnel> {
    let log = log_path();
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent)?;
    }
    let errfile = File::create(&log).with_context(|| format!("creating {}", log.display()))?;

    let mut cmd = Command::new("ssh");
    cmd.arg("-N")
        .arg(format!("-{kind}"))
        .arg(spec)
        .arg(host)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(errfile));

    // Detach from our controlling terminal's job control so Ctrl-C / our exit
    // doesn't take the tunnel down with us.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let child = cmd.spawn().with_context(|| "spawning ssh tunnel")?;
    let pid = child.id();

    // Give ssh a moment to connect, then judge it by what it wrote rather than
    // only by whether it is still running.
    sleep(Duration::from_millis(800));
    let stderr = fs::read_to_string(&log).unwrap_or_default();
    let first = || {
        stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("ssh exited immediately")
            .to_string()
    };
    if !proc_alive(pid) {
        let reason = first();
        let _ = fs::remove_file(&log);
        bail!("{reason}");
    }
    // A forward whose port is already taken does NOT kill ssh: it keeps the
    // session open with no forward on it, which used to be listed as a live
    // tunnel that quietly carried nothing. Take ssh at its word and clean up.
    if forwarding_failed(&stderr) {
        let reason = first();
        let _ = Command::new("kill").arg(pid.to_string()).status();
        let _ = fs::remove_file(&log);
        bail!("{reason}");
    }

    let tunnel = Tunnel {
        pid,
        kind,
        spec: spec.to_string(),
        host: host.to_string(),
        log,
    };

    let mut all = list();
    all.push(tunnel.clone());
    save(&all)?;
    Ok(tunnel)
}

/// Did ssh tell us the forward itself could not be set up? These are the exact
/// phrases OpenSSH uses; the session survives all of them, so nothing else
/// would notice.
fn forwarding_failed(stderr: &str) -> bool {
    const FAILURES: [&str; 4] = [
        "Address already in use",
        "cannot listen to port",
        "Could not request local forwarding",
        "remote port forwarding failed",
    ];
    FAILURES.iter().any(|f| stderr.contains(f))
}

/// Terminate a tunnel by PID (SIGTERM via `kill`), delete its log, and forget it.
pub fn kill(pid: u32) -> Result<()> {
    let all = list();
    if let Some(t) = all.iter().find(|t| t.pid == pid) {
        let _ = fs::remove_file(&t.log);
    }
    let _ = Command::new("kill").arg(pid.to_string()).status();
    let remaining: Vec<Tunnel> = all.into_iter().filter(|t| t.pid != pid).collect();
    save(&remaining)?;
    Ok(())
}
