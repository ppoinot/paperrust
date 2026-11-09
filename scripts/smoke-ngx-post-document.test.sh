#!/usr/bin/env bash
# Guards for scripts/smoke-ngx-post-document.sh. No network, no database.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT="$ROOT/scripts/smoke-ngx-post-document.sh"
# shellcheck disable=SC1090
source "$SCRIPT"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

assert_prod_refused() {
  if env_is_production; then
    return 0
  fi
  fail "expected production"
}

assert_not_prod() {
  if env_is_production; then
    fail "did not expect production"
  fi
}

PAPERRUST_ENV=production assert_prod_refused
PAPERRUST_ENV=Production assert_prod_refused
PAPERRUST_ENV=' production ' assert_prod_refused
PAPERRUST_ENV=beta assert_not_prod
PAPERRUST_ENV= assert_not_prod
unset PAPERRUST_ENV
assert_not_prod

assert_disposable() {
  if database_is_disposable; then
    return 0
  fi
  fail "expected disposable db"
}
assert_not_disposable() {
  if database_is_disposable; then
    fail "expected non-disposable db"
  fi
}

unset PAPERRUST_DB_DISPOSABLE
unset DATABASE_URL
assert_not_disposable

PAPERRUST_DB_DISPOSABLE=1 assert_disposable
PAPERRUST_DB_DISPOSABLE=true assert_disposable
PAPERRUST_DB_DISPOSABLE=yes assert_disposable
PAPERRUST_DB_DISPOSABLE=0 assert_not_disposable
unset PAPERRUST_DB_DISPOSABLE

DATABASE_URL='postgres://postgres:secret@127.0.0.1:5433/paperrust?sslmode=disable' assert_not_disposable
DATABASE_URL='postgres://postgres:secret@127.0.0.1:5433/paperrust_test?sslmode=disable' assert_disposable
DATABASE_URL='postgres://postgres@127.0.0.1/paperrust-smoke' assert_disposable
DATABASE_URL='postgresql://u:p%40ss@db.example/my_disposable_db' assert_disposable
DATABASE_URL='postgres://postgres@127.0.0.1/contest' assert_not_disposable
DATABASE_URL='postgres://postgres@127.0.0.1/throwaway' assert_disposable
# Flag wins over a production-looking name. Production env is a separate gate.
PAPERRUST_DB_DISPOSABLE=1 DATABASE_URL='postgres://postgres@127.0.0.1/paperrust' assert_disposable
unset PAPERRUST_DB_DISPOSABLE DATABASE_URL

# The URL itself must not be echoed by the name helper.
name="$(db_name_from_url 'postgres://user:s3cret@127.0.0.1:5433/paperrust_test?sslmode=disable')"
[[ "$name" == "paperrust_test" ]] || fail "db name parse: $name"
[[ "$name" != *s3cret* ]] || fail "password leaked into db name"

[[ "$(bulk_delete_body 12 34)" == '{"ids":[12,34],"deleteSelected":true}' ]] || fail "bulk body"
if bulk_delete_body '1;drop' >/dev/null 2>&1; then
  fail "non-integer id accepted"
fi
if bulk_delete_body >/dev/null 2>&1; then
  fail "empty ids accepted"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
printf 'HTTP/1.1 200 OK\r\nX-Document-Id: 42\r\nContent-Type: application/json\r\n\r\n' > "$tmp/h"
[[ "$(header_document_id "$tmp/h")" == "42" ]] || fail "header id"
printf '{"ok":true,"documentId":7,"usedLlm":false}\n' > "$tmp/j"
[[ "$(json_document_id "$tmp/j")" == "7" ]] || fail "json id"
printf '"task-uuid"\n' > "$tmp/task"
[[ -z "$(json_document_id "$tmp/task")" ]] || fail "task uuid is not a document id"

if grep -E -q 'podman|psql|update documents' "$SCRIPT"; then
  fail "smoke script still contains podman or SQL"
fi

echo "OK smoke-ngx guards"
