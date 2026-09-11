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

## Browse and connect

![Moving down the connection list to a postgres connection whose tunnel is not running, pressing Enter to reopen it into a real psql session, then turning that forward off on the Tunnels tab](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/browse.gif)

Legend: green answered, red did not, yellow means the tunnel it needs is not
running; `pw` is a password on file, `no client` the program that opens it is
missing, and a blue name refuses writes.

```bash
esql                        # the toolbox
esql warehouse              # straight into one, tunnel and all
```

Every connection you have saved, from every engine, in one list, with where it
points and whether it answers. A yellow dot is not a dead server: whatever
answers on that port cannot be the database, so it is worth knowing before you
go in.

A forward dies with a reboot; the connection that needs it does not. easysql
remembers which one, and Enter reopens it and connects in one step. The Tunnels
tab lists every forward it knows about, the running ones and the remembered
ones, and `↵` turns one off or back on.

## Edit a connection

![Editing a postgres connection: the preview follows the database as it is typed, then Read only is switched on and the saved connection's name turns blue](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/edit.gif)

```bash
esql analytics 'select count(*) from orders'   # rows, as usual
esql analytics 'truncate orders'               # ERROR: cannot execute TRUNCATE TABLE in a read-only transaction
esql ls -v                                     # marks it (read-only)
```

`e` opens the form with the command it builds and what that command resolves
to, updated as you type. Keys you added by hand are carried through untouched,
and the file is backed up before every write.

**Read only** makes every session on it refuse writes: a Postgres connection
gets `default_transaction_read_only` in its service block, so pgAdmin and every
driver using that service get it too, and a SQLite one opens its file with
`-readonly`. Before going in on a Postgres connection, easysql asks the server
whether writes really are refused, and stops if they are not - a connection
pooler such as pgbouncer can drop the setting on the way.

It is a guardrail, not a permission: a session can switch it back off, so a
production database also wants a login role granted nothing but `SELECT`.

## Passwords, where the client looks

![Saving a password for the one connection without one, typed as dots, then the Passwords tab listing it among the others, never shown](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/passwords.gif)

Typed once into a hidden field, written straight to the file that engine's
client reads, and never read back, never echoed, never put in an argv or an
environment variable. easysql only ever shows you that one exists.

SQL Server's `sqlcmd` has no such file, so it asks every time - and a script or
an agent cannot answer. Turn on **sqlcmd passwords** in Settings (or accept
the offer `p` makes) and easysql keeps one in `~/.esqlpass`
(chmod 600), handed over in `SQLCMDPASSWORD`, never on the command line.

## Saved queries

![Making a saved query in the Snippets tab as one line, then opening its file in vim with o and pasting a longer version, which the details panel then shows](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/snippets.gif)

```bash
esql app :tables       # → psql … -c
esql notes :tables     # → sqlite3 … (bare argument)
```

![Running esql app :tables from the shell, printing the table list from the connection](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/snippet-run.png)

The check you keep rewriting, kept as a `.sql` file and run against any
connection. easysql hands each client the flag it wants, so the same word works
on every engine, and `o` opens the file in your `$EDITOR` for a longer one.

For Postgres they also become `\set` shortcuts in `~/.psqlrc`, so `:tables`
expands at the psql prompt too. Your own lines in that file are left alone.

## Settings

![The Settings tab showing ten settings grouped into behaviour and defaults, with the details panel explaining the selected one and naming the key it writes](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/settings.png)

Which program opens each engine, whether ports are checked, what order the list
is in. `d` puts any of them back.

## Commands

The handful of things faster to type than to click. Everything else is in the
toolbox, where `?` lists every key.

```bash
esql                        # the toolbox
esql prod                   # open a saved connection
esql prod 'select 1'        # run one query and exit, like `ssh host 'cmd'`
esql prod/reporting         # the same connection, another database
esql prod -c 'select 1'     # anything starting with - goes to the client
esql ls                     # list them, one name per line
esql ls -v                  # ...and where each one points
```

`esql --help` and `esql <command> --help` have the rest.

![esql ls -v printing seven connections with their engine, name and user@host:port/database, one marked read-only](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/ls.png)

## Keys

| key | does |
| --- | --- |
| `↑` `↓` / `k` `j` | move in the list |
| `←` `→` / `h` `l` / `Tab` | switch tab |
| `/` | filter the list; `Enter` keeps it, `Esc` drops it |
| `?` | every key this tab answers to |
| `q` / `Ctrl-C` | quit |

Each tab's own keys are on its bottom line, and `?` lists them all.

## What it edits

| tab | what it is | the file it edits |
| --- | --- | --- |
| Connections | every saved connection, from every engine | `~/.pg_service.conf`, `~/.my.cnf`, `~/.config/easysql/{sqlite,mssql}.conf` |
| Passwords | what unlocks them, written and never read back | `~/.pgpass`, `~/.my.cnf`, and `~/.esqlpass` once allowed (all chmod 600) |
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
- From a script: rows go to stdout and easysql's own words to stderr. Once the
  client starts, the exit code is the client's; before that easysql exits 2 for
  a name, snippet or `/db` it cannot use, 127 when there is no client to run,
  and 1 when a tunnel it needs will not open or a read-only connection would
  not be read-only.

## Compatibility

Four engines, each handed over to the client you already have: Postgres to
`psql`, MySQL and MariaDB to `mysql`, SQLite to `sqlite3`, SQL Server to
`sqlcmd`. Any of them can be pointed somewhere else in Settings.

SQL Server is the odd one out twice: no file both `sqlcmd` builds read, so
easysql keeps that list itself and passes `-S host,port -d db -U user`, and by
default it saves no password, because `sqlcmd` asking you is Microsoft's own
advice over `-P`; Settings can let easysql keep one instead. It is in no
distro's repos either, so there is no install to offer.

Linux. Tunnel liveness is read from `/proc` and killing a forward shells out to
`kill`, so macOS and BSD need a different implementation first.

## License

AGPL-3.0-only
