//! easysql - a database toolbox that makes sql easy. Binary: `esql`.
//!
//! One tool over `psql`, `mysql` and `sqlite3` so you never dig through a
//! password manager, a wiki page or your own notes for a connection string
//! again.
//!
//!   esql                 Launch the TUI: connections, passwords, tunnels
//!   esql <name>          Open it (anything unknown is a saved connection)
//!   esql ls              List every saved connection  (-v shows targets)
//!   esql self update     Reinstall the latest release from crates.io
//!   esql self check      Ask crates.io whether a newer release exists
//!
//! The philosophy, and it is the only rule that matters here: easysql is a
//! front end, never a client. It writes the files psql and mysql already read
//! (`~/.pg_service.conf`, `~/.pgpass`, `~/.my.cnf`) and then hands the terminal
//! to the real program, so `\c`, `\dt`, your `.psqlrc` and every other tool on
//! the machine keep working, and uninstalling this costs you nothing.

mod clip;
mod commands;
mod creds;
mod engines;
mod history;
mod ini;
mod reach;
mod settings;
mod sshhosts;
mod tui;
mod tunnels;
mod vias;

use clap::{Parser, Subcommand};

/// Shown under `esql --help`. The command list can't convey the two things that
/// aren't subcommands: bare `esql` opens the TUI, and any saved name connects.
const AFTER: &str = concat!(
    "\
Two more ways to run it (not subcommands):
  esql                 open the toolbox (TUI): connections, passwords, tunnels
  esql <name>          open a saved connection (e.g. `esql prod`, or `esql pg:prod`)

The toolbox is where passwords, tunnels and adding/editing connections live.
Run `esql <command> --help` for a command's details.",
    "\n\n",
    env!("CARGO_PKG_REPOSITORY"),
    "\ncontributors: ",
    env!("CARGO_PKG_AUTHORS"),
);

/// `-V` stays a bare version string for scripts; `--version` spells out the
/// license, where it lives, and who's contributed. Every field comes from
/// Cargo.toml, so none of it can drift from the manifest.
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "\n",
    env!("CARGO_PKG_LICENSE"),
    "  ",
    env!("CARGO_PKG_REPOSITORY"),
    "\ncontributors: ",
    env!("CARGO_PKG_AUTHORS"),
);

#[derive(Parser)]
#[command(
    name = "easysql",
    bin_name = "esql",
    version,
    long_version = LONG_VERSION,
    about,
    after_help = AFTER
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List every saved connection, from every engine
    ///   -v   also show where each one connects
    #[command(verbatim_doc_comment)]
    Ls(commands::ls::Args),
    /// Manage easysql itself: `self update` reinstalls, `self check` looks for a newer release
    #[command(name = "self", subcommand)]
    Selfie(commands::selfcmd::Cmd),
    /// Any other word is a saved connection, opened in its own client
    #[command(external_subcommand)]
    Connect(Vec<String>),
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        // Bare `esql` → the toolbox.
        None => {
            if let Err(e) = tui::run() {
                eprintln!("esql: {e}");
                std::process::exit(1);
            }
        }
        Some(Cmd::Ls(args)) => commands::ls::run(args),
        Some(Cmd::Selfie(cmd)) => commands::selfcmd::run(cmd),
        Some(Cmd::Connect(args)) => commands::connect::run(args),
    }
}
