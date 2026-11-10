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
//! We `exec` (replace this process) so the client owns the terminal cleanly,
//! except under `--md`, where the client runs as a child so its rows can be
//! redrawn as markdown, and for a usql run holding a passfile or a service
//! file, which a child has to outlive so it can be deleted. `--dump` and
//! `--restore` exec the engine's dump and restore tools instead.

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
                crate::engines::install_hint(conn.engine, s)
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
fn push_query(
    argv: &mut Vec<String>,
    conn: &crate::engines::Conn,
    s: &crate::settings::Settings,
    sql: String,
) {
    if let Some(flag) = conn.engine.query_flag(s) {
        argv.push(flag.to_string());
    }
    argv.push(sql);
}

/// Run the client with its stdout captured and print the rows as markdown.
/// stdin and stderr stay the terminal's, so a password prompt and the client's
/// own errors reach the user as they would without `--md`, and the exit status
/// is still the client's.
fn print_markdown(
    program: &str,
    args: &[String],
    env: Vec<(String, String)>,
    secret: crate::engines::Secret,
    rows: crate::markdown::Rows,
) -> ! {
    use std::process::{Command, Stdio};
    let caught = catch_interrupts_for(&secret);
    let out = Command::new(program)
        .args(args)
        .envs(env)
        .envs(secret.env.iter().cloned())
        .stdin(Stdio::inherit())
        .stderr(Stdio::inherit())
        .output();
    drop(secret);
    drop(caught);
    match out {
        Ok(out) => {
            use std::io::Write as _;
            let _ = write!(
                std::io::stdout(),
                "{}",
                crate::markdown::render(&String::from_utf8_lossy(&out.stdout), rows)
            );
            std::process::exit(exit_code(out.status));
        }
        Err(e) => could_not_run(program, &e, ""),
    }
}

/// Ctrl-C is the child's while a passfile is out, or it would end esql before
/// the file is deleted. Keep what it returns until the child has exited.
fn catch_interrupts_for(secret: &crate::engines::Secret) -> Option<crate::tui::Swallowed> {
    secret.holds_file().then(crate::tui::swallow_interrupts)
}

/// The child's exit code, or the shell's `128 + signal` when a signal ended it.
fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    status.code().unwrap_or(1)
}

/// Run `program` as a child, where `exec` would leave nothing behind to delete
/// the passfile `secret` holds, and exit with its status once the file is gone.
fn run_then_forget(
    program: &str,
    args: &[String],
    env: Vec<(String, String)>,
    secret: crate::engines::Secret,
    install: &str,
) -> ! {
    let caught = catch_interrupts_for(&secret);
    let status = std::process::Command::new(program)
        .args(args)
        .envs(env)
        .envs(secret.env.iter().cloned())
        .status();
    drop(secret);
    drop(caught);
    match status {
        Ok(status) => std::process::exit(exit_code(status)),
        Err(e) => could_not_run(program, &e, install),
    }
}

/// Replace this process with `program`, so it owns the terminal and its exit
/// status is ours, reading `stdin` instead of ours when given. `install` is
/// what the failure says to do about it.
fn exec(
    program: &str,
    args: &[String],
    env: Vec<(String, String)>,
    stdin: Option<std::fs::File>,
    install: &str,
) -> ! {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).envs(env);
    if let Some(f) = stdin {
        cmd.stdin(f);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // exec never returns on success; the client takes over this PID and terminal.
        let err = cmd.exec();
        could_not_run(program, &err, install);
    }

    #[cfg(not(unix))]
    {
        match cmd.status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(e) => could_not_run(program, &e, install),
        }
    }
}

/// Why `program` could not be started, in words, and what to do about it. The
/// install line only when it is missing: a client that is there but cannot run
/// needs its mode or owner fixed, not a second copy.
fn could_not_run(program: &str, e: &std::io::Error, install: &str) -> ! {
    eprintln!(
        "{}",
        format!("esql: could not run {program}: {}", super::io_said(e)).red()
    );
    if e.kind() == std::io::ErrorKind::NotFound && !install.is_empty() {
        eprintln!("{}", format!("install it: {install}").dimmed());
    }
    std::process::exit(127);
}

