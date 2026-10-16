//! `esql completions <shell>`: print the script that turns on Tab completion,
//! or with `--add`, put the one line that loads it into the shell's startup file.

use super::{DryRun, fail, step};
use colored::Colorize;
use std::io::Write;

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl Shell {
    fn name(self) -> &'static str {
        match self {
            Shell::Bash => "bash",
            Shell::Zsh => "zsh",
            Shell::Fish => "fish",
        }
    }

    /// The file the shell reads at every start, relative to `$HOME`.
    fn rc_file(self) -> &'static str {
        match self {
            Shell::Bash => ".bashrc",
            Shell::Zsh => ".zshrc",
            Shell::Fish => ".config/fish/config.fish",
        }
    }

    /// The line in `rc_file` that loads completion at every start.
    fn rc_line(self) -> &'static str {
        match self {
            Shell::Bash => "source <(esql completions bash)",
            Shell::Zsh => "source <(esql completions zsh)",
            Shell::Fish => "esql completions fish | source",
        }
    }

    /// Lines that already load it; a commented one does not.
    fn loads_it(self, line: &str) -> bool {
        let line = line.trim();
        !line.starts_with('#')
            && (line == self.rc_line() || line.contains(&format!("COMPLETE={} esql", self.name())))
    }
}

#[derive(clap::Args)]
pub struct Args {
    /// Which shell to print the completion script for
    #[arg(value_enum, value_name = "bash|zsh|fish")]
    pub shell: Shell,

    /// Instead of printing, add the line that loads it to your shell's startup file
    #[arg(long)]
    pub add: bool,

    #[command(flatten)]
    pub dry: DryRun,
}

pub fn run(args: Args, cmd: clap::Command) {
    if args.add {
        add(args.shell, args.dry.dry_run);
        return;
    }
    let mut out = std::io::stdout().lock();
    if let Err(e) = crate::completion::write_registration(&cmd, args.shell.name(), &mut out)
        .and_then(|_| out.flush())
    {
        fail(&format!("could not print the script: {}", e.kind()));
    }
}

/// Append `rc_line` to the startup file once, leaving every other line alone.
fn add(shell: Shell, dry: bool) {
    let Some(home) = dirs::home_dir() else {
        fail("could not find your home directory; set `HOME` and try again");
    };
    let rc = home.join(shell.rc_file());
    let shown = format!("~/{}", shell.rc_file());
    // Lossy, so one byte that is not UTF-8 cannot make the file read as empty.
    let existing = std::fs::read(&rc)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    if existing.lines().any(|l| shell.loads_it(l)) {
        step(&format!("nothing to add: {shown} already loads it"));
        return;
    }
    step(&format!("add `{}` to {shown}", shell.rc_line()));
    if dry {
        return;
    }
    if let Some(dir) = rc.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        fail(&format!(
            "could not create `{}`: {}",
            dir.display(),
            e.kind()
        ));
    }
    // A file that does not end in a newline would glue our comment onto its last line.
    let gap = if existing.is_empty() {
        ""
    } else if existing.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let block = format!("{gap}# easysql tab completion\n{}\n", shell.rc_line());
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&rc)
        .and_then(|mut f| f.write_all(block.as_bytes()));
    match written {
        Ok(()) => eprintln!(
            "{}",
            "esql: open a new shell and Tab completes connections, commands and their names"
                .dimmed()
        ),
        Err(e) => fail(&format!(
            "could not write {shown} ({}), so add `{}` to it by hand",
            e.kind(),
            shell.rc_line()
        )),
    }
}
