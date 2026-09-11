//! `esql <name> [client args…]` - just open it. Anything that isn't a known
//! subcommand is treated as a saved connection, looked up across every engine
//! and handed to that engine's own client, so `\c`, `\dt` and your `.psqlrc`
//! work exactly as they always did.
//!
//! Four shapes, decided by what follows the name: nothing opens a session,
//! `:word` runs a saved query, a word that is not a flag is SQL to run and
//! exit (`ssh host 'cmd'`, for databases), and anything else is the client's.
//! `<name>/<db>` picks another database on the same server first.
//!
//! We `exec` (replace this process) so the client owns the terminal cleanly.

use colored::Colorize;

/// A query or a `/db` is spelled in the client's own flags - `-c`, `-e`, `-Q`,
/// and `dbname=` inside a libpq conninfo - so a configured client that cannot
/// take them is stepped around rather than handed an argument it will reject:
/// the engine's own client runs this one, and a plain session still opens in
/// the configured client. A wrapper that still *is* that client
/// (`docker exec -it db psql`) speaks them and is left alone.
fn use_default_client_for_one_shot(conn: &crate::engines::Conn, s: &mut crate::settings::Settings) {
    if crate::engines::speaks_client_flags(conn.engine, s) {
        return;
    }
    let configured = conn.engine.client_argv(s).join(" ");
    let default = conn.engine.default_client();
    if !crate::engines::on_path(default) {
        eprintln!(
            "{}",
            format!(
                "esql: `{configured}` cannot take a query or another database, and `{default}` is not installed."
            )
            .red()
        );
        eprintln!(
            "{}",
            format!(
                "      install it: {}",
                crate::engines::install_hint(conn.engine)
            )
            .dimmed()
        );
        std::process::exit(127);
    }
    // Said out loud: the command that ran is not the one Settings names, and a
    // silent swap would make a `\set` or a wrapper's environment look broken.
    eprintln!(
        "{}",
        format!(
            "esql: `{configured}` cannot take a query or another database, so this runs with `{default}`."
        )
        .dimmed()
    );
    s.set(conn.engine.client_setting(), default);
}

/// Hand the client a query to run and exit, so stdout carries rows, easysql's
/// own words stay on stderr and the exit status is the client's own. sqlite3
/// takes it as a bare argument, so it has no flag.
fn push_query(argv: &mut Vec<String>, conn: &crate::engines::Conn, sql: String) {
    if let Some(flag) = conn.engine.query_flag() {
        argv.push(flag.to_string());
    }
    argv.push(sql);
}