/// A connection that only works through a forward brings the forward back with
/// it: the process died with the last reboot, but what it was is remembered.
/// Said out loud rather than done silently, because it starts a background ssh
/// that outlives this command.
fn reopen_tunnel(conn: &crate::engines::Conn) {
    match crate::vias::ensure(&conn.key()) {
        Some(Ok(msg)) => eprintln!("{}", format!("esql: {msg}").dimmed()),
        Some(Err(e)) => {
            eprintln!(
                "{}",
                format!("esql: could not reopen the tunnel: {}", super::said(&e)).red()
            );
            eprintln!(
                "{}",
                "      the connection points at it, so this will not connect.".dimmed()
            );
            // The command that gets past it, where there is one.
            let next = match e.downcast_ref::<crate::tunnels::Blocked>() {
                Some(crate::tunnels::Blocked::NoSsh) => {
                    crate::engines::install_argv_for(None, &[crate::engines::Tool::Ssh])
                        .map(|argv| format!("install it: {}", argv.join(" ")))
                }
                Some(crate::tunnels::Blocked::PortTaken { .. }) => Some(format!(
                    "move it: `esql tunnel up {} --local <port>`",
                    conn.key()
                )),
                Some(crate::tunnels::Blocked::UnknownHostKey { host, .. }) => Some(format!(
                    "`ssh {host} exit` checks its key once and accepts it"
                )),
                None => None,
            };
            if let Some(next) = next {
                eprintln!("{}", format!("      {next}").dimmed());
            }
            std::process::exit(1);
        }
        None => {}
    }
}

/// `esql --dump <name> [args]`: the engine's dump tool takes this process, so
/// stdout is the dump and the exit status is the tool's. Everything after the
/// name is the tool's own, a leading `--` dropped.
fn dump(conn: &crate::engines::Conn, db: Option<&str>, mut rest: Vec<String>) -> ! {
    if rest.first().is_some_and(|a| a == "--") {
        rest.remove(0);
    }
    let Some(argv) = conn.dump_argv(db, &rest) else {
        eprintln!(
            "{}",
            "esql: `--dump` cannot dump SQL Server, since sqlcmd ships no dump tool.".red()
        );
        eprintln!(
            "{}",
            "      run `BACKUP DATABASE` as a query instead, which writes the file on the server."
                .dimmed()
        );
        std::process::exit(2);
    };
    let (program, args) = argv.split_first().expect("dump_argv is never empty");
    let install = need_engine_tool(conn, program);
    reopen_tunnel(conn);
    crate::history::record(&conn.key());
    exec(program, args, Vec::new(), None, &install)
}

/// Exit 127 naming the install command when `program`, one of the engine's own
/// tools whichever client opens its sessions, is not on PATH; else that command,
/// for `exec` to name should starting it fail.
fn need_engine_tool(conn: &crate::engines::Conn, program: &str) -> String {
    let install = crate::engines::install_hint(conn.engine, &crate::settings::Settings::default());
    if !crate::engines::on_path(program) {
        eprintln!(
            "{}",
            format!("esql: `{program}` is not installed, and easysql is only a front end for it.")
                .red()
        );
        eprintln!("      {}  {}", "install it:".dimmed(), install.bold());
        std::process::exit(127);
    }
    install
}

