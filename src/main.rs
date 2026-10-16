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
//! The TUI is where a person finds things; the CLI holds the same actions for
//! a script or an agent, one noun per tab (`conn`, `pw`, `tunnel`, `snippet`,
//! `set`), so nothing the toolbox does needs a terminal or a hand edit.
//!
//! The philosophy, and it is the only rule that matters here: easysql is a
//! front end, never a client. It writes the files psql and mysql already read
//! (`~/.pg_service.conf`, `~/.pgpass`, `~/.my.cnf`) and then hands the terminal
//! to the real program, so `\c`, `\dt`, your `.psqlrc` and every other tool on
//! the machine keep working, and uninstalling this costs you nothing.

mod clip;
mod commands;
mod completion;
mod creds;
mod engines;
mod history;
mod ini;
mod markdown;
mod reach;
mod settings;
mod snippets;
mod sshhosts;
mod tui;
mod tunnels;
mod vias;

use clap::{CommandFactory, Parser, Subcommand};

/// clap's own layout with one change: `{before-help}` moves from above the
/// description to just under `Usage:`, so the shapes block lands on top of the
/// command list rather than on top of the screen.
const TEMPLATE: &str =
    "{about-with-newline}\n{usage-heading} {usage}\n\n{before-help}{all-args}{after-help}\n";

/// Shown under `esql --help`, above the command list: the shapes clap cannot
/// list, because most of this tool is not a subcommand.
const WAYS: &str = "\x1b[1mWays to run it (not subcommands):\x1b[0m
  esql                       open the toolbox (TUI): connections, passwords, tunnels, saved queries
  esql <name>                open a saved connection (`esql prod`, or `esql pg:prod` when the name is shared or a command)
  esql <name>/<db>           the same connection, another database on that server
  esql <name> 'select 1'     run one query and exit, the way `ssh host 'cmd'` does
  esql <name> :<query>       run a saved query (`esql snippet ls` lists them)
  esql <name> [client args]  anything else goes to the client (`esql prod -c 'select 1'`)
  esql --md <name> ...       any of the above, its rows printed as markdown tables
  esql --dump <name>[/<db>]  print the database as SQL (`> app.sql`), any words after it going to the dump tool";

