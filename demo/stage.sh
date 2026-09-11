#!/usr/bin/env bash
# A staged HOME for the README screenshots and the demo GIF: fake connections,
# fake passwords, fake tunnels, and two throwaway databases that really answer.
# Nothing here touches your real ~/.pg_service.conf, ~/.pgpass, ~/.my.cnf or
# ~/.config/easysql - every path is redirected into ./home, XDG included.
#
#   ./stage.sh up     build the fixtures, start the servers and the listeners
#   ./stage.sh run    launch esql against them (this is what you screenshot)
#   ./stage.sh shell  a shell where `esql` is this build, for the CLI shots
#   ./stage.sh down   stop everything, then delete the stage
#
# The shell it opens wears the same invented `user@host` prompt as every other
# crate's rig, and every tape sets Catppuccin Mocha, so the frames across the
# projects are one terminal.
#
# Every address is loopback or from a range reserved for documentation
# (RFC 5737), every name is example.com (RFC 2606) and every password is
# `devpass`, typed into throwaway containers that are removed on `down`. There
# is nothing real in it to leak, and no `.env` to fill in: unlike an ssh tool,
# a database front end can stage its whole world locally.
#
# The one thing that cannot be staged is a client easysql does not have. sqlcmd
# is deliberately left missing, because it is missing on every distro - that
# frame is the truth, not a fixture.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
STAGE="$HERE/home"
# Everything the rig creates lives inside the stage, so `down` takes all of it
# with one guarded delete and nothing is left in demo/ to gitignore.
PIDS="$STAGE/.pids"
BIN="$STAGE/.bin"
ESQL="$HERE/../target/release/esql"

# Written by `up`, required by `down`. See the guard further down.
MARKER=".easysql-demo-stage"

# High ports so the rig can never collide with a real postgres or mysql on the
# machine rendering it, and so nothing here needs root.
PG_PORT=55432
MY_PORT=53306
# The near end of the tunnel that is up. A plain listener is enough: reach only
# ever asks "is anything accepting on that port", never "is that a database".
TUN_PORT=56543

CENGINE="$(command -v podman || command -v docker || true)"

# The complete environment anything staged runs in. Used with `env -i`, so this
# is not "the real environment plus overrides" - it is everything there is.
# HOME alone would not do: easysql keeps history and tunnel state under
# XDG_STATE_HOME and its settings under XDG_CONFIG_HOME, so a missed variable
# points a frame at the real config dir.
env_for_stage() {
  echo "HOME=$STAGE" \
       "XDG_CONFIG_HOME=$STAGE/.config" \
       "XDG_DATA_HOME=$STAGE/.local/share" \
       "XDG_STATE_HOME=$STAGE/.local/state" \
       "XDG_CACHE_HOME=$STAGE/.cache" \
       "PATH=$BIN:/usr/local/bin:/usr/bin:/bin" \
       "TERM=${TERM:-xterm-256color}" \
       "COLORTERM=truecolor" \
       "LANG=C.UTF-8" \
       "EDITOR=vim" \
       "PGCONNECT_TIMEOUT=5"
}

# ---------------------------------------------------------------------------
# fixtures
# ---------------------------------------------------------------------------

write_pg() {
  # The file libpq itself reads. `app` is the live container, `warehouse` is
  # behind a tunnel that is not running, `analytics` is a documentation name
  # that will never resolve - so the list shows up, sleeping and down at once
  # instead of a wall of one colour. `analytics` is also the read-only one, the
  # connection you would hand an agent.
  cat > "$STAGE/.pg_service.conf" <<EOF
# ~/.pg_service.conf - read by psql, pgAdmin, DBeaver and every libpq driver.
# easysql rewrites one [section] at a time and leaves the rest of the file alone.

[app]
host=127.0.0.1
port=$PG_PORT
dbname=app
user=dev

[warehouse]
host=127.0.0.1
port=$TUN_PORT
dbname=reporting
user=analyst

[metrics]
host=127.0.0.1
port=15432
dbname=metrics
user=grafana

[analytics]
host=db.example.com
port=5432
dbname=metrics
user=reader
sslmode=verify-full
options=-c default_transaction_read_only=on
EOF
}

