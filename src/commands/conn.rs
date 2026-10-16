//! `esql conn`: the connection form without the terminal. Every write goes
//! through the functions the form uses (`engines::save`, the extras it owns,
//! `carry_rename`, `delete_with_keyed`), so a block written here is the block
//! the TUI would have written, in the file that engine's client reads.

use super::{
    COMMANDS, DryRun, Yes, can_ask, confirm_delete, fail, find_conn, said, shown, step, warn,
};
use crate::engines::{self, Conn, Engine, NewConn};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Save a new connection
    Add(AddArgs),
    /// Change only the settings given, and an empty value removes that line
    Edit(EditArgs),
    /// Delete a connection, with its tunnel record and any kept SQL Server password
    Rm(RmArgs),
}

/// One flag per field of the connection form.
#[derive(clap::Args)]
pub struct Fields {
    /// Server address (default: localhost)
    #[arg(long)]
    host: Option<String>,
    /// Port, when it is not the engine's own (5432, 3306, 1433)
    #[arg(long)]
    port: Option<String>,
    /// Database to open (default: whatever the server gives that user)
    #[arg(long, value_name = "NAME")]
    db: Option<String>,
    /// User to log in as
    #[arg(long)]
    user: Option<String>,
    /// SQLite only: the database file
    #[arg(long, value_name = "PATH")]
    file: Option<String>,
    /// Postgres only: libpq's sslmode, or unset to leave it to libpq
    #[arg(long, value_name = "MODE", value_parser = SSL_CHOICES)]
    ssl: Option<String>,
    /// Postgres and SQLite only: make every session refuse writes (on|off, alone means on)
    #[arg(long, value_name = "on|off", num_args = 0..=1, default_missing_value = "on",
          value_parser = ["on", "off"])]
    read_only: Option<String>,
    /// SQL Server only: trust a self-signed certificate, sqlcmd's -C (on|off, alone means on)
    #[arg(long, value_name = "on|off", num_args = 0..=1, default_missing_value = "on",
          value_parser = ["on", "off"])]
    trust_cert: Option<String>,
}

const SSL_CHOICES: [&str; 6] = [
    "prefer",
    "require",
    "verify-ca",
    "verify-full",
    "disable",
    "unset",
];

#[derive(clap::Args)]
pub struct AddArgs {
    /// Which client opens it
    #[arg(value_name = "ENGINE", value_parser = ENGINE_CHOICES)]
    engine: String,
    /// The name you type after `esql`
    #[arg(value_name = "NAME")]
    name: String,
    #[command(flatten)]
    fields: Fields,
    #[command(flatten)]
    dry: DryRun,
}

const ENGINE_CHOICES: [&str; 8] = [
    "pg",
    "mysql",
    "sqlite",
    "sqlserver",
    "postgres",
    "my",
    "lite",
    "ms",
];

#[derive(clap::Args)]
pub struct EditArgs {
    /// The connection to change, as `esql ls` prints it, or `<engine>:<name>`
    #[arg(value_name = "NAME",
          add = clap_complete::ArgValueCompleter::new(crate::completion::conn_names))]
    name: String,
    /// Give it a new name, moving its tunnel record, history and kept password with it
    #[arg(long, value_name = "NAME")]
    rename: Option<String>,
    #[command(flatten)]
    fields: Fields,
    #[command(flatten)]
    dry: DryRun,
}

#[derive(clap::Args)]
pub struct RmArgs {
    /// The connection to delete, as `esql ls` prints it, or `<engine>:<name>`
    #[arg(value_name = "NAME",
          add = clap_complete::ArgValueCompleter::new(crate::completion::conn_names))]
    name: String,
    #[command(flatten)]
    yes: Yes,
    #[command(flatten)]
    dry: DryRun,
}

pub fn run(cmd: Cmd) {
    match cmd {
        Cmd::Add(args) => add(args),
        Cmd::Edit(args) => edit(args),
        Cmd::Rm(args) => rm(args),
    }
}