/// `esql --restore <name> <file> [args]`: the engine's restore tool takes this
/// process and loads `file` into the connection, its output and exit status the
/// tool's own. Words after the file are the tool's, a leading `--` dropped.
fn restore(conn: &crate::engines::Conn, db: Option<&str>, mut rest: Vec<String>) -> ! {
    use crate::engines::Engine;
    if conn.engine == Engine::MsSql {
        eprintln!(
            "{}",
            "esql: `--restore` cannot restore SQL Server, since it has no dump file of its own."
                .red()
        );
        eprintln!(
            "{}",
            "      a script runs with `esql <name> -b -i app.sql` (-b exits 1 on an error), a `.bak` with `RESTORE DATABASE` on the server."
                .dimmed()
        );
        std::process::exit(2);
    }
    if rest.first().is_some_and(|a| a == "--") {
        rest.remove(0);
    }
    let usage = |headline: &str| -> ! {
        eprintln!("{}", format!("esql: {headline}").red());
        eprintln!(
            "{}",
            "      the file goes first, the tool's flags after it: `esql --restore <name> app.dump --clean`"
                .dimmed()
        );
        std::process::exit(2);
    };
    match rest.first() {
        None => usage("`--restore` needs the file to load."),
        Some(first) if first.starts_with('-') => usage(&format!(
            "`{first}` is a flag, where `--restore` wants the file."
        )),
        Some(_) => {}
    }
    let file = rest.remove(0);
    if rest.first().is_some_and(|a| a == "--") {
        rest.remove(0);
    }
    if conn.engine == Engine::Sqlite && !rest.is_empty() {
        eprintln!(
            "{}",
            "esql: a SQLite restore takes nothing after the file, since sqlite3 reads a bare word as the database to open."
                .red()
        );
        std::process::exit(2);
    }
    if conn.read_only() {
        eprintln!(
            "{}",
            format!(
                "esql: `{}` is read-only, so nothing can be restored into it.",
                conn.name
            )
            .red()
        );
        eprintln!(
            "{}",
            format!(
                "      turn it off for the load: `esql conn edit {} --read-only off`",
                conn.key()
            )
            .dimmed()
        );
        std::process::exit(2);
    }
    let unreadable = |e: std::io::Error| -> ! {
        eprintln!(
            "{}",
            format!("esql: could not read `{file}`: {}", super::io_said(&e)).red()
        );
        std::process::exit(2);
    };
    let archive = match conn.engine {
        Engine::Pg => crate::engines::pg::is_archive(std::path::Path::new(&file))
            .unwrap_or_else(|e| unreadable(e)),
        _ => false,
    };
    // A directory on stdin reads as an empty dump to sqlite3, which then exits 0.
    if conn.engine != Engine::Pg
        && std::fs::metadata(&file)
            .unwrap_or_else(|e| unreadable(e))
            .is_dir()
    {
        eprintln!(
            "{}",
            format!(
                "esql: `{file}` is a directory, and only a Postgres archive (`pg_dump -Fd`) is one."
            )
            .red()
        );
        std::process::exit(2);
    }
    let plan = conn
        .restore_argv(db, &file, archive, &rest)
        .expect("only SQL Server has no restore tool");
    let stdin = match plan.stdin {
        true => Some(std::fs::File::open(&file).unwrap_or_else(|e| unreadable(e))),
        false => None,
    };
    let (program, args) = plan
        .argv
        .split_first()
        .expect("restore_argv is never empty");
    let install = need_engine_tool(conn, program);
    reopen_tunnel(conn);
    crate::history::record(&conn.key());
    exec(program, args, Vec::new(), stdin, &install)
}