write_pgpass() {
  # host:port:database:user:password, with * as a wildcard. libpq refuses to
  # read it at all unless it is 0600, which is why easysql chmods it. `metrics`
  # has none, so the Passwords GIF has one to save.
  cat > "$STAGE/.pgpass" <<EOF
127.0.0.1:$PG_PORT:app:dev:devpass
127.0.0.1:$TUN_PORT:reporting:analyst:devpass
db.example.com:5432:*:reader:devpass
EOF
  chmod 600 "$STAGE/.pgpass"
}

write_my() {
  # Only [clientNAME] groups are easysql's. Plain [client] is read by every
  # MySQL tool on the machine, so writing a host into it would silently
  # redirect mysqldump too - which is why the fixture shows one.
  cat > "$STAGE/.my.cnf" <<EOF
# ~/.my.cnf - read by mysql, mysqldump and every MySQL tool on the machine.

[client]
default-character-set=utf8mb4

[clientshop]
host=127.0.0.1
port=$MY_PORT
database=shop
user=root
password=devpass
EOF
  chmod 600 "$STAGE/.my.cnf"
}

write_easysql_conf() {
  local cfg="$STAGE/.config/easysql"
  mkdir -p "$cfg"

  # SQLite has no server and no password: a connection is a path.
  cat > "$cfg/sqlite.conf" <<'EOF'
[notes]
path=~/notes.db
EOF

  # SQL Server keeps its list here because no file both sqlcmd builds read
  # exists. It stores no password either - sqlcmd asks.
  cat > "$cfg/mssql.conf" <<'EOF'
[reporting]
host=mssql.example.com
port=1433
database=Reporting
user=sa
trust_cert=yes
EOF

  # The forward `warehouse` depends on. Recorded but not running, so the list
  # shows the tunnel state rather than blaming the database.
  cat > "$cfg/vias" <<EOF
# metrics has a forward that is running, warehouse has one that is not, so the
# list shows both halves of what a remembered tunnel looks like. No backticks in
# this heredoc: it interpolates \$TUN_PORT below, so bash would run them.
[pg:metrics]
host=dbhost
target=127.0.0.1
port=5432
local=15432

[pg:warehouse]
host=bastion
target=127.0.0.1
port=5432
local=$TUN_PORT
EOF
}

write_snippets() {
  # Saved queries, the thing `esql <connection> :name` runs. Deliberately a mix:
  # one that works anywhere, one that is Postgres-only, so the tab shows that
  # they are plain SQL files and not an engine-scoped abstraction.
  local d="$STAGE/.config/easysql/snippets"
  mkdir -p "$d"
  cat > "$d/tables.sql" <<'EOF'
select table_schema, table_name
  from information_schema.tables
 where table_schema not in ('pg_catalog', 'information_schema')
 order by 1, 2;
EOF
  cat > "$d/activity.sql" <<'EOF'
select pid, usename, state, left(query, 60) as query
  from pg_stat_activity
 where state is not null
 order by pid;
EOF
  cat > "$d/sizes.sql" <<'EOF'
select relname, pg_size_pretty(pg_total_relation_size(c.oid)) as size
  from pg_class c
 order by pg_total_relation_size(c.oid) desc
 limit 10;
EOF
}

write_ssh_shim() {
  # The stage's ssh. easysql opens a forward as `ssh -N -L <spec> <host>` and
  # trusts it only while /proc/<pid>/cmdline still carries that flag, spec and
  # host, so this keeps them as its own arguments and really forwards <local>
  # to the staged Postgres - which is what lets Enter on `warehouse` reopen its
  # tunnel and land in a real session. It exits once the stage is gone, so no
  # teardown can leave it listening.
  mkdir -p "$BIN"
  cat > "$BIN/ssh" <<EOF
#!/bin/sh
exec python3 -c '
import os, socket, sys, threading, time
local = int(sys.argv[sys.argv.index("-L") + 1].split(":")[0])
srv = socket.socket()
srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
srv.bind(("127.0.0.1", local))
srv.listen(16)
def pump(a, b):
    try:
        while True:
            data = a.recv(65536)
            if not data:
                break
            b.sendall(data)
    except OSError:
        pass
    for s in (a, b):
        try:
            s.close()
        except OSError:
            pass
def serve():
    while True:
        near, _ = srv.accept()
        far = socket.create_connection(("127.0.0.1", $PG_PORT))
        threading.Thread(target=pump, args=(near, far), daemon=True).start()
        threading.Thread(target=pump, args=(far, near), daemon=True).start()
threading.Thread(target=serve, daemon=True).start()
while os.path.exists("$STAGE/$MARKER"):
    time.sleep(1)
' "\$@"
EOF
  chmod +x "$BIN/ssh"
}

