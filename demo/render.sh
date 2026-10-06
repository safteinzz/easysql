#!/usr/bin/env bash
# Render the README assets in a container, so a machine needs podman or docker
# and nothing else: no vhs, no database clients, no font, and the same frames on
# every machine that runs it.
#
#   ./render.sh              every tape
#   ./render.sh browse       one tape
#
# The two database servers run on this machine, as `stage.sh up` starts them,
# and each tape runs in the image from ./Dockerfile on the host network, so the
# stage inside it reaches them on the same loopback ports.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

ENGINE="$(command -v podman || command -v docker || true)"
[ -n "$ENGINE" ] || { echo "render.sh needs podman or docker" >&2; exit 1; }
IMAGE=localhost/easysql-render

TAPES=("$@")
[ ${#TAPES[@]} -gt 0 ] || TAPES=(browse cli edit passwords shots snippets)

(cd .. && cargo build --release)
# The Dockerfile is the whole build context: nothing in this folder is copied in.
"$ENGINE" build -q -t "$IMAGE" - < Dockerfile > /dev/null

# Run as this user, not root: the images it writes stay yours, and the staged
# shell's prompt ends in `$` as it does on a machine rendering without a
# container, rather than root's `#`.
if [ "$(basename "$ENGINE")" = docker ]; then
  USER_ARGS=(--user "$(id -u):$(id -g)" -e HOME=/tmp)
else
  USER_ARGS=(--userns=keep-id -e HOME=/tmp)
fi

./stage.sh servers
trap './stage.sh down > /dev/null' EXIT

for t in "${TAPES[@]}"; do
  echo "── $t.tape"
  "$ENGINE" run --rm --network host "${USER_ARGS[@]}" \
    -v "$(cd .. && pwd):/work/easysql:Z" -w /work/easysql/demo \
    -e DEMO_SERVERS=external --entrypoint bash "$IMAGE" \
    -c "./stage.sh up > /dev/null && vhs $t.tape > /dev/null; s=\$?; ./stage.sh down > /dev/null; exit \$s"
done