/// The rest of the block: the words the output uses, what a script can
/// expect, then where to look next.
const AFTER: &str = concat!(
    "\
What it prints:
  ls         one name per line as `esql` takes it, `<engine>:<name>` when shared
  ls -v      `(read-only)` after a connection whose sessions refuse writes
  ls --json  one object per connection, its field names fixed (`esql ls --help`)
  set        each key, its value, what it does and what it takes (a|b, or typed)
  steps      one line per change, with a password always written `(set)`

A connect hands the terminal to the client: once it starts, stdout, stderr and
the exit code are the client's own, rows or the dump on stdout. Before that
easysql exits 2 for a name, snippet, `/db`, `--md` or `--dump` it cannot use,
127 when there is no client to run, and 1 when a tunnel it needs will not open
or a read-only connection would not be read-only. Everything after `--` reaches
the client untouched. Every other command prints its result on stdout and its
errors on stderr, exiting 1 on a failure (`self check` has codes of its own,
in `esql self --help`): a command that changes something prints one line per
step, and -n prints the same lines and changes nothing. `rm` and `down` take
one name each, so every delete names what it loses, and a script loops over
`ls` for more.
Nothing asks without a terminal: a delete asks first only on one, and -y
answers it, and `pw set` reads the password from --stdin, or from a prompt that
does not echo.
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
    help_template = TEMPLATE,
    before_help = WAYS,
    after_help = AFTER,
    add = clap_complete::engine::SubcommandCandidates::new(completion::conns)
)]
struct Cli {
    /// Print the rows as markdown tables, for SQL Server only under `usql`; goes before the connection name
    #[arg(long)]
    md: bool,
    /// Print the database as SQL with pg_dump, mysqldump or sqlite3, not for SQL Server; goes before the connection name
    #[arg(long, conflicts_with = "md")]
    dump: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// List every saved connection, from every engine
    ///   -v      also show where each connection points
    ///   --json  every connection with its password and tunnel state, for a script
    #[command(verbatim_doc_comment, after_help = commands::ls::JSON_FIELDS)]
    Ls(commands::ls::Args),
    /// Add, change or delete a saved connection, in the file its client reads  <COMMAND>
    ///   conn add <ENGINE> <NAME>  a new one: --host, --port, --db, --user, or --file for sqlite
    ///   conn edit <NAME>          change only the flags given, "" removes that line (--rename)
    ///   conn rm <NAME>            delete it, with its tunnel record and kept password
    #[command(verbatim_doc_comment, subcommand)]
    Conn(commands::conn::Cmd),
    /// Save or forget the password a connection logs in with, never shown  <COMMAND>
    ///   pw set <NAME>  from --stdin, or a prompt that does not echo on a terminal
    ///   pw rm <NAME>   forget it
    #[command(verbatim_doc_comment, subcommand)]
    Pw(commands::pw::Cmd),
    /// Open and stop the ssh forward a connection needs, like the Tunnels tab  <COMMAND>
    ///   tunnel up <NAME>    open it (--via HOST), remember it, point the connection at it
    ///   tunnel down <NAME>  stop it and keep the record (--forget drops it)
    #[command(verbatim_doc_comment, subcommand)]
    Tunnel(commands::tunnel::Cmd),
    /// Keep the queries `esql <name> :<query>` runs, like the Snippets tab  <COMMAND>
    ///   snippet ls                every saved query (--json)
    ///   snippet add <NAME> [SQL]  save one, the SQL given or read from stdin
    ///   snippet rm <NAME>         delete its file
    #[command(verbatim_doc_comment, subcommand)]
    Snippet(commands::snippet::Cmd),
    /// List the settings, or change one  [KEY] [VALUE]
    ///   esql set backups 5        keep five copies of each file easysql rewrites
    ///   esql set hints --default  put one back the way it ships
    #[command(verbatim_doc_comment, after_help = commands::set::JSON_FIELDS)]
    Set(commands::set::Args),
    /// Print the script that turns on Tab completion  <bash|zsh|fish>
    ///   --add   add the line that loads it to your shell's startup file
    #[command(verbatim_doc_comment)]
    Completions(commands::completions::Args),
    /// Manage easysql itself: `self update` reinstalls, `self check` looks for a newer release
    #[command(name = "self", subcommand)]
    Selfie(commands::selfcmd::Cmd),
    /// Any other word is a saved connection (`<name>`, or `<name>/<database>`)
    #[command(external_subcommand)]
    Connect(Vec<String>),
}

fn main() {
    completion::handle(Cli::command);
    let cli = Cli::parse();

    if cli.md && !matches!(cli.command, Some(Cmd::Connect(_))) {
        eprintln!(
            "esql: `--md` formats a query's rows, so it needs a connection: `esql --md <name> 'select 1'`."
        );
        std::process::exit(2);
    }
    if cli.dump && !matches!(cli.command, Some(Cmd::Connect(_))) {
        eprintln!("esql: `--dump` needs a connection: `esql --dump <name> > dump.sql`.");
        std::process::exit(2);
    }
    // Every command that rewrites a user's file keeps the copies the setting asks for.
    if cli.command.is_some() {
        ini::set_backups(settings::load().backups);
    }

    match cli.command {
        // Bare `esql` → the toolbox.
        None => {
            if let Err(e) = tui::run() {
                eprintln!("esql: {e}");
                std::process::exit(1);
            }
        }
        Some(Cmd::Ls(args)) => commands::ls::run(args),
        Some(Cmd::Conn(cmd)) => commands::conn::run(cmd),
        Some(Cmd::Pw(cmd)) => commands::pw::run(cmd),
        Some(Cmd::Tunnel(cmd)) => commands::tunnel::run(cmd),
        Some(Cmd::Snippet(cmd)) => commands::snippet::run(cmd),
        Some(Cmd::Set(args)) => commands::set::run(args),
        Some(Cmd::Completions(args)) => commands::completions::run(args, Cli::command()),
        Some(Cmd::Selfie(cmd)) => commands::selfcmd::run(cmd),
        Some(Cmd::Connect(args)) => commands::connect::run(args, cli.md, cli.dump),
    }
}
