//! easysql - a database toolbox that makes sql easy. Binary: `esql`.
//!
//! One tool over `psql`, `mysql` and `sqlite3` so you never dig through a
//! password manager, a wiki page or your own notes for a connection string
//! again.
//!
//! This file is the clap `Cmd` enum and the dispatch match; what you can run is
//! `esql --help`, which renders from the manifest, those doc comments and
//! `AFTER`, and is the only copy of that list.
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
mod snippets;
mod sshhosts;
mod tui;
mod tunnels;
mod vias;

use clap::{Parser, Subcommand};

/// clap's own layout with one change: `{before-help}` moves from above the
/// description to just under `Usage:`, so the shapes block lands on top of the
/// command list rather than on top of the screen.
const TEMPLATE: &str =
    "{about-with-newline}\n{usage-heading} {usage}\n\n{before-help}{all-args}{after-help}\n";

/// Shown under `esql --help`: the shapes clap cannot list, because most of this
/// tool is not a subcommand, and then the contract a script needs. Read top to
/// bottom by somebody - or something - looking for the one line that answers
/// "how do I ask this database a question", so that line is in the block rather
/// than in prose below it.
const WAYS: &str = "\x1b[1mWays to run it (not subcommands):\x1b[0m
  esql                       open the toolbox (TUI): connections, passwords, tunnels, saved queries
  esql <name>                open a saved connection (e.g. `esql prod`, or `esql pg:prod`)
  esql <name>/<db>           the same connection, another database on that server
  esql <name> 'select 1'     run one query and exit, the way `ssh host 'cmd'` does
  esql <name> :<query>       run a saved query (see the Snippets tab)
  esql <name> [client args]  anything else goes to the client (`esql prod -c 'select 1'`)";

/// The rest of the block: what a script can expect, then where to look next.
const AFTER: &str = concat!(
    "\
Rows go to stdout and easysql's own words to stderr, so a pipe carries only
data, and the exit code is the client's own: a `psql` that refuses to connect
exits 2 for its own reasons, not for easysql's. Everything after `--` reaches
the client untouched.
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
    // The shapes come first: this is a bare-first binary, so the command list is
    // the leftovers and burying them above it answers the wrong question first.
    help_template = TEMPLATE,
    before_help = WAYS,
    after_help = AFTER
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List every saved connection, from every engine
    ///   -v   also show where each connection points
    #[command(verbatim_doc_comment)]
    Ls(commands::ls::Args),
    /// Manage easysql itself: `self update` reinstalls, `self check` looks for a newer release
    #[command(name = "self", subcommand)]
    Selfie(commands::selfcmd::Cmd),
    /// Any other word is a saved connection (`<name>`, or `<name>/<database>`)
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
