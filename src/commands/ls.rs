//! `esql ls` - every saved connection, one per line, across every engine. This
//! is the `grep '^\[' ~/.pg_service.conf` you keep rewriting, built in.
//!
//! Alphabetical by engine then name, never by history: a script's output must
//! not depend on what you did yesterday.

use super::{json_array, json_opt, json_str};
use crate::engines;
use colored::Colorize;

/// Under `esql ls --help`: what `--json` prints, so a script never guesses.
pub const JSON_FIELDS: &str = "\
--json prints an array with one object per connection, and these names are fixed:
  engine     pg, my, lite or ms
  name, key  what you type after `esql`, and `<engine>:<name>`
  host, port, database, user
             as the block says them, null when it leaves one out
  read_only  true when every session refuses writes
  password   true or false for whether one is saved, null for sqlite
  tunnel     on or off for the forward it needs, null when it needs none
  via        the ssh host that forward goes through, or null";

#[derive(clap::Args)]
pub struct Args {
    /// Also show where each connection points (user@host:port/db)
    #[arg(short, long)]
    pub verbose: bool,
    #[command(flatten)]
    pub json: super::Json,
}

pub fn run(args: Args) {
    let conns = engines::list();

    if args.json.json {
        let creds = crate::creds::list();
        let rows: Vec<String> = conns
            .iter()
            .map(|c| {
                let password = match c.engine {
                    engines::Engine::Sqlite => "null",
                    _ if creds.iter().any(|cr| cr.covers(c)) => "true",
                    _ => "false",
                };
                let via = crate::vias::get(&c.key());
                let tunnel = via.as_ref().map(|v| {
                    match crate::tunnels::carrying_via(v) {
                        Some(_) => "on",
                        None => "off",
                    }
                });
                let opt = |s: &str| json_opt((!s.is_empty()).then_some(s));
                format!(
                    "{{\"engine\": {}, \"name\": {}, \"key\": {}, \"host\": {}, \"port\": {}, \"database\": {}, \"user\": {}, \"read_only\": {}, \"password\": {password}, \"tunnel\": {}, \"via\": {}}}",
                    json_str(c.engine.slug()),
                    json_str(&c.name),
                    json_str(&c.key()),
                    opt(&c.host),
                    opt(&c.port),
                    opt(&c.database),
                    opt(&c.user),
                    c.read_only(),
                    json_opt(tunnel),
                    json_opt(via.as_ref().map(|v| v.host.as_str())),
                )
            })
            .collect();
        println!("{}", json_array(&rows));
        return;
    }

    if conns.is_empty() {
        eprintln!(
            "{}",
            "esql: no saved connections yet: `esql conn add <engine> <name>` adds one".dimmed()
        );
        return;
    }

    let names: Vec<String> = conns.iter().map(|c| typed(c, &conns)).collect();
    if args.verbose {
        // Pad the name column so the targets line up.
        let width = names.iter().map(|n| n.len()).max().unwrap_or(0);
        let ew = conns
            .iter()
            .map(|c| c.engine.label().len())
            .max()
            .unwrap_or(0);
        for (c, name) in conns.iter().zip(&names) {
            // Words rather than the TUI's colour, because this is the line a
            // script or an agent reads to pick the connection it may use.
            let ro = if c.read_only() { "  (read-only)" } else { "" };
            println!(
                "  {:ew$}  {:width$}  {}{}",
                c.engine.label().dimmed(),
                name.bold(),
                c.target().dimmed(),
                ro.cyan()
            );
        }
    } else {
        for name in &names {
            println!("{name}");
        }
    }
}

/// The name as `esql` takes it: bare, or `<engine>:<name>` when another engine
/// has the same name or it is also a command, since the bare word would then
/// be refused or open something else.
fn typed(c: &engines::Conn, all: &[engines::Conn]) -> String {
    let shared = all.iter().filter(|o| o.name == c.name).count() > 1;
    match shared || super::COMMANDS.contains(&c.name.as_str()) {
        true => c.key(),
        false => c.name.clone(),
    }
}