pub fn run(args: Vec<String>, md: bool, dump: bool, restore: bool) {
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
            "      save that file as its own connection: `esql conn add sqlite <name> --file <path>`"
                .dimmed()
        );
        std::process::exit(2);
    }

    let rest: Vec<String> = args.into_iter().skip(1).collect();
    if dump {
        self::dump(&conn, db.as_deref(), rest);
    }
    if restore {
        self::restore(&conn, db.as_deref(), rest);
    }
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
                    "      `esql snippet ls` lists them, and `esql snippet add` saves one."
                        .dimmed()
                );
                std::process::exit(2);
            }
        },
        None => None,
    };
    let adhoc = !session && first != "--" && snippet.is_none() && !first.starts_with('-');

    let mut settings = crate::settings::load();
    crate::ini::set_backups(settings.backups);

    let table = match (md, conn.engine.table_output(&settings)) {
        (false, _) => None,
        (true, None) => {
            eprintln!(
                "{}",
                format!(
                    "esql: `--md` cannot read `{}`'s rows back, since it has no quoted output.",
                    conn.engine.default_client()
                )
                .red()
            );
            if conn.engine == crate::engines::Engine::MsSql {
                let key = match settings.usql_cli {
                    true => "usql_mssql",
                    false => "usql_cli",
                };
                eprintln!(
                    "{}",
                    format!(
                        "      `esql set {key} on` opens SQL Server in usql, whose CSV it can read."
                    )
                    .dimmed()
                );
            }
            std::process::exit(2);
        }
        (true, Some(_)) if session => {
            eprintln!(
                "{}",
                "esql: `--md` formats a query's rows, so give it one: `esql --md <name> 'select 1'`."
                    .red()
            );
            std::process::exit(2);
        }
        (true, Some(t)) => Some(t),
    };

    // Before the tunnel and the history stamp, so a run refused for want of a
    // client leaves nothing behind. A flag the user typed themselves is theirs,
    // so a passthrough is not a one-shot here, unless `--md` has to add its own.
    if db.is_some() || snippet.is_some() || adhoc || table.is_some() {
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

    if let Some(note) = crate::engines::mssql::usql_password_note(&conn, &settings, !session) {
        eprintln!("{}", format!("esql: {note}").dimmed());
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
    let install = crate::engines::install_hint_for(conn.engine, &settings, &program);
    if !crate::engines::on_path(&program) {
        eprintln!(
            "{}",
            format!("esql: `{program}` is not installed, and easysql is only a front end for it.")
                .red()
        );
        if conn.engine.uses_usql(&settings) {
            eprintln!(
                "{}",
                format!(
                    "      the `usql_cli` setting opens {} connections in it.",
                    conn.engine.label()
                )
                .dimmed()
            );
        }
        eprintln!("      {}  {}", "install it:".dimmed(), install.bold());
        std::process::exit(127);
    }

    reopen_tunnel(&conn);

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

    // The same argv the TUI and the wizard preview build, so both ways in
    // behave identically and the preview can never lie about what runs.
    let mut argv = conn.connect_argv_db(&settings, db.as_deref());
    if let Some((flags, _)) = table {
        argv.extend(flags.iter().map(|f| f.to_string()));
    }

    if let Some(sql) = snippet.clone() {
        push_query(&mut argv, &conn, &settings, sql);
        argv.extend(rest.into_iter().skip(1));
    } else if first == "--" {
        // The escape hatch: an argument the client wants bare, which the rule
        // below would otherwise read as SQL.
        argv.extend(rest.into_iter().skip(1));
    } else if adhoc {
        // `esql prod 'select 1'`, the shape `ssh host 'cmd'` taught everyone.
        push_query(&mut argv, &conn, &settings, first);
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

    let env = conn.connect_env(&settings);
    let mut secret = conn.secret_env(&settings).unwrap_or_else(|e| {
        eprintln!(
            "{}",
            format!(
                "esql: could not hand the kept password over: {}",
                super::said(&e)
            )
            .red()
        );
        std::process::exit(1);
    });
    let csets = conn
        .usql_session(&settings, db.as_deref(), session, &mut secret)
        .unwrap_or_else(|e| {
            eprintln!(
                "{}",
                format!(
                    "esql: could not prepare usql's connection: {}",
                    super::said(&e)
                )
                .red()
            );
            std::process::exit(1);
        });

    // Only when a session is actually being opened: a one-shot query prints its
    // own result and a hint above it would just be noise in a pipe.
    if settings.hints && session {
        eprintln!(
            "{}",
            format!(
                "  {}",
                crate::engines::hint_line(conn.engine, &settings, !csets.is_empty())
            )
            .dimmed()
        );
    }

    let (program, rest) = argv.split_first().expect("connect_argv is never empty");
    let program = program.clone();
    let mut rest: Vec<String> = rest.to_vec();
    rest.extend(csets);

    // Stamp the connect before handing the terminal over, once nothing is left
    // that could refuse it: `exec` never comes back, so there is no "after".
    crate::history::record(&conn.key());

    if let Some((_, rows)) = table {
        print_markdown(&program, &rest, env, secret, rows);
    }
    if secret.holds_file() {
        run_then_forget(&program, &rest, env, secret, &install);
    }
    let mut env = env;
    env.extend(secret.env);
    exec(&program, &rest, env, None, &install);
}
