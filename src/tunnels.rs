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
//! We capture each tunnel's stderr to its own log and, after spawning, wait
//! until the forward is really listening, so a bad port or an auth failure
//! surfaces as an error instead of a "tunnel" that was already dead on arrival.

use anyhow::{Context, Result};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// What `open` runs, which is also what every line that shows the command
/// prints, so the two cannot disagree. `BatchMode` turns a host key or a
/// passphrase that would need asking into a failure that can be read, where a
/// prompt from a background ssh is drawn over the screen and never answered;
/// `ExitOnForwardFailure` ends ssh when its port cannot be bound, where it
/// would otherwise stay connected and carry nothing.
pub fn argv(kind: char, spec: &str, host: &str) -> Vec<String> {
    [
        "ssh",
        "-N",
        "-o",
        "BatchMode=yes",
        "-o",
        "ExitOnForwardFailure=yes",
    ]
    .iter()
    .map(|w| w.to_string())
    .chain([format!("-{kind}"), spec.to_string(), host.to_string()])
    .collect()
}

/// `argv` as one line, for a preview or a status line.
pub fn command(kind: char, spec: &str, host: &str) -> String {
    argv(kind, spec, host).join(" ")
}

/// Why a forward could not come up, for the failures something can be done
/// about. Anything else is ssh's own words.
#[derive(Debug)]
pub enum Blocked {
    /// There is no `ssh` on PATH.
    NoSsh,
    /// Something already listens on the local port: another program, or `by`,
    /// a forward that carries another connection.
    PortTaken { port: u16, by: Option<String> },
    /// ssh has never seen `host`'s key, and a forward in the background cannot
    /// ask whether to trust it. `said` is ssh's own words.
    UnknownHostKey { host: String, said: String },
}

impl std::fmt::Display for Blocked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Blocked::NoSsh => write!(
                f,
                "`ssh` is not on PATH, so no tunnel can be opened: install the OpenSSH client"
            ),
            Blocked::PortTaken { port, by: None } => write!(
                f,
                "localhost:{port} is already taken by another program: pick another local port"
            ),
            Blocked::PortTaken { port, by: Some(by) } => write!(
                f,
                "localhost:{port} is already forwarded to {by}, which is not this connection's: pick another local port"
            ),
            Blocked::UnknownHostKey { said, .. } => write!(f, "{said}"),
        }
    }
}

impl std::error::Error for Blocked {}

/// Whether nothing listens on `port` here, so a forward can take it.
pub fn port_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// The first port after `from` that nothing listens on and no recorded
/// forward claims, for a tunnel that cannot have the one it asked for.
pub fn free_port_after(from: u16) -> Option<u16> {
    let claimed: Vec<String> = recorded()
        .iter()
        .filter_map(|t| t.ports().map(|(open, _, _)| open.to_string()))
        .collect();
    (from.saturating_add(1)..=from.saturating_add(200))
        .find(|p| !claimed.contains(&p.to_string()) && port_free(*p))
}

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

/// One row of the Tunnels tab: a forward that is running, or one a connection
/// remembers and nobody has opened yet. The second kind used to be invisible
/// here, which made the tab a list of processes rather than of the forwards this
/// tool knows about - and left an `off` state you could see on the Connections
/// tab but not act on.
pub struct Entry {
    /// The connection that asked for it (`Conn::key()`), when one did.
    pub owner: Option<String>,
    pub kind: char,
    pub spec: String,
    pub host: String,
    /// The process carrying it, when it is up.
    pub live: Option<Tunnel>,
}

/// The three parts of a `-L`/`-R` spec: the port opened on this side, and the
/// `host:port` the far side connects onward to. `None` for a spec we did not
/// write (a bind address in front makes it four fields). Shared, because the
/// row and the process it may be carrying are asked the same question.
fn ports_of(spec: &str) -> Option<(&str, &str, &str)> {
    let mut f = spec.split(':');
    match (f.next(), f.next(), f.next(), f.next()) {
        (Some(open), Some(target), Some(port), None) => Some((open, target, port)),
        _ => None,
    }
}

impl Entry {
    pub fn on(&self) -> bool {
        self.live.is_some()
    }

    pub fn pid(&self) -> Option<u32> {
        self.live.as_ref().map(|t| t.pid)
    }

