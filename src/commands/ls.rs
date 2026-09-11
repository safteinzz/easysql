//! `esql ls` - every saved connection, one per line, across every engine. This
//! is the `grep '^\[' ~/.pg_service.conf` you keep rewriting, built in.
//!
//! Alphabetical by engine then name, never by history: a script's output must
//! not depend on what you did yesterday.

use crate::engines;
use colored::Colorize;

#[derive(clap::Args)]
pub struct Args {
    /// Also show where each connection points (user@host:port/db)
    #[arg(short, long)]
    pub verbose: bool,
}

pub fn run(args: Args) {
    let conns = engines::list();

    if conns.is_empty() {
        println!(
            "{}",
            "No saved connections yet. Run `esql` and press `c` to add one.".dimmed()
        );
        return;
    }

    if args.verbose {
        // Pad the name column so the targets line up.
        let width = conns.iter().map(|c| c.name.len()).max().unwrap_or(0);
        let ew = conns
            .iter()
            .map(|c| c.engine.label().len())
            .max()
            .unwrap_or(0);
        for c in &conns {
            // Words rather than the TUI's colour, because this is the line a
            // script or an agent reads to pick the connection it may use.
            let ro = if c.read_only() { "  (read-only)" } else { "" };
            println!(
                "  {:ew$}  {:width$}  {}{}",
                c.engine.label().dimmed(),
                c.name.bold(),
                c.target().dimmed(),
                ro.cyan()
            );
        }
    } else {
        for c in &conns {
            println!("{}", c.name);
        }
    }
}
