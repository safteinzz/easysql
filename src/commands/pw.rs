//! `esql pw`: save or forget a connection's password in the file its client
//! reads, through the same calls as the password form. The secret comes from
//! stdin or a prompt that does not echo, and never reaches an argv, the
//! environment, a step line or an error.

use super::{DryRun, Yes, can_ask, confirm_delete, fail, find_conn, said, shown, step};
use crate::creds::{self, Source};
use crate::engines::{self, Conn, Engine};
use std::io::{IsTerminal, Read};

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// Save the password a connection logs in with
    Set(SetArgs),
    /// Forget the password a connection logs in with
    Rm(RmArgs),
}

#[derive(clap::Args)]
pub struct SetArgs {
    /// The connection, as `esql ls` prints it, or `<engine>:<name>`
    #[arg(value_name = "NAME",
          add = clap_complete::ArgValueCompleter::new(crate::completion::conn_names))]
    name: String,
    /// Read the password from stdin (one trailing newline is dropped), for a script
    #[arg(long)]
    stdin: bool,
    /// Postgres only: the database the `.pgpass` line is for (default `*`, every database as that user)
    #[arg(long, value_name = "NAME")]
    db: Option<String>,
    #[command(flatten)]
    dry: DryRun,
}

#[derive(clap::Args)]
pub struct RmArgs {
    /// The connection, as `esql ls` prints it, or `<engine>:<name>`
    #[arg(value_name = "NAME",
          add = clap_complete::ArgValueCompleter::new(crate::completion::conn_names))]
    name: String,
    /// Postgres only: forget the line for this database rather than the one libpq would use
    #[arg(long, value_name = "NAME")]
    db: Option<String>,
    #[command(flatten)]
    yes: Yes,
    #[command(flatten)]
    dry: DryRun,
}

pub fn run(cmd: Cmd) {
    match cmd {
        Cmd::Set(args) => set(args),
        Cmd::Rm(args) => rm(args),
    }
}

/// The four fields a `.pgpass` line matches on, as the password form fills
/// them: the connection's own, with the database widened to `*` by default
/// because a role and its password are the same on every database.
fn pgpass_fields(c: &Conn, db: Option<&str>) -> [String; 4] {
    let host = if c.host.is_empty() {
        "localhost".to_string()
    } else {
        c.host.clone()
    };
    [
        host,
        c.port_or_default(),
        db.unwrap_or("*").to_string(),
        match c.user.is_empty() {
            true => "*".to_string(),
            false => c.user.clone(),
        },
    ]
}

fn set(args: SetArgs) {
    let c = find_conn(&args.name);
    if args.db.is_some() && c.engine != Engine::Pg {
        fail(&format!(
            "`--db` is for postgres, whose `.pgpass` matches on the database; {} keeps one password per connection",
            c.engine.label()
        ));
    }
    let settings = crate::settings::load();
    let target = match c.engine {
        Engine::Sqlite => fail(&format!(
            "'{}' is a sqlite file, which has no password",
            c.name
        )),
        Engine::MsSql if !settings.mssql_passwords => fail(
            "easysql keeps SQL Server passwords only once you allow it, because sqlcmd has no file of its own: `esql set mssql_passwords on`, then run this again",
        ),
        Engine::Pg => {
            let f = pgpass_fields(&c, args.db.as_deref());
            format!("{} as {}", shown(&creds::pgpass_path()), f.join(":"))
        }
        Engine::MySql => format!(
            "{} [{}]",
            shown(&engines::mysql::cnf_path()),
            engines::mysql::group_of(&c.name)
        ),
        Engine::MsSql => format!("{} [{}]", shown(&engines::mssql::pass_path()), c.name),
    };
    // Read before the step, so a run with no password prints nothing it did not do.
    let password = (!args.dry.dry_run).then(|| read_password(&c, args.stdin));
    step(&format!(
        "save the password (set) for {} in {target}",
        c.key()
    ));
    let Some(password) = password else {
        return;
    };
    let saved = match c.engine {
        Engine::Pg => {
            let [host, port, db, user] = pgpass_fields(&c, args.db.as_deref());
            creds::set_pg(&host, &port, &db, &user, &password)
        }
        Engine::MySql => engines::mysql::set_password(&c.name, Some(&password)),
        Engine::MsSql => engines::mssql::set_password(&c.name, Some(&password)),
        Engine::Sqlite => Ok(()),
    };
    if let Err(e) = saved {
        fail(&format!("could not save the password: {}", said(&e)));
    }
}

/// The password, from stdin with `--stdin` or from a prompt that does not echo
/// on a terminal, and otherwise a refusal: nothing here may block on a prompt
/// nobody can answer.
fn read_password(c: &Conn, from_stdin: bool) -> String {
    let password = if from_stdin {
        let mut raw = String::new();
        if std::io::stdin().read_to_string(&mut raw).is_err() {
            fail("could not read the password from stdin: it has to be text");
        }
        let trimmed = raw
            .strip_suffix("\r\n")
            .or_else(|| raw.strip_suffix('\n'))
            .unwrap_or(&raw);
        trimmed.to_string()
    } else if std::io::stdin().is_terminal() {
        match super::read_secret(&format!("password for {}: ", c.key())) {
            Some(p) => p,
            None => fail("cancelled, nothing was saved"),
        }
    } else {
        fail("no password given: pipe it in with `--stdin` (`esql pw set <name> --stdin < file`)");
    };
    if password.is_empty() {
        fail("the password is empty, so nothing was saved");
    }
    if password.contains(['\n', '\r']) {
        fail("a password cannot hold a line break, since each of these files keeps one per line");
    }
    password
}

fn rm(args: RmArgs) {
    let c = find_conn(&args.name);
    if args.db.is_some() && c.engine != Engine::Pg {
        fail(&format!(
            "`--db` is for postgres, whose `.pgpass` matches on the database; {} keeps one password per connection",
            c.engine.label()
        ));
    }
    if c.engine == Engine::Sqlite {
        fail(&format!(
            "'{}' is a sqlite file, which has no password",
            c.name
        ));
    }
    // The one libpq would use: the first line that covers the connection, or
    // with `--db` the first that covers it on that database.
    let mut asked = c.clone();
    if let Some(db) = &args.db {
        asked.database = db.clone();
    }
    let Some(cred) = creds::list().into_iter().find(|cr| cr.covers(&asked)) else {
        fail(&format!("{} has no saved password", c.key()));
    };
    let what = match &cred.source {
        Source::Pgpass(_) => format!(
            "the password for {} ({}:{}:{}:{})",
            c.key(),
            cred.host,
            cred.port,
            cred.database,
            cred.user
        ),
        _ => format!("the password for {}", c.key()),
    };
    can_ask(&what, args.yes.yes, args.dry.dry_run);
    step(&format!("forget {what} from {}", cred.where_stored()));
    if args.dry.dry_run {
        return;
    }
    confirm_delete(&what, None, args.yes.yes);
    if let Err(e) = creds::delete(&cred) {
        fail(&format!("could not forget it: {}", said(&e)));
    }
}