write_vim_shim() {
  # The editor `o` opens. `-u DEFAULTS` skips this machine's vimrc, system one
  # included, so the frame is the same on every machine that renders it; a word
  # in EDITOR itself would be split apart by `env -i $(env_for_stage)`.
  mkdir -p "$BIN"
  cat > "$BIN/vim" <<'EOF'
#!/bin/sh
PATH=/usr/local/bin:/usr/bin:/bin exec vim -u DEFAULTS "$@"
EOF
  chmod +x "$BIN/vim"
}

write_ssh_config() {
  # Read, never written. The tunnel wizard picks its hop from here, so the
  # picker has something to show. RFC 5737 documentation addresses.
  mkdir -p "$STAGE/.ssh"
  cat > "$STAGE/.ssh/config" <<'EOF'
Host bastion
    HostName 192.0.2.10
    User admin

Host dbhost
    HostName 192.0.2.20
    User postgres

Host reporting-vpn
    HostName 198.51.100.7
    User tunnel
EOF
  chmod 600 "$STAGE/.ssh/config"
}

make_sqlite_db() {
  # A real database, so Enter on it opens a real sqlite3 session with something
  # to select. Small enough to build every time.
  sqlite3 "$STAGE/notes.db" <<'SQL'
create table if not exists note (
  id      integer primary key,
  title   text not null,
  body    text not null,
  created text not null default (date('now'))
);
delete from note;
insert into note (title, body) values
  ('groceries',  'oat milk, tomatoes, bread'),
  ('reading',    'finish the postgres internals chapter'),
  ('esql todo',  'try the tunnel wizard against the pi');
SQL
}

seed_history() {
  # `<epoch> <count> <key>`, keyed by <slug>:<name>. Computed at run time so the
  # "3 days ago" in a frame is always three days ago, on any machine.
  local state="$STAGE/.local/state/easysql"
  mkdir -p "$state"
  local now; now="$(date +%s)"
  {
    echo "$((now - 3600))     41 pg:app"
    echo "$((now - 93600))    12 my:shop"
    echo "$((now - 259200))    7 lite:notes"
    echo "$((now - 604800))    5 pg:metrics"
    echo "$((now - 1209600))   3 pg:warehouse"
  } > "$state/history"
}

seed_tunnels() {
  # A row is kept only while /proc/<pid>/cmdline still holds that forward's own
  # flag, spec and host, so the stand-in for the ssh this rig must not run has
  # to carry them too: a bare `sleep` is not that pid's forward and is pruned on
  # the first refresh. The loop sleeps in short hops so nothing outlives the
  # teardown that kills its parent.
  local state="$STAGE/.local/state/easysql"
  mkdir -p "$state/tunnels"
  : > "$state/tunnels.tsv"
  local i=0
  while read -r kind spec host; do
    sh -c 'while :; do sleep 5; done' "-$kind" "$spec" "$host" > /dev/null 2>&1 &
    local pid=$!
    echo "$pid" >> "$PIDS"
    local log="$state/tunnels/stage-$i.log"
    : > "$log"
    printf '%s\t%s\t%s\t%s\t%s\n' "$pid" "$kind" "$spec" "$host" "$log" >> "$state/tunnels.tsv"
    i=$((i + 1))
  done <<ROWS
L 15432:127.0.0.1:5432 dbhost
L 13306:127.0.0.1:3306 reporting-vpn
ROWS
}

start_listeners() {
  # One socket on the near end of the forward that is up, so `metrics` is
  # genuinely reachable through it. Nothing on $TUN_PORT, deliberately:
  # `warehouse` keeps a via with no forward running, which is the state the list
  # paints as a sleeping tunnel rather than a dead server, and that distinction
  # is worth a frame. Output is redirected or a child holding stdout open makes
  # `./stage.sh up | anything` hang forever.
  python3 -c "
import socket, time
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('127.0.0.1', 15432)); s.listen(16); time.sleep(86400)" > /dev/null 2>&1 &
  echo $! >> "$PIDS"
}

