//! `esql tunnel`: the tunnel form and the Tunnels tab's on/off without the
//! terminal. A forward is opened with `tunnels::open`, remembered with
//! `vias::set` and the connection repointed with `engines::save`, the same
//! three steps the form and its offer take, so a connect afterwards reopens it.

use super::{DryRun, can_ask, confirm_delete, fail, find_conn, said, shown, step};
use crate::engines::{self, Conn};
use crate::tunnels::Tunnel;
use crate::vias::{self, Via};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Open the ssh forward a connection needs, remember it and point the connection at it
    Up(UpArgs),
    /// Stop a connection's forward, keeping the record so the next connect reopens it
    Down(DownArgs),
}

#[derive(clap::Args)]
pub struct UpArgs {
    /// The connection, as `esql ls` prints it, or `<engine>:<name>`
    #[arg(value_name = "NAME",
          add = clap_complete::ArgValueCompleter::new(crate::completion::conn_names))]
    name: String,
    /// The ssh host to go through (default: the `tunnel_host` setting, else the host it went through last)
    #[arg(long, value_name = "HOST")]
    via: Option<String>,
    /// The port to open here (default: the database's own port)
    #[arg(long, value_name = "PORT")]
    local: Option<String>,
    #[command(flatten)]
    dry: DryRun,
}

#[derive(clap::Args)]
pub struct DownArgs {
    /// The connection, as `esql ls` prints it, or `<engine>:<name>`
    #[arg(value_name = "NAME",
          add = clap_complete::ArgValueCompleter::new(crate::completion::conn_names))]
    name: String,
    /// Also forget the record, so a connect no longer reopens it, and point the connection back at the database
    #[arg(long)]
    forget: bool,
    /// With `--forget`, forget it without asking, which is required when there is no terminal to ask on
    #[arg(short = 'y', long = "yes")]
    yes: bool,
    #[command(flatten)]
    dry: DryRun,
}

pub fn run(cmd: Cmd) {
    match cmd {
        Cmd::Up(args) => up(args),
        Cmd::Down(args) => down(args),
    }
}

fn up(args: UpArgs) {
    let c = find_conn(&args.name);
    if !c.engine.networked() {
        fail(&format!(
            "'{}' is a sqlite file on this machine, so there is nothing to tunnel to",
            c.name
        ));
    }
    let key = c.key();
    let recorded = vias::get(&key);
    // Nothing new asked for: bring back the forward that is remembered.
    if let Some(v) = &recorded
        && args.via.as_ref().is_none_or(|h| *h == v.host)
        && args.local.as_ref().is_none_or(|l| *l == v.local)
    {
        if crate::tunnels::carrying_via(v).is_some() {
            step(&format!(
                "nothing to open: localhost:{} is already up",
                v.local
            ));
            repoint(&c, &v.local, args.dry.dry_run);
            return;
        }
        if let Some(t) = crate::tunnels::carrying(&v.local) {
            fail(&format!(
                "localhost:{} is already forwarded to {} through {}: pick another with `--local <port>`",
                v.local, t.spec, t.host
            ));
        }
        open(&c, v, None, false, args.dry.dry_run);
        return;
    }
    let settings = crate::settings::load();
    let host = args
        .via
        .or_else(|| (!settings.tunnel_host.is_empty()).then(|| settings.tunnel_host.clone()))
        .or_else(|| vias::default_host(&key))
        .unwrap_or_else(|| {
            fail(&format!(
                "which ssh host does it go through? `esql tunnel up {} --via <host>`",
                args.name
            ))
        });
    // A connection already pointed at its own forward asks for 127.0.0.1, so
    // its database is the far address the record kept, seen again from the
    // hop asked for now: `127.0.0.1` meant the old hop, never the new one.
    let (target, port) = match &recorded {
        Some(v) if vias::points_at_it(&c, v) => (
            vias::forward_target(&host, &vias::far_address(v)),
            v.port.clone(),
        ),
        _ => (vias::forward_target(&host, &c.host), c.port_or_default()),
    };
    let local = args.local.unwrap_or_else(|| port.clone());
    // The forward this connection had is replaced, never left running with
    // nothing remembering it and no command that could stop it, unless another
    // connection rides on it too.
    let replaced = recorded
        .as_ref()
        .and_then(crate::tunnels::carrying_via)
        .filter(|t| vias::sharing(&key, t).is_empty());
    if let Some(t) = crate::tunnels::carrying(&local)
        && replaced.as_ref().is_none_or(|r| r.pid != t.pid)
    {
        fail(&format!(
            "localhost:{local} is already forwarded to {} through {}: pick another with `--local <port>`",
            t.spec, t.host
        ));
    }
    let via = Via {
        host,
        target,
        port,
        local,
    };
    open(&c, &via, replaced, true, args.dry.dry_run);
}

