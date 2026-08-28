#!/usr/bin/env bash
# Throwaway postgres + mysql to point easysql at while developing. Not part of
# the crate: this only exists so `Enter` on a connection has something to open.
#
#   dev/db.sh up     start both, on 5432 and 3306
#   dev/db.sh down   remove both, data and all
#
# You still need the *clients* on this machine, since easysql never talks to a
# database itself. esql offers to install them for you when you press Enter on
# a connection, or do it yourself:
#   sudo apt-get install postgresql-client mariadb-client sqlite3
set -euo pipefail

ENGINE=$(command -v podman || command -v docker) || {
    echo "dev/db.sh: neither podman nor docker is installed" >&2
    exit 1
}

up() {
    "$ENGINE" run -d --name easysql-pg \
        -e POSTGRES_PASSWORD=devpass -e POSTGRES_USER=dev -e POSTGRES_DB=app \
        -p 5432:5432 docker.io/library/postgres:17-alpine
    "$ENGINE" run -d --name easysql-my \
        -e MARIADB_ROOT_PASSWORD=devpass -e MARIADB_DATABASE=shop \
        -p 3306:3306 docker.io/library/mariadb:11
    cat <<'HINT'

Both are starting. In esql, press `c` and add:

  postgres   name: dev     host: localhost  port: 5432  database: app   user: dev
  mysql      name: devmy   host: 127.0.0.1  port: 3306  database: shop  user: root

then `p` on each and give the password `devpass`. Enter opens a real session.
HINT
}

down() {
    "$ENGINE" rm -f easysql-pg easysql-my
}

case "${1:-}" in
    up) up ;;
    down) down ;;
    *) echo "usage: dev/db.sh up|down" >&2; exit 2 ;;
esac