start_servers() {
  [ -n "$CENGINE" ] || { echo "no podman or docker: the sessions will not open" >&2; return 0; }
  "$CENGINE" rm -f easysql-demo-pg easysql-demo-my > /dev/null 2>&1 || true
  "$CENGINE" run -d --name easysql-demo-pg \
    -e POSTGRES_PASSWORD=devpass -e POSTGRES_USER=dev -e POSTGRES_DB=app \
    -p "127.0.0.1:$PG_PORT:5432" docker.io/library/postgres:16-alpine > /dev/null
  "$CENGINE" run -d --name easysql-demo-my \
    -e MARIADB_ROOT_PASSWORD=devpass -e MARIADB_DATABASE=shop \
    -p "127.0.0.1:$MY_PORT:3306" docker.io/library/mariadb:11 > /dev/null
  echo "waiting for the servers to accept connections..."
  seed_app_data
}

seed_app_data() {
  # Wait for postgres, then give `app` a table worth selecting from in the GIF.
  local i
  for i in $(seq 1 60); do
    if PGPASSWORD=devpass psql -h 127.0.0.1 -p "$PG_PORT" -U dev -d app \
         -c 'select 1' > /dev/null 2>&1; then
      break
    fi
    sleep 1
  done
  PGPASSWORD=devpass psql -h 127.0.0.1 -p "$PG_PORT" -U dev -d app -q <<'SQL' > /dev/null 2>&1 || true
create table if not exists customer (
  id      serial primary key,
  name    text not null,
  email   text not null,
  signed  date not null default current_date
);
truncate customer restart identity;
insert into customer (name, email) values
  ('Ada Lovelace',    'ada@example.com'),
  ('Grace Hopper',    'grace@example.com'),
  ('Alan Turing',     'alan@example.com'),
  ('Katherine Johnson','katherine@example.com');
SQL
  # `warehouse` is `analyst` on `reporting`, reached through the forward the
  # stage's ssh opens, so its session has something of its own to show.
  PGPASSWORD=devpass psql -h 127.0.0.1 -p "$PG_PORT" -U dev -d app -q \
    -c "create role analyst login password 'devpass'" > /dev/null 2>&1 || true
  PGPASSWORD=devpass psql -h 127.0.0.1 -p "$PG_PORT" -U dev -d app -q \
    -c "create database reporting owner analyst" > /dev/null 2>&1 || true
  PGPASSWORD=devpass psql -h 127.0.0.1 -p "$PG_PORT" -U analyst -d reporting -q <<'SQL' > /dev/null 2>&1 || true
create table if not exists sales (region text not null, quarter text not null, total numeric not null);
truncate sales;
insert into sales values
  ('north', 'Q1', 48210), ('south', 'Q1', 39875),
  ('east',  'Q1', 51402), ('west',  'Q1', 27330);
SQL
}

up() {
  down_quiet
  mkdir -p "$STAGE"
  # Stamp it before anything else, so a later `down` can prove this tree is ours.
  : > "$STAGE/$MARKER"
  : > "$PIDS"
  mkdir -p "$STAGE/.config" "$STAGE/.local/state" "$STAGE/.local/share" "$STAGE/.cache"
  write_pg
  write_pgpass
  write_my
  write_easysql_conf
  write_snippets
  write_ssh_config
  write_ssh_shim
  write_vim_shim
  make_sqlite_db
  seed_history
  seed_tunnels
  start_listeners
  start_servers
  echo "staged in $STAGE"
  echo
  echo "  ./stage.sh run    open the toolbox against it"
  echo "  ./stage.sh shell  a shell where esql is this build"
  echo "  ./stage.sh down   tear it all down"
}

# ---------------------------------------------------------------------------
# the teardown guard - identical in every crate's rig
# ---------------------------------------------------------------------------
# A rig is a convenience script with a recursive delete in it, run half
# attentively while thinking about something else, against a path some scenario
# may have mounted a remote filesystem onto. Both halves of that have already
# happened in this workflow: a stage path that pointed somewhere real and was
# deleted because the script trusted its own variable, and an sshfs mount inside
# a staged home torn down with `rm -rf`, which walked through the mountpoint and
# deleted the dotfiles on the machine at the far end. So the delete is proved
# rather than trusted.
refuse() { echo "REFUSING to delete $STAGE: $1" >&2; exit 1; }