    /// One-line human summary for the listing / TUI.
    pub fn describe(&self) -> String {
        // The host used to be printed twice ("dbhost (local → dbhost)"), which
        // said nothing the spec did not and pushed the row off the pane. The
        // arrow carries the direction on its own. What used to lead the row was
        // the pid, which is in the detail panel and is not why you are looking.
        let arrow = if self.kind == 'L' { "→" } else { "←" };
        format!(
            "{:<3}  -{} {}  {} {}",
            if self.on() { "on" } else { "off" },
            self.kind,
            self.spec,
            arrow,
            self.host
        )
    }

    pub fn ports(&self) -> Option<(&str, &str, &str)> {
        ports_of(&self.spec)
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

    /// The command that opens it, exactly as it is run.
    pub fn command(&self) -> String {
        command(self.kind, &self.spec, &self.host)
    }
}

impl Tunnel {
    pub fn ports(&self) -> Option<(&str, &str, &str)> {
        ports_of(&self.spec)
    }

    /// The command that runs it, exactly as it was started.
    pub fn command(&self) -> String {
        command(self.kind, &self.spec, &self.host)
    }

    /// When it was opened: the log is named for the moment the tunnel was
    /// spawned, which ssh writing to it later does not move. A log from an
    /// older build without that name falls back to its mtime.
    pub fn started_at(&self) -> Option<SystemTime> {
        let named = self
            .log
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u64>().ok());
        match named {
            Some(nanos) => Some(UNIX_EPOCH + Duration::from_nanos(nanos)),
            None => fs::metadata(&self.log).ok()?.modified().ok(),
        }
    }

    /// Whether this is the forward `via` asks for: the same spec through the
    /// same host. Holding the same local port is not enough, since two
    /// connections may remember one port for two different servers.
    pub fn carries(&self, via: &crate::vias::Via) -> bool {
        self.kind == 'L' && self.spec == via.spec() && self.host == via.host
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

    /// A tunnel is alive while its PID is still running *this* forward. The
    /// argv is checked rather than only `/proc/<pid>`: the state file outlives a
    /// reboot, and a pid the kernel has since handed to something else would
    /// otherwise read as up and be what Enter or `d` kills.
    pub fn alive(&self) -> bool {
        proc_is_ours(self.pid, self.kind, &self.spec, &self.host)
    }
}

/// Is this pid running the forward we think it is? `/proc/<pid>` existing only
/// says *something* is running under that number.
fn proc_is_ours(pid: u32, kind: char, spec: &str, host: &str) -> bool {
    let Ok(raw) = fs::read(format!("/proc/{pid}/cmdline")) else {
        return false;
    };
    let args: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    let flag = format!("-{kind}");
    args.contains(&flag) && args.iter().any(|a| a == spec) && args.iter().any(|a| a == host)
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

/// Every tracked tunnel whose process still runs it. One that has died has its
/// log deleted and its line dropped from the state file, which is written only
/// then. TSV: `pid\tkind\tspec\thost\tlog`.
pub fn list() -> Vec<Tunnel> {
    let (live, dead): (Vec<Tunnel>, Vec<Tunnel>) = recorded().into_iter().partition(Tunnel::alive);
    if !dead.is_empty() {
        for t in &dead {
            if !t.log.as_os_str().is_empty() {
                let _ = fs::remove_file(&t.log);
            }
        }
        // Best effort: a stale line just reappears and is dropped next time.
        let _ = forget(&dead);
    }
    live
}

/// Every line of the state file, alive or not.
fn recorded() -> Vec<Tunnel> {
    let Ok(text) = fs::read_to_string(state_path()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
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
        out.push(Tunnel {
            pid,
            kind: kind.chars().next().unwrap_or('L'),
            spec: spec.to_string(),
            host: host.to_string(),
            log,
        });
    }
    out
}

/// Drop `gone` from the state file. The file is read again just before the
/// write, so a tunnel another esql recorded in the meantime is kept.
fn forget(gone: &[Tunnel]) -> Result<()> {
    let same = |a: &Tunnel, b: &Tunnel| a.pid == b.pid && a.spec == b.spec && a.host == b.host;
    let keep: Vec<Tunnel> = recorded()
        .into_iter()
        .filter(|t| !gone.iter().any(|g| same(g, t)))
        .collect();
    save(&keep)
}

fn save(tunnels: &[Tunnel]) -> Result<()> {
    let path = state_path();
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
    crate::ini::write_atomic(&path, &body)
}

/// Is a live `-L` already opening this local port? A remembered tunnel must
/// never be dug twice: the second ssh would fail on the bound port and leave a
/// confusing error in front of a connection that was about to work.
pub fn carrying(local: &str) -> Option<Tunnel> {
    list()
        .into_iter()
        .find(|t| t.kind == 'L' && t.ports().is_some_and(|(open, _, _)| open == local))
}

/// Is this very forward running: the same spec through the same host? Two
/// connections may remember the same local port for different servers, and
/// `carrying` alone would let one use, report and stop the other's.
pub fn carrying_via(via: &crate::vias::Via) -> Option<Tunnel> {
    list().into_iter().find(|t| t.carries(via))
}

/// How long a `-L` gets to come up: ssh connecting, authenticating and
/// binding the port. Long enough for a bastion on the far side of a VPN.
const UP_WITHIN: Duration = Duration::from_secs(15);

/// Open a forward: spawn `argv` fully detached (its own process group, stderr
/// to a log file) so it outlives us, wait until it is up, then record it. Up
/// means a `-L` is listening on its port, and a `-R`, which has nothing here to
/// ask, is still running after a moment. If ssh bailed (bad port, auth, DNS)
/// its own words become the returned error instead of a phantom tunnel.
/// Assumes key or agent auth: `BatchMode` turns any prompt into a failure.
pub fn open(kind: char, spec: &str, host: &str) -> Result<Tunnel> {
    if !crate::engines::on_path("ssh") {
        return Err(Blocked::NoSsh.into());
    }
    let local = match kind {
        'L' => ports_of(spec).and_then(|(open, _, _)| open.parse::<u16>().ok()),
        _ => None,
    };
    // A port another program already holds would make the forward fail, or
    // answer for it if it were only asked whether something listens there.
    if let Some(port) = local
        && !port_free(port)
    {
        return Err(Blocked::PortTaken { port, by: None }.into());
    }

    let log = log_path();
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent)?;
    }
    let errfile = File::create(&log).with_context(|| format!("creating {}", log.display()))?;

