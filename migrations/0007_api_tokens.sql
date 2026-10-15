-- Applied at API startup via api_tokens::ensure_schema (mirror for ops/docs).
-- Personal ingest tokens: hash-only at rest, secret shown once (pr_…).

create table if not exists api_tokens (
    id serial primary key,
    owner_id int not null references users(id) on delete cascade,
    name text not null default '',
    token_hash text not null unique,
    prefix text not null,
    created_at timestamptz not null default now(),
    last_used_at timestamptz,
    revoked_at timestamptz
);

create index if not exists api_tokens_owner_id_idx on api_tokens (owner_id);
create index if not exists api_tokens_hash_idx on api_tokens (token_hash) where revoked_at is null;