assert_safe_to_delete() {
  case "$STAGE" in
    /*) ;;
    *) refuse "the stage path must be absolute" ;;
  esac
  # Resolve symlinks first: a link pointing the stage at something real must not
  # let a delete through on the strength of a harmless-looking path.
  local real
  real="$(cd "$STAGE" && pwd -P)" || refuse "cannot resolve the path"
  case "$real" in
    / | /home | /root | /usr | /etc | /var | /opt | /srv | /boot | /tmp)
      refuse "that is a system directory" ;;
  esac
  [ "$real" = "$HOME" ] && refuse "that is your home directory"
  case "$HOME/" in
    "$real"/*) refuse "your home directory is inside it" ;;
  esac
  # The real gate: only ever delete a tree this script built and stamped.
  [ -f "$real/$MARKER" ] || refuse "no \`$MARKER\` in it, so this script did not build it"
  # Unmount anything under it, longest path first, then check again: a recursive
  # delete walks straight through a mountpoint and removes the far side.
  local mp
  while read -r mp; do
    [ -n "$mp" ] || continue
    echo "unmounting $mp"
    fusermount -u "$mp" 2> /dev/null || umount "$mp" 2> /dev/null || true
  done < <(awk -v s="$real/" '$2 ~ "^"s {print length($2), $2}' /proc/mounts |
             sort -rn | cut -d' ' -f2-)
  if awk -v s="$real/" '$2 ~ "^"s {found=1} END {exit !found}' /proc/mounts; then
    refuse "something is still mounted under it; unmount it by hand and rerun"
  fi
}

down_quiet() {
  if [ -f "$PIDS" ]; then
    while read -r pid; do
      [ -n "$pid" ] && kill "$pid" 2> /dev/null || true
    done < "$PIDS"
  fi
  [ -n "$CENGINE" ] && "$CENGINE" rm -f easysql-demo-pg easysql-demo-my > /dev/null 2>&1 || true
  [ -d "$STAGE" ] || return 0
  assert_safe_to_delete
  # --one-file-system as a second net, in case the mount check was wrong.
  rm -rf --one-file-system "$STAGE"
}

# ---------------------------------------------------------------------------
# the shell in frame - identical in every crate's rig
# ---------------------------------------------------------------------------
# The prompt is invented, and deliberately not the renderer's own. Sourcing a
# real ~/.bashrc paints a different picture on every machine that regenerates
# the assets, which defeats the point of keeping the rig in the repo: these
# images are a build output, and a build output that depends on whose machine
# ran it is not reproducible.
write_demorc() {
  cat > "$STAGE/.demorc" <<'EOF'
PS1='\[\e[38;5;114m\]user@host\[\e[0m\]:\[\e[38;5;110m\]\w\[\e[0m\]\$ '
unset PROMPT_COMMAND
HISTFILE=
clear
EOF
}

# A shell that finds this build as `esql`, so a CLI screenshot shows the command
# you actually type rather than a path into target/release.
open_shell() {
  # The guard that matters: without it a mistyped path or a missing stage drops
  # the tape into the *real* home, and the tool cheerfully renders somebody's
  # actual connections into a README image. Refuse instead.
  [ -f "$STAGE/$MARKER" ] || {
    echo "no stage in $STAGE - run './stage.sh up' first" >&2
    exit 1
  }
  mkdir -p "$BIN"
  ln -sf "$(cd "$(dirname "$ESQL")" && pwd)/esql" "$BIN/esql"
  write_demorc
  (cd "$STAGE" && env -i $(env_for_stage) \
    bash --noprofile --rcfile "$STAGE/.demorc" -i)
}

case "${1:-up}" in
  up)    up ;;
  run)   (cd "$STAGE" && env -i $(env_for_stage) "$ESQL") ;;
  shell) open_shell ;;
  ls)    (cd "$STAGE" && env -i $(env_for_stage) "$ESQL" ls -v) ;;
  down)  down_quiet; echo "torn down" ;;
  *)     echo "usage: $0 [up|run|shell|ls|down]" >&2; exit 2 ;;
esac