    let argv = argv(kind, spec, host);
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
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

    let mut child = cmd.spawn().context("starting ssh")?;
    let pid = child.id();
    let started = Instant::now();
    let within = if local.is_some() {
        UP_WITHIN
    } else {
        Duration::from_millis(1500)
    };
    loop {
        sleep(Duration::from_millis(100));
        if let Ok(Some(_)) = child.try_wait() {
            return Err(gave_up(
                &mut child,
                &log,
                host,
                "ssh exited before the tunnel was up",
            ));
        }
        if forwarding_failed(&fs::read_to_string(&log).unwrap_or_default()) {
            return Err(gave_up(
                &mut child,
                &log,
                host,
                "ssh could not set up the forward",
            ));
        }
        if local.is_some_and(|port| listening(pid, port)) {
            break;
        }
        if started.elapsed() >= within {
            if local.is_none() {
                break;
            }
            return Err(gave_up(
                &mut child,
                &log,
                host,
                &format!(
                    "ssh did not bring the tunnel up within {}s",
                    UP_WITHIN.as_secs()
                ),
            ));
        }
    }
    // Reaped in the background, or a forward that ends during a long session
    // stays a zombie of ours until the app exits.
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    let tunnel = Tunnel {
        pid,
        kind,
        spec: spec.to_string(),
        host: host.to_string(),
        log,
    };
    let mut all = recorded();
    all.push(tunnel.clone());
    save(&all)?;
    Ok(tunnel)
}