fn add(args: AddArgs) {
    let engine = Engine::from_slug(&args.engine)
        .unwrap_or_else(|| fail(&format!("`{}` is not an engine", args.engine)));
    let name = args.name.trim().to_string();
    check_name(engine, &name);
    if engines::list()
        .iter()
        .any(|c| c.engine == engine && c.name == name)
    {
        fail(&format!(
            "there is already a {} connection called '{name}': `esql conn edit {}:{name}` changes it",
            engine.label(),
            engine.slug()
        ));
    }
    let f = args.fields;
    check_fields(engine, &f);
    let mut extra = Vec::new();
    apply_choices(engine, &f, None, &mut extra);
    let nc = match engine {
        Engine::Sqlite => {
            let file = engines::sqlite::absolute(&f.file.unwrap_or_default());
            if file.is_empty() {
                fail("a sqlite connection needs its file: `--file <PATH>`");
            }
            NewConn {
                engine,
                name,
                host: String::new(),
                port: String::new(),
                database: file.trim().to_string(),
                user: String::new(),
                extra,
            }
        }
        _ => NewConn {
            engine,
            name,
            host: f.host.unwrap_or_default(),
            port: f.port.unwrap_or_default(),
            database: f.db.unwrap_or_default(),
            user: f.user.unwrap_or_default(),
            extra,
        },
    };
    step(&format!(
        "add {} connection '{}' to {}",
        engine.label(),
        nc.name,
        shown(&engine.store())
    ));
    lines(&nc);
    if args.dry.dry_run {
        return;
    }
    if let Err(e) = engines::save(None, &nc) {
        fail(&said(&e));
    }
}

fn edit(args: EditArgs) {
    let c = find_conn(&args.name);
    let engine = c.engine;
    let f = args.fields;
    check_fields(engine, &f);
    let name = match &args.rename {
        Some(n) => n.trim().to_string(),
        None => c.name.clone(),
    };
    if name != c.name {
        check_name(engine, &name);
        if engines::list()
            .iter()
            .any(|o| o.engine == engine && o.name == name)
        {
            fail(&format!(
                "there is already a {} connection called '{name}'",
                engine.label()
            ));
        }
    }
    // What the edit form opens on, with the flags given laid over it.
    let keep = |given: Option<String>, now: &str| given.unwrap_or_else(|| now.to_string());
    let mut extra = c.extra.clone();
    apply_choices(engine, &f, Some(&c), &mut extra);
    let nc = match engine {
        Engine::Sqlite => {
            let file = keep(f.file.map(|p| engines::sqlite::absolute(&p)), &c.database);
            if file.trim().is_empty() {
                fail("a sqlite connection needs its file, so `--file` cannot be empty");
            }
            NewConn {
                engine,
                name: name.clone(),
                host: String::new(),
                port: String::new(),
                database: file.trim().to_string(),
                user: String::new(),
                extra,
            }
        }
        _ => NewConn {
            engine,
            name: name.clone(),
            host: keep(f.host, &c.host),
            port: keep(f.port, &c.port),
            database: keep(f.db, &c.database),
            user: keep(f.user, &c.user),
            extra,
        },
    };
    step(&format!(
        "edit {} connection '{}' in {}",
        engine.label(),
        c.name,
        shown(&engine.store())
    ));
    lines(&nc);
    if name != c.name {
        step(&format!("rename it to '{name}'"));
        if crate::vias::get(&c.key()).is_some() {
            step(&format!(
                "move its tunnel record in {}",
                shown(&crate::vias::store_path())
            ));
        }
        if crate::history::load().get(&c.key()).is_some() {
            step(&format!(
                "move its place in the recent list in {}",
                shown(&crate::history::path())
            ));
        }
        if engine == Engine::MsSql && engines::mssql::has_password(&c.name) {
            step(&format!(
                "move its password in {}",
                shown(&engines::mssql::pass_path())
            ));
        }
    }
    if args.dry.dry_run {
        return;
    }
    if let Err(e) = engines::save(Some(&c.name), &nc) {
        fail(&said(&e));
    }
    if name != c.name
        && let Err(e) = engines::carry_rename(engine, &c.name, &name)
    {
        fail(&format!("saved, but {}", said(&e)));
    }
}

