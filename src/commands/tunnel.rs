//! `esql tunnel`: the tunnel form and the Tunnels tab's on/off without the
//! terminal. A forward is opened with `tunnels::open`, remembered with
//! `vias::set` and the connection repointed with `engines::save`, the same
//! three steps the form and its offer take, so a connect afterwards reopens it.

use super::{DryRun, Yes, can_ask, confirm_delete, fail, find_conn, said, shown, step};
use crate::engines::{self, Conn, NewConn};
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
    /// Also forget the record, so a connect no longer reopens it
    #[arg(long)]
    forget: bool,
    #[command(flatten)]
    yes: Yes,
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
        open(&c, v, false, args.dry.dry_run);
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
    // the far address is the one the record kept, not the one in the block.
    let (target, port) = match &recorded {
        Some(v) if vias::points_at_it(&c, v) => (v.target.clone(), v.port.clone()),
        _ => (vias::forward_target(&host, &c.host), c.port_or_default()),
    };
    let local = args.local.unwrap_or_else(|| port.clone());
    if let Some(t) = crate::tunnels::carrying(&local) {
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
    open(&c, &via, true, args.dry.dry_run);
}

/// Open `via`, remember it when it is new, and point the connection at it.
fn open(c: &Conn, via: &Via, remember: bool, dry: bool) {
    step(&format!("open {}", via.command()));
    if remember {
        step(&format!(
            "remember it for {} in {}",
            c.key(),
            shown(&vias::store_path())
        ));
    }
    let moves = !vias::points_at_it(c, via);
    if moves {
        step(&point_line(c, &via.local));
    }
    if dry {
        return;
    }
    if let Err(e) = crate::tunnels::open('L', &via.spec(), &via.host) {
        fail(&format!("ssh did not open the tunnel: {}", said(&e)));
    }
    if remember && let Err(e) = vias::set(&c.key(), via) {
        fail(&format!(
            "the tunnel is open, but it was not remembered: {}",
            said(&e)
        ));
    }
    if moves {
        save_pointed(c, &via.local);
    }
}

/// Point the connection at a forward that is already up, when it does not.
fn repoint(c: &Conn, local: &str, dry: bool) {
    let loopback = matches!(c.host.as_str(), "127.0.0.1" | "localhost" | "::1");
    if loopback && c.port_or_default() == local {
        return;
    }
    step(&point_line(c, local));
    if !dry {
        save_pointed(c, local);
    }
}

fn point_line(c: &Conn, local: &str) -> String {
    format!(
        "point {} at 127.0.0.1:{local} in {}",
        c.key(),
        shown(&c.engine.store())
    )
}

/// Rewrite the block's host and port and nothing else, as the repoint offer does.
fn save_pointed(c: &Conn, local: &str) {
    let nc = NewConn {
        engine: c.engine,
        name: c.name.clone(),
        host: "127.0.0.1".into(),
        port: local.to_string(),
        database: c.database.clone(),
        user: c.user.clone(),
        extra: c.extra.clone(),
    };
    if let Err(e) = engines::save(Some(&c.name), &nc) {
        fail(&format!(
            "the tunnel is open, but the connection was not pointed at it: {}",
            said(&e)
        ));
    }
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
    let what = format!("the tunnel record for {key}");
    if args.forget {
        can_ask(&what, args.yes.yes, args.dry.dry_run);
    }
    if live.is_none() && !args.forget {
        step(&format!(
            "nothing to stop: the tunnel for {key} is not running"
        ));
        return;
    }
    if let Some(t) = &live {
        step(&format!(
            "stop ssh -N -L {} {} (pid {})",
            t.spec, t.host, t.pid
        ));
    }
    if args.forget {
        step(&format!("forget it in {}", shown(&vias::store_path())));
    }
    if args.dry.dry_run {
        return;
    }
    if args.forget {
        confirm_delete(&what, None, args.yes.yes);
    }
    if let Some(t) = live
        && let Err(e) = crate::tunnels::kill(t.pid)
    {
        fail(&format!("could not stop it: {}", said(&e)));
    }
    if args.forget
        && let Err(e) = vias::remove(&key)
    {
        fail(&format!(
            "stopped, but the record was not forgotten: {}",
            said(&e)
        ));
    }
}