/// Stop ssh if it still runs, and turn what it wrote into the error: its own
/// lines that say what failed, at most five, without the known-hosts notice it
/// prints on the way, or `fallback` when it said nothing. A host whose key ssh
/// has never seen is `Blocked::UnknownHostKey`; a key that changed is not,
/// since accepting that one is a decision, not a fix.
fn gave_up(child: &mut Child, log: &Path, host: &str, fallback: &str) -> anyhow::Error {
    let _ = child.kill();
    let _ = child.wait();
    let text = fs::read_to_string(log).unwrap_or_default();
    let _ = fs::remove_file(log);
    let said: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("Warning: Permanently added"))
        .collect();
    if said.is_empty() {
        return anyhow::anyhow!("{fallback}");
    }
    let words = said[said.len().saturating_sub(5)..].join("\n");
    // ssh's own words over a host it would have asked about, and over a key
    // that changed, which says "has changed" in both of its warnings.
    if text.contains("Host key verification failed.")
        && !text.to_ascii_lowercase().contains("has changed")
    {
        return Blocked::UnknownHostKey {
            host: host.to_string(),
            said: words,
        }
        .into();
    }
    anyhow::anyhow!("{words}")
}

/// Whether `pid` holds a listening socket on `port`: the forward's own
/// listener and never another program's, read from `/proc` so no connection
/// reaches the far end just to ask. Where there is no `/proc/net/tcp`, a
/// connect to the port answers instead.
fn listening(pid: u32, port: u16) -> bool {
    if !Path::new("/proc/net/tcp").exists() {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        return std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok();
    }
    let sockets: Vec<String> = fs::read_dir(format!("/proc/{pid}/fd"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| fs::read_link(e.path()).ok())
        .filter_map(|l| {
            let l = l.to_str()?;
            Some(l.strip_prefix("socket:[")?.strip_suffix(']')?.to_string())
        })
        .collect();
    ["/proc/net/tcp", "/proc/net/tcp6"].iter().any(|table| {
        fs::read_to_string(table)
            .unwrap_or_default()
            .lines()
            .skip(1)
            .any(|row| {
                // sl, local address, remote address, state (0A is LISTEN), ..., inode
                let cols: Vec<&str> = row.split_whitespace().collect();
                cols.len() > 9
                    && cols[3] == "0A"
                    && cols[1]
                        .rsplit(':')
                        .next()
                        .and_then(|p| u16::from_str_radix(p, 16).ok())
                        == Some(port)
                    && sockets.iter().any(|s| s == cols[9])
            })
    })
}

/// Did ssh tell us the forward itself could not be set up? These are the exact
/// phrases OpenSSH uses. `ExitOnForwardFailure` ends ssh over them too, but
/// reading them first is what stops a failing forward a moment sooner.
fn forwarding_failed(stderr: &str) -> bool {
    const FAILURES: [&str; 4] = [
        "Address already in use",
        "cannot listen to port",
        "Could not request local forwarding",
        "remote port forwarding failed",
    ];
    FAILURES.iter().any(|f| stderr.contains(f))
}

/// How long `kill` waits for ssh to exit after the signal.
const GONE_WITHIN: Duration = Duration::from_secs(3);

/// Whether `pid` has exited with its files closed: gone, or a zombie waiting
/// to be reaped. Its command line empties before that, while it still holds
/// its port, so `alive` cannot tell.
fn exited(pid: u32) -> bool {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return true;
    };
    // `pid (comm) state ...`, where the name may hold a `)` itself.
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.trim_start().chars().next())
        .is_some_and(|state| matches!(state, 'Z' | 'X'))
}

/// Stop the tunnel `pid` runs (SIGTERM via `kill`), delete its log and forget
/// it. A pid that no longer runs that forward is never signalled, since the
/// kernel may have handed the number to something else. It returns once ssh
/// has exited, or after `GONE_WITHIN`, so a forward opened onto the same port
/// straight after does not find it still taken. Returns whether there was a
/// forward to stop.
pub fn kill(pid: u32) -> Result<bool> {
    let Some(t) = recorded().into_iter().find(|t| t.pid == pid) else {
        return Ok(false);
    };
    let ours = t.alive();
    if ours {
        let _ = Command::new("kill")
            .arg(pid.to_string())
            .stderr(Stdio::null())
            .status();
        let deadline = Instant::now() + GONE_WITHIN;
        while !exited(pid) && Instant::now() < deadline {
            sleep(Duration::from_millis(20));
        }
    }
    if !t.log.as_os_str().is_empty() {
        let _ = fs::remove_file(&t.log);
    }
    forget(std::slice::from_ref(&t))?;
    Ok(ours)
}