fn rm(args: RmArgs) {
    let c = find_conn(&args.name);
    let what = format!("{} connection '{}'", c.engine.label(), c.name);
    can_ask(&what, args.yes.yes, args.dry.dry_run);
    step(&format!("delete {what} from {}", shown(&c.engine.store())));
    if crate::vias::get(&c.key()).is_some() {
        step(&format!(
            "forget its tunnel in {}",
            shown(&crate::vias::store_path())
        ));
    }
    if c.engine == Engine::MsSql && engines::mssql::has_password(&c.name) {
        step(&format!(
            "forget its password in {}",
            shown(&engines::mssql::pass_path())
        ));
    }
    if args.dry.dry_run {
        return;
    }
    confirm_delete(&what, None, args.yes.yes);
    if let Err(e) = engines::delete_with_keyed(&c) {
        fail(&said(&e));
    }
}

/// Refuse a name the CLI could not hand back, and warn about one it can only
/// reach through its engine prefix.
fn check_name(engine: Engine, name: &str) {
    if name.is_empty() {
        fail("a connection needs a name");
    }
    if name.starts_with(['-', ':']) || name.contains(['[', ']', '\n']) {
        fail(&format!(
            "'{name}' cannot be a connection name: it may not start with `-` or `:`, or hold `[` or `]`"
        ));
    }
    if COMMANDS.contains(&name) {
        warn(&format!(
            "`esql {name}` is a command, so this connection opens as `esql {}:{name}`",
            engine.slug()
        ));
    }
}

/// Refuse a flag the engine has no field for, rather than writing a key its
/// client would ignore or, for libpq, refuse the whole file over.
fn check_fields(engine: Engine, f: &Fields) {
    let only = |given: bool, flag: &str, engines: &str| {
        if given {
            fail(&format!(
                "`--{flag}` is for {engines} connections, not {}",
                engine.label()
            ));
        }
    };
    let server = engine.networked();
    only(
        !server && (f.host.is_some() || f.port.is_some() || f.db.is_some() || f.user.is_some()),
        "host`, `--port`, `--db` or `--user",
        "server",
    );
    only(server && f.file.is_some(), "file", "sqlite");
    only(engine != Engine::Pg && f.ssl.is_some(), "ssl", "postgres");
    only(
        !engine.offers_read_only() && f.read_only.is_some(),
        "read-only",
        "postgres and sqlite",
    );
    only(
        engine != Engine::MsSql && f.trust_cert.is_some(),
        "trust-cert",
        "sqlserver",
    );
}

/// The extras the form owns, from the flags given. An edit changes only what a
/// flag names, and read-only only when the answer differs from the file's.
fn apply_choices(
    engine: Engine,
    f: &Fields,
    was: Option<&Conn>,
    extra: &mut Vec<(String, String)>,
) {
    if let Some(mode) = &f.ssl {
        engines::set_sslmode(extra, (mode != "unset").then_some(mode.as_str()));
    }
    if let Some(on) = &f.trust_cert {
        engines::set_trust_cert(extra, on == "on");
    }
    if let Some(on) = &f.read_only {
        let on = on == "on";
        if on != was.is_some_and(Conn::read_only) {
            engines::set_read_only(engine, extra, on);
        }
    }
}

/// The keys the block will hold, under its step. A password is never one of
/// them: MySQL's is carried through unseen, and no other block holds one.
fn lines(nc: &NewConn) {
    let fields: [(&str, &str); 4] = match nc.engine {
        Engine::Sqlite => [("path", &nc.database), ("", ""), ("", ""), ("", "")],
        Engine::Pg => [
            ("host", &nc.host),
            ("port", &nc.port),
            ("dbname", &nc.database),
            ("user", &nc.user),
        ],
        _ => [
            ("host", &nc.host),
            ("port", &nc.port),
            ("database", &nc.database),
            ("user", &nc.user),
        ],
    };
    for (k, v) in fields {
        if !k.is_empty() && !v.trim().is_empty() {
            step(&format!("  {k}={}", v.trim()));
        }
    }
    for (k, v) in &nc.extra {
        if !k.eq_ignore_ascii_case("password") {
            step(&format!("  {k}={v}"));
        }
    }
}
