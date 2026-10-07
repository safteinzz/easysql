# easysql (`esql`)

> **Canonical:** [gitlab.com/safteinzz/easysql](https://gitlab.com/safteinzz/easysql) · **Mirror:** [github.com/safteinzz/easysql](https://github.com/safteinzz/easysql)

<!-- desc:start -->
getting to the sql prompt, quick and easy - saved connections, their passwords and their tunnels in one CLI + TUI
<!-- desc:end -->

## Install

```bash
cargo install easysql
esql self check   # is a newer release out?
esql self update  # install the latest
```

No cargo yet? Rust installs the same way on every distro: [rustup.rs](https://rustup.rs).

## Browse and connect

![Moving down the connection list to a postgres connection whose tunnel is not running, pressing Enter to reopen it into a real psql session, then turning that forward off on the Tunnels tab](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/browse.gif)

Legend: green answered · red did not · yellow its tunnel is not running · `pw` a password on file · `no client` its program is missing · blue name refuses writes

```bash
esql                        # the toolbox
esql warehouse              # straight into one, tunnel and all
```

A forward dies with a reboot, but the connection that needs it remembers it, so Enter reopens the tunnel and connects in one step.

## Edit a connection

![Editing a postgres connection: the preview follows the database as it is typed, then Read only is switched on and the saved connection's name turns blue](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/edit.gif)

```bash
esql analytics 'select count(*) from orders'   # rows, as usual
esql analytics 'truncate orders'               # ERROR: cannot execute TRUNCATE TABLE in a read-only transaction
esql ls -v                                     # marks it (read-only)
```

![esql ls -v printing seven connections with their engine, name and user@host:port/database, one marked read-only](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/ls.png)

**Read only** makes every session refuse writes, and easysql asks the server whether it really does before going in. It is a guardrail, not a permission, so a production database also wants a role granted nothing but `SELECT`.

## Passwords, where the client looks

![Saving a password for the one connection without one, typed as dots, then the Passwords tab listing it among the others, never shown](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/passwords.gif)

Typed once, written straight to the file that engine's client reads, and never read back, echoed, or put in an argv or the environment.

## Saved queries

![Making a saved query in the Snippets tab as one line, then opening its file in vim with o and pasting a longer version, which the details panel then shows](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/snippets.gif)

```bash
esql app :tables       # → psql … -c
esql notes :tables     # → sqlite3 … (bare argument)
```

![Running esql app :tables from the shell, printing the table list from the connection](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/snippet-run.png)

The check you keep rewriting, kept as a `.sql` file: the same `:name` runs on every engine, and on Postgres it also expands at the psql prompt.

## Your defaults, not mine

![The Settings tab showing eleven settings grouped into behaviour and defaults, with the details panel explaining the selected one and naming the key it writes](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/settings.png)

Which program opens each engine, whether ports are checked, what order the list is in. `d` puts any of them back.

## Commands

```bash
esql prod 'select 1'        # run one query and exit, like `ssh host 'cmd'`
esql prod/reporting         # the same connection, another database
esql prod -c 'select 1'     # anything starting with - goes to the client
esql --md prod -f q.sql     # the rows as markdown tables, ready to paste
esql ls                     # list them, one name per line
```

`esql <command> --help` has the details, and `?` in the toolbox lists every key.

Rows go to stdout and easysql's own words to stderr. Once the client starts the exit code is the client's, and `esql --help` lists the codes easysql exits with before that.

## Where it keeps things

```
~/.pg_service.conf                   Postgres connections
~/.pgpass                            Postgres passwords
~/.psqlrc                            your saved queries as \set shortcuts
~/.my.cnf                            MySQL and MariaDB connections and passwords
~/.esqlpass                          sqlcmd passwords, once you allow it
~/.config/easysql/sqlite.conf        SQLite connections
~/.config/easysql/mssql.conf         SQL Server connections
~/.config/easysql/snippets/          your saved queries, a .sql file each
~/.config/easysql/vias               the forward each connection needs
~/.config/easysql/settings           your settings
~/.local/state/easysql/              running tunnels, history and backups
```

Password files are chmod 600, and every write rewrites only its own section, so your comments and extra keys survive.

## When it fails

A connect that fails is asked again without a prompt, and easysql offers the step that would get you in:

```
no password supplied             save one in ~/.pgpass
Connection refused               open an ssh tunnel
no pg_hba.conf entry for host    open an ssh tunnel
database "x" does not exist      edit the connection
```

## Notes

- Nothing is sent anywhere: easysql speaks no wire protocol and holds no connection open, it writes the files your clients already read and runs them.
- The clients are not Rust, so cargo cannot bring them: easysql offers the install command on apt, pacman, dnf, zypper and apk, and names the program anywhere else.
- `sqlcmd` reads no password file, so it asks every time; turn on **sqlcmd passwords** in Settings and easysql hands it one in `SQLCMDPASSWORD`, never on the command line.
- **Backups** in Settings keeps the last 1, 3, 5 or 10 copies of each file from before it was changed.

## Compatibility

Four engines, each handed to the client you already have, and any of them can be pointed elsewhere in Settings:

```
Postgres           psql
MySQL, MariaDB     mysql
SQLite             sqlite3
SQL Server         sqlcmd
```

SQL Server is in no distro's repos, so there is no install to offer, and `sqlcmd` shares no file easysql could write, so easysql keeps that list itself.

Linux. Tunnel liveness is read from `/proc` and killing a forward shells out to
`kill`, so macOS and BSD need a different implementation first.

## License

AGPL-3.0-only
