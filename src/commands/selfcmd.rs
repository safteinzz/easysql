//! `esql self` - manage the installed binary itself.
//!
//! `self update` shells out to `cargo install easysql --force`, the crate name even when the command you type is shorter.
//! `self check` asks the registry for the latest release through `cargo search`,
//! so there is no HTTP client in the dependency tree and the answer comes from
//! the same registry `cargo install` would pull from.

use colored::Colorize;
use std::io::{self, Write};
use std::process::Command;

const CRATE: &str = "easysql";

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Reinstall the latest release from crates.io
    ///   -y   skip the confirmation prompt
    #[command(verbatim_doc_comment)]
    Update(UpdateArgs),
    /// Ask crates.io whether a newer release exists, without installing anything
    Check,
}

#[derive(clap::Args)]
pub struct UpdateArgs {
    /// Skip the confirmation prompt
    #[arg(short, long)]
    pub yes: bool,
}

pub fn run(cmd: Cmd) {
    match cmd {
        Cmd::Update(args) => update(args),
        Cmd::Check => check(),
    }
}

fn update(args: UpdateArgs) {
    if !args.yes && !confirm() {
        println!("{}", "Aborted.".dimmed());
        return;
    }

    println!(
        "{} {}\n",
        "Updating easysql via".dimmed(),
        "cargo install easysql --force".bold()
    );

    match Command::new("cargo")
        .args(["install", CRATE, "--force"])
        .status()
    {
        Ok(status) if status.success() => {
            println!("\n{}", "✓ easysql is up to date.".green());
        }
        Ok(status) => {
            eprintln!("\n{}", "✗ update failed.".red());
            std::process::exit(status.code().unwrap_or(1));
        }
        Err(e) => {
            eprintln!("{} {e}", "esql: could not run cargo:".red());
            eprintln!(
                "{}",
                "is cargo installed and on your PATH? (https://rustup.rs)".dimmed()
            );
            std::process::exit(127);
        }
    }
}

/// Compare the installed version with the newest one on crates.io. Nothing is
/// downloaded or written, so this is safe to run on a machine you do not want to
/// change.
fn check() {
    let current = env!("CARGO_PKG_VERSION");
    match latest() {
        Ok(latest) if newer(&latest, current) => {
            println!(
                "{} {} {}",
                format!("easysql {latest}").bold(),
                "is available, you have".dimmed(),
                current.bold()
            );
            println!("{} {}", "run".dimmed(), "esql self update".bold());
        }
        Ok(_) => println!(
            "{} {}",
            format!("easysql {current}").bold(),
            "is the latest release.".dimmed()
        ),
        Err(e) => {
            eprintln!("{} {e}", "esql: could not reach crates.io:".red());
            std::process::exit(1);
        }
    }
}

/// `cargo search` prints `easysql = "X.Y.Z"    # description` for an exact name
/// match, which is the whole reason no HTTP client is needed here.
fn latest() -> Result<String, String> {
    let out = Command::new("cargo")
        .args(["search", CRATE, "--limit", "1"])
        .output()
        .map_err(|e| format!("could not run cargo: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let prefix = format!("{CRATE} = \"");
    text.lines()
        .find_map(|l| l.strip_prefix(&prefix))
        .and_then(|rest| rest.split('"').next())
        .map(str::to_string)
        .ok_or_else(|| format!("the registry did not list `{CRATE}`"))
}

/// Compare dotted versions field by field, so `0.10.0` correctly beats `0.9.9`
/// where a plain string compare would not.
fn newer(a: &str, b: &str) -> bool {
    let fields = |v: &str| {
        v.split(['.', '-'])
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    fields(a) > fields(b)
}

/// Ask the user to confirm. Defaults to No, so a bare Enter cancels.
fn confirm() -> bool {
    print!(
        "{} {} ",
        "Update easysql to the latest release via cargo?".bold(),
        "[y/N]".dimmed()
    );
    io::stdout().flush().ok();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        return false;
    }
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}
