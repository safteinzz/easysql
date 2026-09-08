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

The real clients have to be on the machine, and cargo cannot bring them: they
are not Rust. easysql works out the one command *your* machine needs and offers
to run it, so you never have to go and find out.

![The easysql tour: opening a saved postgres connection into a real psql session, coming back to the list, inspecting a connection that needs an ssh tunnel, editing one in the wizard, filtering, and the Passwords, Tunnels, Snippets and Settings tabs](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/demo.gif)

## What you see first

Every connection you have saved, from every engine, in one list. Green answered,
red did not, yellow means the tunnel it needs is not running - whatever answered
on that port cannot be the database, so it is worth knowing before you go in. `pw` is a password
already on file, and `no client` is the program that would open it missing.

![The Connections tab listing seven connections across postgres, mysql, sqlite and sqlserver, with a details panel showing host, port, database, user and the psql command that will run](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/connections.png)

## When it needs a tunnel

A forward dies with a reboot; the connection that needs it does not. easysql
remembers which one, says so before you press anything, and Enter reopens it and
connects in one step.

![The details panel for a connection whose tunnel is closed, reading "tunnel to bastion is not open - Enter reopens it and connects", with the ssh -L command it will run](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/tunnel-aware.png)

## The command, built for you

The wizard shows the command it is building and what that command resolves to,
so nothing about it is a surprise. Keys you added by hand are carried through
untouched, and the file is backed up before every write.

![The edit wizard for a postgres connection, with name, host, port, database, user and sslmode fields, previewing both the psql command and the user@host:port/database it resolves to](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/wizard.png)

## Passwords, where the client looks

Typed once into a hidden field, written straight to the file that engine's
client reads, and never read back, never echoed, never put in an argv or an
environment variable. easysql only ever shows you that one exists.

![The Passwords tab listing five saved passwords by engine, host, database and user, each shown as dots, with a details panel noting the password itself is never read back](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/passwords.png)

## Tunnels are just `ssh -L`

Opened from the connection that needs one, with every field already filled in.
Each forward says which connection it serves, and every one this tool knows about
is listed - the ones running and the ones a connection remembers - so `↵` turns
one back on and `d` kills it.

![The Tunnels tab listing three forwards, two on and one off, each labelled with the connection it serves, with a details panel showing the ssh -N -L command and the kill command](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/tunnels.png)

## Saved queries

The check you keep rewriting, kept as a `.sql` file and run against any
connection. easysql hands each client the flag it wants, so the same word works
on every engine.

```sh
esql app :tables       # → psql … -c
esql notes :tables     # → sqlite3 … (bare argument)
```

![The Snippets tab listing three saved queries with the :name that runs each, and a details panel showing the SQL, the file it lives in and the command that runs it](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/snippets.png)

For Postgres they also become `\set` shortcuts in `~/.psqlrc`, so `:tables`
expands at the psql prompt too. Your own lines in that file are left alone.

![Running esql app :tables from the shell, printing the table list from the connection](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/snippet-run.png)

## Settings

Which program opens each engine, whether ports are checked, what order the list
is in. `d` puts any of them back.

![The Settings tab showing eight settings grouped into behaviour and defaults, with the details panel explaining the selected one and naming the key it writes](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/settings.png)

## Commands

The handful of things faster to type than to click. Everything else is in the
toolbox, where `?` lists every key.

```sh
esql                        # the toolbox
esql prod                   # open a saved connection
esql prod 'select 1'        # run one query and exit, like `ssh host 'cmd'`
esql prod/reporting         # the same connection, another database
esql prod :tables           # run a saved query against it
esql ls                     # list them, one name per line
esql ls -v                  # ...and where each one points
```

Rows go to stdout and easysql's own words to stderr, and the exit code is the
client's, so a one-shot is safe to pipe. An argument starting with `-` goes
straight to the client (`esql prod -c 'select 1'`), as does everything after
`--`.

![esql ls -v printing seven connections with their engine, name and user@host:port/database](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/ls.png)

## What it edits

| tab | what it is | the file it edits |
| --- | --- | --- |
| Connections | every saved connection, from every engine | `~/.pg_service.conf`, `~/.my.cnf`, `~/.config/easysql/{sqlite,mssql}.conf` |
| Passwords | what unlocks them, written and never read back | `~/.pgpass`, `~/.my.cnf` (both chmod 600) |
| Tunnels | the `ssh -L` between you and a database you cannot route to | on and off, remembered per connection |
| Settings | the choices that are yours rather than a client's | `~/.config/easysql/settings` |

Every write backs the file up first and rewrites only the one section, so your
own comments, ordering and extra keys survive.

## When it fails

A connect that does not work is the interesting case, so easysql asks the server
the same question again non-interactively, reads the client's own answer, and
offers the step that would actually get you in:

- `no password supplied` → save one in `~/.pgpass`
- `Connection refused`, or `no pg_hba.conf entry for host` → open an ssh tunnel
- `database "x" does not exist` → edit the connection

## Notes

- Nothing is sent anywhere. A front end, not a client: easysql speaks no wire
  protocol and holds no connection open, it writes the files your tools already
  read and runs them, so uninstalling it costs you nothing.
- apt, pacman, dnf, zypper and apk are recognised for the install offer. On
  anything else easysql names the program rather than guessing a package.

## Compatibility

Four engines, each handed over to the client you already have: Postgres to
`psql`, MySQL and MariaDB to `mysql`, SQLite to `sqlite3`, SQL Server to
`sqlcmd`. Any of them can be pointed somewhere else in Settings.

SQL Server is the odd one out twice: no file both `sqlcmd` builds read, so
easysql keeps that list itself and passes `-S host,port -d db -U user`, and it
saves no password, because `sqlcmd` asking you is Microsoft's own advice over
`-P`. It is in no distro's repos either, so there is no install to offer.

Linux. Tunnel liveness is read from `/proc` and killing a forward shells out to
`kill`, so macOS and BSD need a different implementation first.

## License

AGPL-3.0-only
