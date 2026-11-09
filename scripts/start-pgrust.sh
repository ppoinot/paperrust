#!/usr/bin/env bash
# Start the current latest pgrust release (v0.4-beta) on loopback for a
# single-user machine. The tarball ships its own postgres, initdb, and psql.
# Docker Hub tag "latest" is still v0.3; this script pins the v0.4-beta tarball.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export RUST_MIN_STACK="${RUST_MIN_STACK:-33554432}"

VERSION="0.4-beta"
PLATFORM="linux-x86_64"
TARBALL="pgrust-${VERSION}-${PLATFORM}.tar.gz"
URL="https://pgrust.com/downloads/v${VERSION}/${TARBALL}"
# SHA-256 from https://pgrust.com/downloads/v0.4-beta/MANIFEST.txt
# (sidecar ${URL}.sha256 must match this pin).
EXPECTED_SHA256="9a53fdc039ca338808702641369b0ac7d6c163124f76f2bfd21860b0be9fb7bb"
DEST="$ROOT/bin/pgrust-${VERSION}-${PLATFORM}"
BIN="$DEST/bin/postgres"
INITDB="$DEST/bin/initdb"
PSQL="$DEST/bin/psql"
DATA="$ROOT/pgrust-data"
PORT="${PGRUST_PORT:-5433}"

install_pgrust() {
  local stage actual sidecar
  stage="$(mktemp -d)"
  echo "Downloading pgrust ${VERSION} (${PLATFORM})..."
  curl -fsSL -o "$stage/$TARBALL" "$URL"
  curl -fsSL -o "$stage/$TARBALL.sha256" "${URL}.sha256"
  actual="$(sha256sum "$stage/$TARBALL" | awk '{print $1}')"
  sidecar="$(awk '{print $1}' "$stage/$TARBALL.sha256")"
  if [[ "$actual" != "$EXPECTED_SHA256" || "$sidecar" != "$EXPECTED_SHA256" ]]; then
    echo "checksum mismatch (got ${actual}, sidecar ${sidecar}, pin ${EXPECTED_SHA256})" >&2
    rm -rf "$stage"
    exit 1
  fi
  tar -xzf "$stage/$TARBALL" -C "$stage"
  local unpacked
  unpacked="$(find "$stage" -mindepth 1 -maxdepth 1 -type d -name "pgrust-${VERSION}-linux-*" -print -quit)"
  if [[ -z "$unpacked" || ! -x "$unpacked/bin/postgres" ]]; then
    echo "tarball did not contain bin/postgres" >&2
    rm -rf "$stage"
    exit 1
  fi
  rm -rf "$DEST"
  mkdir -p "$ROOT/bin"
  mv "$unpacked" "$DEST"
  printf '%s\n' "$EXPECTED_SHA256" > "$DEST/.sha256"
  rm -rf "$stage"
}

if [[ -x "$BIN" && -x "$INITDB" && -x "$PSQL" && -f "$DEST/.sha256" ]] \
  && [[ "$(tr -d '[:space:]' < "$DEST/.sha256")" == "$EXPECTED_SHA256" ]]; then
  :
else
  install_pgrust
fi

if [[ ! -f "$DATA/PG_VERSION" ]]; then
  "$INITDB" -D "$DATA" --no-locale --encoding UTF8 -U postgres
fi
if ! "$PSQL" -h 127.0.0.1 -p "$PORT" -U postgres -d postgres -c "select 1" >/dev/null 2>&1; then
  echo "Starting pgrust on 127.0.0.1:${PORT}..."
  ulimit -s 65520 || true
  nohup "$BIN" -D "$DATA" -k /tmp -p "$PORT" -c listen_addresses=127.0.0.1 -c io_method=sync -c max_stack_depth=60000 >"$ROOT/pgrust.log" 2>&1 &
  echo $! > "$ROOT/pgrust.pid"
  for _ in $(seq 1 40); do
    "$PSQL" -h 127.0.0.1 -p "$PORT" -U postgres -d postgres -c "select version()" >/dev/null 2>&1 && break
    sleep 0.25
  done
fi
VER="$("$PSQL" -h 127.0.0.1 -p "$PORT" -U postgres -d postgres -Atc "select version()")"
echo "$VER"
# v0.4-beta: version() contains 'pgrust 0.4-beta'.
if [[ "$VER" != *"pgrust 0.4-beta"* ]]; then
  echo "unexpected version(): ${VER} (want substring 'pgrust 0.4-beta')" >&2
  exit 1
fi
EXISTS=$("$PSQL" -h 127.0.0.1 -p "$PORT" -U postgres -d postgres -Atc "SELECT 1 FROM pg_database WHERE datname = 'paperrust'" || true)
if [[ "$EXISTS" != "1" ]]; then
  "$PSQL" -h 127.0.0.1 -p "$PORT" -U postgres -d postgres -c "CREATE DATABASE paperrust;"
fi
"$PSQL" -h 127.0.0.1 -p "$PORT" -U postgres -d paperrust -v ON_ERROR_STOP=1 -c \
  "CREATE TABLE IF NOT EXISTS _migrations (name TEXT PRIMARY KEY, applied_at TIMESTAMPTZ NOT NULL DEFAULT now())"
shopt -s nullglob
for file in "$ROOT"/migrations/*.sql; do
  name="$(basename "$file")"
  # Filenames are repo migrations. Reject anything that is not a plain SQL name
  # before it is placed in a query (this psql does not expand :'variables').
  [[ "$name" =~ ^[0-9][A-Za-z0-9_.-]*\.sql$ ]] || continue
  applied="$("$PSQL" -h 127.0.0.1 -p "$PORT" -U postgres -d paperrust -Atc \
    "SELECT 1 FROM _migrations WHERE name = '${name}'")"
  if [[ "$applied" == "1" ]]; then
    continue
  fi
  # One session so a failed statement rolls back. This psql has no -1/--single-transaction.
  {
    printf 'BEGIN;\n'
    cat "$file"
    printf "INSERT INTO _migrations (name) VALUES ('%s');\nCOMMIT;\n" "$name"
  } | "$PSQL" -h 127.0.0.1 -p "$PORT" -U postgres -d paperrust -v ON_ERROR_STOP=1 -f -
done
echo "DATABASE_URL=postgres://postgres@127.0.0.1:${PORT}/paperrust?sslmode=disable"