pub fn run(args: Vec<String>) {
    if args.is_empty() {
        eprintln!(
            "{}",
            "esql: no connection given. Try `esql ls` or just `esql`.".red()
        );
        std::process::exit(2);
    }

    let (conn, db) = match crate::engines::find_target(&args[0]) {
        Ok(found) => found,
        Err(e) => {
            eprintln!("{}", format!("esql: {e}").red());
            std::process::exit(2);
        }
    };

    if db.is_some() && conn.engine == crate::engines::Engine::Sqlite {
        eprintln!(
            "{}",
            format!(
                "esql: `{}` is a sqlite file, and another database is another file.",
                conn.name
            )
            .red()
        );
        eprintln!(
            "{}",
            "      save that file as its own connection with `c` in `esql`.".dimmed()
        );
        std::process::exit(2);
    }

    let rest: Vec<String> = args.into_iter().skip(1).collect();
    // Nothing after the connection name means a session is being opened rather
    // than a one-shot query. Asked before `rest` is consumed below.
    let session = rest.is_empty();
    let first = rest.first().cloned().unwrap_or_default();
    // `esql prod :slots` runs the saved query instead of opening a session. The
    // colon is psql's own sigil for exactly this, and it cannot collide with a
    // client flag or a database name. Resolved before anything with a side
    // effect, so a mistyped name opens no tunnel and stamps no history.
    let snippet: Option<String> = match first.strip_prefix(':') {
        Some(name) => match crate::snippets::get(name) {
            Some(s) => Some(s.sql),
            None => {
                eprintln!("{}", format!("esql: no snippet called '{name}'").red());
                eprintln!(
                    "{}",
                    "      `esql` and the Snippets tab list and create them.".dimmed()
                );
                std::process::exit(2);
            }
        },
        None => None,
    };
    let adhoc = !session && first != "--" && snippet.is_none() && !first.starts_with('-');

    // Before the tunnel and the history stamp, so a run refused for want of a
    // client leaves nothing behind. A flag the user typed themselves is theirs,
    // so a passthrough is not a one-shot here.
    let mut settings = crate::settings::load();
    if db.is_some() || snippet.is_some() || adhoc {
        use_default_client_for_one_shot(&conn, &mut settings);
    }

    if conn.engine == crate::engines::Engine::MsSql
        && settings.mssql_passwords
        && crate::engines::mssql::has_password(&conn.name)
        && !crate::engines::mssql::pass_is_private()
    {
        eprintln!(
            "{}",
            format!(
                "esql: `{}` can be read by others, so its password is not used: `chmod 600` it.",
                crate::ini::collapse_tilde(&crate::engines::mssql::pass_path().to_string_lossy())
            )
            .dimmed()
        );
    }

    if conn.engine == crate::engines::Engine::Sqlite
        && conn.read_only()
        && !crate::engines::speaks_client_flags(conn.engine, &settings)
    {
        eprintln!(
            "{}",
            format!(
                "esql: `{}` cannot open a file read-only, so this runs with `sqlite3`.",
                conn.engine.client_argv(&settings).join(" ")
            )
            .dimmed()
        );
    }

    // Say it up front rather than letting exec fail with an errno. `cargo
    // install easysql` could not have brought the client along - it is not Rust
    // - so the least this can do is name the one command that would.
    let program = conn.connect_argv_db(&settings, db.as_deref())[0].clone();
    if !crate::engines::on_path(&program) {
        eprintln!(
            "{}",
            format!("esql: `{program}` is not installed, and easysql is only a front end for it.")
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

    // After the tunnel, which the question has to travel through, and before
    // the history stamp, so a refused run leaves nothing behind.
    if conn.engine == crate::engines::Engine::Pg && conn.read_only() {
        let check =
            crate::engines::pg::check_read_only(&conn, db.as_deref(), settings.probe_timeout);
        if let Some((headline, fix)) = check.refusal(&conn.name) {
            eprintln!("{}", format!("esql: {headline}").red());
            eprintln!("{}", format!("      {fix}").dimmed());
            std::process::exit(1);
        }
        if let crate::engines::pg::ReadOnlyCheck::Unknown(said) = check {
            eprintln!(
                "{}",
                format!(
                    "esql: could not confirm `{}` is read-only: {said}",
                    conn.name
                )
                .dimmed()
            );
        }
    }

    // Stamp the connect before handing the terminal over: `exec` never comes
    // back, so there is no "after" in which to record it.
    crate::history::record(&conn.key());

    // The same argv the TUI and the wizard preview build, so both ways in
    // behave identically and the preview can never lie about what runs.
    let mut argv = conn.connect_argv_db(&settings, db.as_deref());

    if let Some(sql) = snippet.clone() {
        push_query(&mut argv, &conn, sql);
        argv.extend(rest.into_iter().skip(1));
    } else if first == "--" {
        // The escape hatch: an argument the client wants bare, which the rule
        // below would otherwise read as SQL.
        argv.extend(rest.into_iter().skip(1));
    } else if adhoc {
        // `esql prod 'select 1'`, the shape `ssh host 'cmd'` taught everyone.
        push_query(&mut argv, &conn, first);
        argv.extend(rest.into_iter().skip(1));
    } else {
        argv.extend(rest);
    }
    // Keep psql's own shortcuts in step on the way in. Doing it here rather
    // than only when a snippet is saved is what makes it self-healing: the
    // files are the source, and editing one by hand (or with `o`) would
    // otherwise leave `:name` expanding to yesterday's query.
    if conn.engine == crate::engines::Engine::Pg && snippet.is_none() {
        let _ = crate::snippets::sync_psqlrc();
    }

    // Only when a session is actually being opened: a one-shot query prints its
    // own result and a hint above it would just be noise in a pipe.
    if settings.hints && session {
        eprintln!(
            "{}",
            format!("  {}", crate::engines::hint_line(conn.engine, &settings)).dimmed()
        );
    }

    let (program, rest) = argv.split_first().expect("connect_argv is never empty");
    let program = program.clone();
    let rest: Vec<String> = rest.to_vec();

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // exec never returns on success; the client takes over this PID and terminal.
        let err = std::process::Command::new(&program)
            .args(&rest)
            .envs(conn.connect_env(&settings))
            .envs(conn.secret_env(&settings))
            .exec();
        eprintln!("{}", format!("esql: could not run {program}: {err}").red());
        eprintln!(
            "{}",
            format!("install it: {}", crate::engines::install_hint(conn.engine)).dimmed()
        );
        std::process::exit(127);
    }

    #[cfg(not(unix))]
    {
        match std::process::Command::new(&program)
            .args(&rest)
            .envs(conn.connect_env(&settings))
            .envs(conn.secret_env(&settings))
            .status()
        {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(e) => {
                eprintln!("{}", format!("esql: could not run {program}: {e}").red());
                std::process::exit(127);
            }
        }
    }
}
