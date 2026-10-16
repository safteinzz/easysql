//! Tab completion, served by the binary itself through clap_complete's env
//! mode: `esql completions bash` (or `COMPLETE=bash esql`) prints the script
//! that registers it, and every Tab then runs `esql` with `COMPLETE` set. The
//! first word completes to commands and saved connections, and a name argument
//! to the connections, saved queries or settings it takes.

use clap_complete::CompletionCandidate;
use clap_complete::env::{Bash, Elvish, EnvCompleter, Fish, Powershell, Shells, Zsh};
use std::ffi::{OsStr, OsString};

const SHELLS: Shells<'static> = Shells(&[
    &PastName(Bash),
    &PastName(Elvish),
    &PastName(Fish),
    &PastName(Powershell),
    &PastName(Zsh),
]);

/// Answers a Tab and exits when `COMPLETE` is set; returns at once otherwise.
pub fn handle(factory: fn() -> clap::Command) {
    clap_complete::CompleteEnv::with_factory(factory)
        .shells(SHELLS)
        .complete();
}

/// The script `COMPLETE=<shell> esql` prints, for `shell` as clap names it.
/// `esql` calls itself back by the name it was run as, made absolute when that
/// name is a relative path, exactly as clap_complete does.
pub fn write_registration(
    cmd: &clap::Command,
    shell: &str,
    buf: &mut dyn std::io::Write,
) -> std::io::Result<()> {
    let completer = SHELLS
        .completer(shell)
        .ok_or_else(|| std::io::Error::other(format!("no completion for `{shell}`")))?;
    let mut me = std::path::PathBuf::from(std::env::args_os().next().unwrap_or("esql".into()));
    if me.components().count() > 1 && me.is_relative() {
        me = std::env::current_dir()?.join(me);
    }
    let bin = cmd.get_bin_name().unwrap_or_else(|| cmd.get_name());
    completer.write_registration("COMPLETE", cmd.get_name(), bin, &me.to_string_lossy(), buf)
}

/// Every saved connection, for the first word, with where it points as help.
pub fn conns() -> Vec<CompletionCandidate> {
    crate::engines::list()
        .into_iter()
        .map(|c| CompletionCandidate::new(&c.name).help(Some(c.target().into())))
        .collect()
}

fn starting_with(all: Vec<CompletionCandidate>, current: &OsStr) -> Vec<CompletionCandidate> {
    let typed = current.to_string_lossy();
    all.into_iter()
        .filter(|c| c.get_value().to_string_lossy().starts_with(typed.as_ref()))
        .collect()
}

/// The connections that start with what is typed, for an argument that takes one.
pub fn conn_names(current: &OsStr) -> Vec<CompletionCandidate> {
    starting_with(conns(), current)
}

/// The saved queries that start with what is typed.
pub fn snippet_names(current: &OsStr) -> Vec<CompletionCandidate> {
    let all = crate::snippets::list()
        .into_iter()
        .map(|s| CompletionCandidate::new(&s.name).help(Some(s.summary().into())))
        .collect();
    starting_with(all, current)
}

/// The setting keys that start with what is typed.
pub fn setting_keys(current: &OsStr) -> Vec<CompletionCandidate> {
    let all = crate::settings::Settings::default()
        .rows()
        .into_iter()
        .map(|r| CompletionCandidate::new(r.key).help(Some(r.help.into())))
        .collect();
    starting_with(all, current)
}

/// A shell's completer, silenced once the cursor is past the connection word:
/// what follows `esql prod` belongs to the client, while clap's engine never
/// enters an external subcommand and would offer the names and commands again.
struct PastName<S>(S);

impl<S: EnvCompleter> EnvCompleter for PastName<S> {
    fn name(&self) -> &'static str {
        self.0.name()
    }
    fn is(&self, name: &str) -> bool {
        self.0.is(name)
    }
    fn write_registration(
        &self,
        var: &str,
        name: &str,
        bin: &str,
        completer: &str,
        buf: &mut dyn std::io::Write,
    ) -> Result<(), std::io::Error> {
        self.0.write_registration(var, name, bin, completer, buf)
    }
    fn write_complete(
        &self,
        cmd: &mut clap::Command,
        args: Vec<OsString>,
        current_dir: Option<&std::path::Path>,
        buf: &mut dyn std::io::Write,
    ) -> Result<(), std::io::Error> {
        // Fish and PowerShell pass no index and always complete the last word.
        let index = std::env::var("_CLAP_COMPLETE_INDEX")
            .ok()
            .and_then(|i| i.parse().ok())
            .unwrap_or(args.len().saturating_sub(1));
        if past_name(cmd, &args, index) {
            return Ok(());
        }
        self.0.write_complete(cmd, args, current_dir, buf)
    }
}

/// Whether a word before `index` is a connection rather than a command.
fn past_name(cmd: &mut clap::Command, args: &[OsString], index: usize) -> bool {
    cmd.build();
    let first = args
        .iter()
        .take(index)
        .skip(1)
        .find(|w| !w.to_string_lossy().starts_with('-'));
    first.is_some_and(|w| w.to_str().is_none_or(|w| cmd.find_subcommand(w).is_none()))
}