/// Stop the forward `via` replaces, open `via`, remember it when it is new,
/// and point the connection at it. Each step is printed as it is taken, so a
/// failure leaves no line for a step that never happened, and a dry run
/// prints them all.
fn open(c: &Conn, via: &Via, replaced: Option<Tunnel>, remember: bool, dry: bool) {
    if let Some(t) = replaced {
        step(&format!("stop {} (pid {})", t.command(), t.pid));
        if !dry && let Err(e) = crate::tunnels::kill(t.pid) {
            fail(&format!("could not stop it: {}", said(&e)));
        }
    }
    step(&format!("open {}", via.command()));
    if !dry && let Err(e) = crate::tunnels::open('L', &via.spec(), &via.host) {
        fail(&format!("ssh did not open the tunnel: {}", said(&e)));
    }
    if remember {
        step(&format!(
            "remember it for {} in {}",
            c.key(),
            shown(&vias::store_path())
        ));
        if !dry && let Err(e) = vias::set(&c.key(), via) {
            fail(&format!(
                "the tunnel is open, but it was not remembered: {}",
                said(&e)
            ));
        }
    }
    if !vias::points_at_it(c, via) {
        step(&point_line(c, &via.local));
        if !dry && let Err(e) = engines::repoint(c, "127.0.0.1", &via.local) {
            fail(&format!(
                "the tunnel is open, but the connection was not pointed at it: {}",
                said(&e)
            ));
        }
    }
}

/// Point the connection at a forward that is already up, when it does not.
fn repoint(c: &Conn, local: &str, dry: bool) {
    let loopback = matches!(c.host.as_str(), "127.0.0.1" | "localhost" | "::1");
    if loopback && c.port_or_default() == local {
        return;
    }
    step(&point_line(c, local));
    if !dry && let Err(e) = engines::repoint(c, "127.0.0.1", local) {
        fail(&format!(
            "the tunnel is open, but the connection was not pointed at it: {}",
            said(&e)
        ));
    }
}

fn point_line(c: &Conn, local: &str) -> String {
    format!(
        "point {} at 127.0.0.1:{local} in {}",
        c.key(),
        shown(&c.engine.store())
    )
}

fn down(args: DownArgs) {
    let c = find_conn(&args.name);
    let key = c.key();
    let Some(via) = vias::get(&key) else {
        fail(&format!(
            "{key} has no tunnel: `esql tunnel up {} --via <host>` opens one",
            args.name
        ));
    };
    let live = crate::tunnels::carrying_via(&via);
    let others = live
        .as_ref()
        .map(|t| vias::sharing(&key, t))
        .unwrap_or_default();
    let what = format!("the tunnel record for {key}");
    if args.forget {
        can_ask(&what, args.yes, args.dry.dry_run);
    }
    if live.is_none() && !args.forget {
        step(&format!(
            "nothing to stop: the tunnel for {key} is not running"
        ));
        return;
    }
    // Forgetting this connection's record leaves running a forward another
    // connection rides on; stopping it is what a plain `down` asks for.
    let live = live.filter(|_| !args.forget || others.is_empty());
    if let Some(t) = &live {
        let also = match others.is_empty() {
            true => String::new(),
            false => format!(", which {} uses too", others.join(", ")),
        };
        step(&format!("stop {} (pid {}){also}", t.command(), t.pid));
    }
    if args.forget {
        step(&format!("forget it in {}", shown(&vias::store_path())));
        // The record is the only place the real address is kept once the
        // connection points at its forward, so it goes back there first.
        if vias::points_at_it(&c, &via) {
            step(&format!(
                "point {key} back at {}:{} in {}",
                vias::far_address(&via),
                via.port,
                shown(&c.engine.store())
            ));
        }
    }
    if args.dry.dry_run {
        return;
    }
    if args.forget {
        confirm_delete(&what, None, args.yes);
    }
    if let Some(t) = live
        && let Err(e) = crate::tunnels::kill(t.pid)
    {
        fail(&format!("could not stop it: {}", said(&e)));
    }
    if args.forget
        && let Err(e) = vias::forget(&c)
    {
        fail(&format!(
            "stopped, but the record was not forgotten: {}",
            said(&e)
        ));
    }
}
