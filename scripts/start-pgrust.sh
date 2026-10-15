#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
export PATH="/usr/lib/postgresql/18/bin:${PATH}"
export PGRUST_PGSHAREDIR="${PGRUST_PGSHAREDIR:-/usr/share/postgresql/18}"
export PGRUST_TZDIR="${PGRUST_TZDIR:-/usr/share/zoneinfo}"
export RUST_MIN_STACK="${RUST_MIN_STACK:-33554432}"
BIN="$ROOT/bin/pgrust"
DATA="$ROOT/pgrust-data"
PORT="${PGRUST_PORT:-5433}"
# Official v0.3 Linux x86_64 binary (https://github.com/malisper/pgrust/releases/tag/v0.3).
URL="https://pgrust.com/downloads/v0.3/pgrust-0.3-linux-x86_64"
# SHA-256 from the v0.3 GitHub release table and https://pgrust.com/downloads/v0.3/MANIFEST.txt
# (same value in both; sidecar ${URL}.sha256 must match this pin).
EXPECTED_SHA256="9dd36041932656c738563853b191ffce301893e95a5f0babcbfa4fafbded6530"
if ! command -v initdb >/dev/null 2>&1; then
  echo "initdb missing; install PostgreSQL 18 tools (see backend/README.md)" >&2
  exit 1
fi
mkdir -p "$ROOT/bin"
need_download=1
if [[ -x "$BIN" ]]; then
  actual="$(sha256sum "$BIN" | awk "{print \$1}")"
  if [[ "$actual" == "$EXPECTED_SHA256" ]]; then
    need_download=0
  else
    echo "Existing bin/pgrust checksum ${actual} != ${EXPECTED_SHA256}; re-downloading v0.3..."
    rm -f "$BIN" "$BIN.sha256"
  fi
fi
if [[ "$need_download" -eq 1 ]]; then
  echo "Downloading pgrust v0.3..."
  curl -fsSL -o "$BIN" "$URL"
  curl -fsSL -o "$BIN.sha256" "${URL}.sha256"
  sidecar="$(awk "{print \$1}" "$BIN.sha256")"
  actual="$(sha256sum "$BIN" | awk "{print \$1}")"
  if [[ "$actual" != "$EXPECTED_SHA256" || "$sidecar" != "$EXPECTED_SHA256" ]]; then
    echo "checksum mismatch (got ${actual}, sidecar ${sidecar}, pin ${EXPECTED_SHA256})" >&2
    rm -f "$BIN"
    exit 1
  fi
  chmod +x "$BIN"
fi
if [[ ! -f "$DATA/PG_VERSION" ]]; then
  initdb -D "$DATA" --no-locale --encoding UTF8 -U postgres
fi
if ! psql -h 127.0.0.1 -p "$PORT" -U postgres -d postgres -c "select 1" >/dev/null 2>&1; then
  echo "Starting pgrust on 127.0.0.1:${PORT}..."
  ulimit -s 65520 || true
  nohup "$BIN" -D "$DATA" -k /tmp -p "$PORT" -c listen_addresses=127.0.0.1 -c io_method=sync -c max_stack_depth=60000 >"$ROOT/pgrust.log" 2>&1 &
  echo $! > "$ROOT/pgrust.pid"
  for _ in $(seq 1 40); do
    psql -h 127.0.0.1 -p "$PORT" -U postgres -d postgres -c "select version()" >/dev/null 2>&1 && break
    sleep 0.25
  done
fi
VER="$(psql -h 127.0.0.1 -p "$PORT" -U postgres -d postgres -Atc "select version()")"
echo "$VER"
# v0.3: version() is 'PostgreSQL 18.6 (pgrust 0.3)' (was Postgres 18.3 / pgrust 0.2).
if [[ "$VER" != *"pgrust 0.3"* ]]; then
  echo "unexpected version(): ${VER} (want substring 'pgrust 0.3')" >&2
  exit 1
fi
EXISTS=$(psql -h 127.0.0.1 -p "$PORT" -U postgres -d postgres -Atc "SELECT 1 FROM pg_database WHERE datname = 'paperrust'" || true)
if [[ "$EXISTS" != "1" ]]; then
  psql -h 127.0.0.1 -p "$PORT" -U postgres -d postgres -c "CREATE DATABASE paperrust;"
fi
psql -h 127.0.0.1 -p "$PORT" -U postgres -d paperrust -f "$ROOT/migrations/0002_paperrust.sql" >/dev/null
echo "DATABASE_URL=postgres://postgres@127.0.0.1:${PORT}/paperrust?sslmode=disable"
