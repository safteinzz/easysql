//! `esql <name> [client args…]` - just open it. Anything that isn't a known
//! subcommand is treated as a saved connection, looked up across every engine
//! and handed to that engine's own client, so `\c`, `\dt` and your `.psqlrc`
//! work exactly as they always did.
//!
//! We `exec` (replace this process) so the client owns the terminal cleanly.

use colored::Colorize;

pub fn run(args: Vec<String>) {
    if args.is_empty() {
        eprintln!(
            "{}",
            "esql: no connection given. Try `esql ls` or just `esql`.".red()
        );
        std::process::exit(2);
    }

    let conn = match crate::engines::find(&args[0]) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", format!("esql: {e}").red());
            std::process::exit(2);
        }
    };

    // A connection that only works through a forward brings the forward back
    // with it: the process died with the last reboot, but what it was is
    // remembered. Said out loud rather than done silently, because it starts a
    // background ssh that outlives this command.
    match crate::vias::ensure(&conn.key()) {
        Some(Ok(msg)) => eprintln!("{}", format!("esql: {msg}").dimmed()),
        Some(Err(e)) => {
            eprintln!(
                "{}",
                format!("esql: could not reopen the tunnel: {e}").red()
            );
            eprintln!(
                "{}",
                "      the connection points at it, so this will not connect.".dimmed()
            );
            std::process::exit(1);
        }
        None => {}
    }

    // Stamp the connect before handing the terminal over: `exec` never comes
    // back, so there is no "after" in which to record it.
    crate::history::record(&conn.key());

    // The same argv the TUI and the wizard preview build, so both ways in
    // behave identically and the preview can never lie about what runs.
    let settings = crate::settings::load();
    let mut argv = conn.connect_argv(&settings);
    argv.extend(args.into_iter().skip(1));
    let (program, rest) = argv.split_first().expect("connect_argv is never empty");
    let program = program.clone();
    let rest: Vec<String> = rest.to_vec();

    // Say it up front rather than letting exec fail with an errno. `cargo
    // install easysql` could not have brought the client along - it is not Rust
    // - so the least this can do is name the one command that would.
    if !crate::engines::on_path(&program) {
        eprintln!(
            "{}",
            format!("esql: {program} is not installed, and easysql is only a front end for it.")
                .red()
        );
        eprintln!(
            "      {}  {}",
            "install it:".dimmed(),
            crate::engines::install_hint(conn.engine).bold()
        );
        // Only promise the offer when there is one: SQL Server has no distro
        // package, so nothing in the TUI can install it for you either.
        if crate::engines::install_argv(conn.engine).is_some() {
            eprintln!(
                "      {}",
                "or run `esql` and press Enter on it, which offers to do this for you.".dimmed()
            );
        }
        std::process::exit(127);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // exec never returns on success; the client takes over this PID and terminal.
        let err = std::process::Command::new(&program).args(&rest).exec();
        eprintln!("{}", format!("esql: could not run {program}: {err}").red());
        eprintln!(
            "{}",
            format!("install it: {}", crate::engines::install_hint(conn.engine)).dimmed()
        );
        std::process::exit(127);
    }

    #[cfg(not(unix))]
    {
        match std::process::Command::new(&program).args(&rest).status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(e) => {
                eprintln!("{}", format!("esql: could not run {program}: {e}").red());
                std::process::exit(127);
            }
        }
    }
}
