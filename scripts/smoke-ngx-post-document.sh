#!/usr/bin/env bash
# Smoke: Paperless-ngx post_document compat (no token value printed).
# Cleanup is POST /api/documents/bulk for the document ids of this run only.
# Refuses production and any database that is not disposable.
set -euo pipefail

env_is_production() {
  local env_name
  env_name="$(printf '%s' "${PAPERRUST_ENV:-}" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
  [[ "$env_name" == "production" ]]
}

# Database name is the URL path, query string stripped. Never print the URL.
db_name_from_url() {
  local url="${1:-}"
  url="${url%%\?*}"
  url="${url%/}"
  printf '%s' "${url##*/}"
}

is_disposable_db_name() {
  local name
  name="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')"
  [[ "$name" =~ (^|[^a-z0-9])(test|smoke|disposable|throwaway|scratch|tmp|temp)([^a-z0-9]|$) ]]
}

database_is_disposable() {
  local flag
  flag="$(printf '%s' "${PAPERRUST_DB_DISPOSABLE:-}" | tr '[:upper:]' '[:lower:]' | tr -d '[:space:]')"
  if [[ "$flag" == "1" || "$flag" == "true" || "$flag" == "yes" ]]; then
    return 0
  fi
  if [[ -n "${DATABASE_URL:-}" ]] && is_disposable_db_name "$(db_name_from_url "$DATABASE_URL")"; then
    return 0
  fi
  return 1
}

header_document_id() {
  # mawk has no IGNORECASE; compare the field in lowercase.
  awk 'tolower($1) == "x-document-id:" { gsub(/\r/, "", $2); print $2; exit }' "$1"
}

json_document_id() {
  python3 -c 'import json,sys
try:
    v=json.load(open(sys.argv[1], encoding="utf-8"))
except Exception:
    raise SystemExit(0)
if isinstance(v, dict) and isinstance(v.get("documentId"), int):
    print(v["documentId"])
' "$1"
}

is_run_id() {
  [[ "${1:-}" =~ ^[0-9]+$ ]]
}

# JSON body for soft-delete of this run's ids. Rejects anything that is not an integer.
bulk_delete_body() {
  local first=1 id out='{"ids":['
  if [[ $# -eq 0 ]]; then
    echo "KO: aucun id de run à nettoyer" >&2
    return 1
  fi
  for id in "$@"; do
    if ! is_run_id "$id"; then
      echo "KO: id de run invalide" >&2
      return 1
    fi
    if [[ $first -eq 0 ]]; then
      out+=','
    fi
    first=0
    out+="$id"
  done
  out+='],"deleteSelected":true}'
  printf '%s' "$out"
}

# Sourced by the test harness. Executing the file continues into the smoke.
if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then
  return 0
fi

if env_is_production; then
  echo "KO: PAPERRUST_ENV=production — smoke refusé" >&2
  exit 2
fi
if ! database_is_disposable; then
  echo "KO: base non jetable — smoke refusé (PAPERRUST_DB_DISPOSABLE=1 ou nom de base jetable)" >&2
  exit 2
fi

BASE="${PAPERRUST_BASE:-http://127.0.0.1:8788}"
TOKEN="${PAPERRUST_API_TOKEN:-}"
if [[ -z "$TOKEN" ]]; then
  echo "KO: set PAPERRUST_API_TOKEN=pr_… (never commit it)" >&2
  exit 2
fi
TMP="$(mktemp -d)"
TITLE="smoke-ngx-$(date -u +%Y%m%dT%H%M%SZ)"
trap 'rm -rf "$TMP"' EXIT
# Minimal valid-ish JPEG (1x1)
printf '\xff\xd8\xff\xd9' > "$TMP/t.jpg"

code="$(curl -sS -D "$TMP/h1" -o "$TMP/body" -w "%{http_code}" -X POST "$BASE/api/documents/post_document/" \
  -H "Authorization: Token ${TOKEN}" \
  -F "document=@${TMP}/t.jpg;type=image/jpeg" \
  -F "title=${TITLE}")"
echo "post_document Token → HTTP $code"
if [[ "$code" != "200" && "$code" != "201" ]]; then
  echo "KO body:"; head -c 200 "$TMP/body"; echo; exit 1
fi

code2="$(curl -sS -D "$TMP/h2" -o "$TMP/body2" -w "%{http_code}" -X POST "$BASE/api/documents/post_document" \
  -H "Authorization: Bearer ${TOKEN}" \
  -F "document=@${TMP}/t.jpg;type=image/jpeg" \
  -F "title=${TITLE}")"
echo "post_document Bearer → HTTP $code2"

code3="$(curl -sS -o /dev/null -w "%{http_code}" -X POST "$BASE/api/documents/post_document/" )"
echo "post_document no auth → HTTP $code3 (expect 401)"

code4="$(curl -sS -D "$TMP/h4" -o "$TMP/ingest" -w "%{http_code}" -X POST "$BASE/api/v1/ingest" \
  -H "Authorization: Bearer ${TOKEN}" \
  -F "file=@${TMP}/t.jpg;type=image/jpeg" \
  -F "title=${TITLE}")"
echo "v1/ingest still works → HTTP $code4"

ids=()
for header_file in "$TMP/h1" "$TMP/h2"; do
  if [[ -f "$header_file" ]]; then
    id="$(header_document_id "$header_file" || true)"
    if is_run_id "$id"; then
      ids+=("$id")
    fi
  fi
done
if [[ -f "$TMP/ingest" ]]; then
  id="$(json_document_id "$TMP/ingest" || true)"
  if is_run_id "$id"; then
    ids+=("$id")
  fi
fi

if [[ ${#ids[@]} -eq 0 ]]; then
  echo "KO: aucun id de run renvoyé par l'API" >&2
  exit 1
fi
body="$(bulk_delete_body "${ids[@]}")"
code_clean="$(curl -sS -o "$TMP/cleanup" -w "%{http_code}" -X POST "$BASE/api/documents/bulk" \
  -H "Authorization: Token ${TOKEN}" \
  -H "Content-Type: application/json" \
  --data "$body")"
echo "cleanup run ids (${#ids[@]}) → HTTP $code_clean"
if [[ "$code_clean" != "200" ]]; then
  echo "KO cleanup" >&2
  head -c 200 "$TMP/cleanup" >&2 || true
  echo >&2
  exit 1
fi

if [[ "$code" =~ ^20[01]$ && "$code3" == "401" && "$code4" =~ ^20 ]]; then
  echo "OK"
else
  echo "KO"
  exit 1
fi
