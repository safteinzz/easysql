//! `esql snippet`: the Snippets tab without the terminal. A snippet is a
//! `.sql` file run as `esql <name> :<snippet>`, and every change here keeps the
//! easysql block in `~/.psqlrc` in step, as the tab does on every reload.

use super::{
    DryRun, Json, Yes, can_ask, confirm_delete, fail, json_array, json_str, said, shown, step,
};
use crate::snippets::{self, Snippet};
use std::io::{IsTerminal, Read};

/// Under `esql snippet ls --help`: what `--json` prints.
const LS_JSON: &str = "\
--json prints an array with one object per saved query, and these names are fixed:
  name  what follows the colon in `esql <connection> :<name>`
  sql   the query as saved
  path  its .sql file";

#[derive(clap::Subcommand)]
pub enum Cmd {
    /// List every saved query
    #[command(after_help = LS_JSON)]
    Ls(LsArgs),
    /// Save a query under a name
    Add(AddArgs),
    /// Delete a saved query's file, the only copy of it
    Rm(RmArgs),
}

#[derive(clap::Args)]
pub struct LsArgs {
    /// Also print each query's SQL under its name
    #[arg(short, long)]
    verbose: bool,
    #[command(flatten)]
    json: Json,
}

#[derive(clap::Args)]
pub struct AddArgs {
    /// What you type after the colon: `esql <connection> :<name>`
    #[arg(value_name = "NAME")]
    name: String,
    /// The SQL; read from stdin when it is not given
    #[arg(value_name = "SQL")]
    sql: Option<String>,
    /// Overwrite a saved query of the same name
    #[arg(long)]
    replace: bool,
    #[command(flatten)]
    dry: DryRun,
}

#[derive(clap::Args)]
pub struct RmArgs {
    /// The saved query to delete
    #[arg(value_name = "NAME",
          add = clap_complete::ArgValueCompleter::new(crate::completion::snippet_names))]
    name: String,
    #[command(flatten)]
    yes: Yes,
    #[command(flatten)]
    dry: DryRun,
}

pub fn run(cmd: Cmd) {
    match cmd {
        Cmd::Ls(args) => ls(args),
        Cmd::Add(args) => add(args),
        Cmd::Rm(args) => rm(args),
    }
}

fn ls(args: LsArgs) {
    let all = snippets::list();
    if args.json.json {
        let rows: Vec<String> = all
            .iter()
            .map(|s| {
                format!(
                    "{{\"name\": {}, \"sql\": {}, \"path\": {}}}",
                    json_str(&s.name),
                    json_str(s.sql.trim_end()),
                    json_str(&s.path.to_string_lossy())
                )
            })
            .collect();
        println!("{}", json_array(&rows));
        return;
    }
    if all.is_empty() {
        eprintln!("esql: no saved queries yet: `esql snippet add <name> 'select 1'` saves one");
        return;
    }
    for s in &all {
        println!("{}", s.name);
        if args.verbose {
            for line in s.sql.trim_end().lines() {
                println!("    {line}");
            }
        }
    }
}

fn add(args: AddArgs) {
    let name = args.name.trim().to_string();
    if !snippets::valid_name(&name) {
        fail("a saved query's name cannot be empty, hold a slash, or start with a dot");
    }
    let sql = match args.sql {
        Some(sql) => sql,
        None if std::io::stdin().is_terminal() => fail(&format!(
            "no SQL given: `esql snippet add {name} 'select 1'`, or pipe it in (`< query.sql`)"
        )),
        None => {
            let mut raw = String::new();
            if std::io::stdin().read_to_string(&mut raw).is_err() {
                fail("could not read the SQL from stdin: it has to be text");
            }
            raw
        }
    };
    if sql.trim().is_empty() {
        fail("the SQL is empty, so nothing was saved");
    }
    let exists = snippets::get(&name).is_some();
    if exists && !args.replace {
        fail(&format!(
            "a query called '{name}' is already saved: `--replace` overwrites it"
        ));
    }
    let path = snippets::dir().join(format!("{name}.sql"));
    let verb = if exists { "replace" } else { "save" };
    step(&format!("{verb} '{name}' in {}", shown(&path)));
    let mut after: Vec<Snippet> = snippets::list()
        .into_iter()
        .filter(|s| s.name != name)
        .collect();
    after.push(Snippet {
        name: name.clone(),
        sql: sql.clone(),
        path,
    });
    after.sort_by(|a, b| a.name.cmp(&b.name));
    psqlrc_step(&after);
    if args.dry.dry_run {
        return;
    }
    if let Err(e) = snippets::save(&name, &sql) {
        fail(&said(&e));
    }
    sync_psqlrc();
}

fn rm(args: RmArgs) {
    let Some(s) = snippets::get(&args.name) else {
        fail(&format!(
            "no saved query called '{}': `esql snippet ls` lists them",
            args.name
        ));
    };
    let what = format!("the saved query '{}'", s.name);
    can_ask(&what, args.yes.yes, args.dry.dry_run);
    step(&format!("delete '{}' at {}", s.name, shown(&s.path)));
    let after: Vec<Snippet> = snippets::list()
        .into_iter()
        .filter(|o| o.name != s.name)
        .collect();
    psqlrc_step(&after);
    if args.dry.dry_run {
        return;
    }
    // Typed, because the file is the only copy of that query.
    confirm_delete(&what, Some(&s.name), args.yes.yes);
    if let Err(e) = snippets::delete(&s.name) {
        fail(&said(&e));
    }
    sync_psqlrc();
}

/// The `~/.psqlrc` step, printed only when the block would really change, as
/// `sync_psqlrc` writes only then.
fn psqlrc_step(after: &[Snippet]) {
    let path = snippets::psqlrc_path();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if snippets::splice(&existing, &snippets::psqlrc_block(after)) != existing {
        step(&format!("update the easysql block in {}", shown(&path)));
    }
}

fn sync_psqlrc() {
    if let Err(e) = snippets::sync_psqlrc() {
        fail(&format!(
            "saved, but `:name` in psql was not updated: {}",
            said(&e)
        ));
    }
}
