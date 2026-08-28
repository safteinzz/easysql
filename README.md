# easysql

One command, `esql`, over `psql`, `mysql`, `sqlite3` and `sqlcmd`. Your saved
connections in a list, their passwords where their clients already look for
them, and the ssh tunnel you need to reach the ones that hide behind a bastion.

It is a front end, never a client. easysql writes the files your tools already
read and then hands the terminal over, so `\c`, `\dt`, your `.psqlrc` and every
script on the machine keep working, and uninstalling it costs you nothing.

## Install

```sh
cargo install easysql
```

The real clients have to be on the machine, and cargo cannot bring them: they
are not Rust. easysql works out the one command *your* machine needs and offers
to run it, so you never have to go and find out.

![The easysql tour: opening a saved postgres connection into a real psql session, coming back to the list, inspecting a connection that needs an ssh tunnel, editing one in the wizard, filtering, and the Passwords, Tunnels and Settings tabs](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/demo.gif)

## What you see first

Every connection you have saved, from every engine, in one list. Green answered,
red did not, yellow means the tunnel it needs is not running. `pw` is a password
already on file, and `no client` is the program that would open it missing.

![The Connections tab listing seven connections across postgres, mysql, sqlite and sqlserver, with a details panel showing host, port, database, user and the psql command that will run](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/connections.png)

## A connection that only works through a tunnel

A forward dies with a reboot; the connection that needs it does not. easysql
remembers which one, says so before you press anything, and Enter reopens it and
connects in one step.

![The details panel for a connection whose tunnel is closed, reading "tunnel to bastion is not open - Enter reopens it and connects", with the ssh -L command it will run](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/tunnel-aware.png)

## Editing writes the file the client already reads

The wizard shows the command it is building and what that command resolves to,
so nothing about it is a surprise. Keys you added by hand are carried through
untouched, and the file is backed up before every write.

![The edit wizard for a postgres connection, with name, host, port, database, user and sslmode fields, previewing both the psql command and the user@host:port/database it resolves to](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/wizard.png)

## Passwords go where the client looks for them

Typed once into a hidden field, written straight to the file that engine's
client reads, and never read back, never echoed, never put in an argv or an
environment variable. easysql only ever shows you that one exists.

![The Passwords tab listing five saved passwords by engine, host, database and user, each shown as dots, with a details panel noting the password itself is never read back](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/passwords.png)

## Tunnels are just `ssh -L`

Opened from the connection that needs one, with every field already filled in.
Each forward says which connection it serves, and `d` kills it.

![The Tunnels tab listing two ssh forwards by pid, one labelled "for metrics (postgres)", with a details panel showing the ssh -N -L command and the kill command](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/tunnels.png)

## Settings

Which program opens each engine, whether ports are checked, what order the list
is in. `d` puts any of them back.

![The Settings tab showing eight settings grouped into behaviour and defaults, with the details panel explaining the selected one and naming the key it writes](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/settings.png)

## The CLI

Three things, all faster to type than to click. Everything else is in the TUI.

```sh
esql                  # the toolbox
esql prod             # open a saved connection
esql prod -c 'select 1'   # anything after the name goes to the client
esql ls               # list them, one name per line
esql ls -v            # ...and where each one points
esql self update      # update this binary
```

![esql ls -v printing seven connections with their engine, name and user@host:port/database](https://gitlab.com/safteinzz/easysql/-/raw/main/readme-assets/ls.png)

## What it edits

| tab | what it is | the file it edits |
| --- | --- | --- |
| Connections | every saved connection, from every engine | `~/.pg_service.conf`, `~/.my.cnf`, `~/.config/easysql/{sqlite,mssql}.conf` |
| Passwords | what unlocks them, written and never read back | `~/.pgpass`, `~/.my.cnf` (both chmod 600) |
| Tunnels | the `ssh -L` between you and a database you cannot route to | tracked by pid, killable |
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

## Keys

| key | what it does |
| --- | --- |
| `↵` | open the connection, change the setting |
| `c` | new connection, password or tunnel |
| `e` | edit |
| `d` | delete, kill a tunnel, or put a setting back |
| `p` | save a password for this connection |
| `t` | reach it through an ssh host (`ssh -L`) |
| `y` / `Y` | yank the command / the URL |
| `Tab` | next tab |
| `/` | filter the list you are in |
| `r` | refresh |
| `?` | help |
| `q` | quit |

## Notes

- Postgres, MySQL/MariaDB, SQLite and SQL Server.
- SQL Server is the odd one: no file both `sqlcmd` builds read, so easysql keeps
  that list itself and passes `-S host,port -d db -U user`. It saves no password
  there either - `sqlcmd` asks you, which is Microsoft's own advice over `-P`.
  It is also in no distro's repos, so there is no install to offer.
- apt, pacman, dnf, zypper and apk are recognised for the install offer. On
  anything else easysql names the program rather than guessing a package.
- Tunnel liveness is read from `/proc`, so tunnels are Linux only.
- Nothing is sent anywhere. easysql speaks no wire protocol and holds no
  connection open; it writes config and runs your client.

Linux only. AGPL-3.0-only.
