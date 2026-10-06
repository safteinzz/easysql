# The tape renderer: vhs's own image, which already carries ttyd, chromium,
# ffmpeg and the JetBrainsMono NF font every tape sets, plus what the stage runs
# inside the frame. render.sh builds it and runs every tape in it, so rendering
# needs podman or docker and nothing else on the machine.
FROM ghcr.io/charmbracelet/vhs:v0.12.1@sha256:3afed459e6d7860f6bb8b9a22ae29d5927d628dac6e30f32a250113406a348b9

# Debian as it was at this moment, from snapshot.debian.org, so every package and
# every library it pulls in is the same on any machine on any day. The normal
# mirror keeps only the current version, which would make a pinned version fail
# to install after the next point release. Move the date to update.
ARG SNAPSHOT=20261006T000000Z

# The clients the sessions open, sqlite3 for the notes database, python3 for the
# stage's ssh and listener stand-ins, and the editor `o` opens.
RUN rm -f /etc/apt/sources.list.d/debian.sources /etc/apt/sources.list \
 && for suite in trixie trixie-updates; do \
      echo "deb http://snapshot.debian.org/archive/debian/$SNAPSHOT $suite main"; \
    done > /etc/apt/sources.list \
 && echo "deb http://snapshot.debian.org/archive/debian-security/$SNAPSHOT trixie-security main" \
      >> /etc/apt/sources.list \
 && apt-get -o Acquire::Check-Valid-Until=false update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      postgresql-client mariadb-client sqlite3 python3 vim procps \
 && rm -rf /var/lib/apt/lists/*
