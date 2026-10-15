-- Identity + per-owner isolation. Applied at API startup (auth::ensure_schema).
-- Do not store OIDC secrets here.

create table if not exists users (
  id serial primary key,
  oidc_sub text not null unique,
  email text not null,
  name text not null,
  created_at timestamptz not null default now()
);

insert into users (id, oidc_sub, email, name)
select 1, 'bootstrap:admin', 'admin@example.invalid', 'Admin'
where not exists (select 1 from users);

select setval('users_id_seq', (select greatest(coalesce(max(id), 1), 1) from users));

create table if not exists auth_sessions (
  token text primary key,
  user_id int not null references users(id) on delete cascade,
  issuer_base text not null,
  id_token text,
  expires_at timestamptz not null,
  created_at timestamptz not null default now()
);

create table if not exists oidc_pending (
  state text primary key,
  code_verifier text not null,
  nonce text not null,
  issuer_base text not null,
  redirect_uri text not null,
  expires_at timestamptz not null
);

alter table documents add column if not exists owner_id int references users(id);
alter table tags add column if not exists owner_id int references users(id);
alter table document_types add column if not exists owner_id int references users(id);
alter table correspondents add column if not exists owner_id int references users(id);

update documents set owner_id = 1 where owner_id is null;
update tags set owner_id = 1 where owner_id is null;
update document_types set owner_id = 1 where owner_id is null;
update correspondents set owner_id = 1 where owner_id is null;

alter table documents alter column owner_id set not null;
alter table tags alter column owner_id set not null;
alter table document_types alter column owner_id set not null;
alter table correspondents alter column owner_id set not null;

alter table tags drop constraint if exists tags_name_key;
alter table correspondents drop constraint if exists correspondents_name_key;
alter table document_types drop constraint if exists document_types_name_key;
alter table documents drop constraint if exists documents_asn_key;
drop index if exists tags_name_key;
drop index if exists correspondents_name_key;
drop index if exists document_types_name_key;
drop index if exists documents_asn_key;

create unique index if not exists tags_owner_name_ci on tags (owner_id, lower(name));
create unique index if not exists correspondents_owner_name_ci on correspondents (owner_id, lower(name));
create unique index if not exists document_types_owner_name_ci on document_types (owner_id, lower(name));
create unique index if not exists documents_owner_asn on documents (owner_id, asn) where asn is not null;

create index if not exists documents_owner_id_idx on documents (owner_id);
create index if not exists tags_owner_id_idx on tags (owner_id);
create index if not exists document_types_owner_id_idx on document_types (owner_id);
create index if not exists correspondents_owner_id_idx on correspondents (owner_id);
create index if not exists auth_sessions_user_id_idx on auth_sessions (user_id);
create index if not exists auth_sessions_expires_idx on auth_sessions (expires_at);

alter table notes add column if not exists owner_id int references users(id);
alter table storage_paths add column if not exists owner_id int references users(id);
alter table custom_fields add column if not exists owner_id int references users(id);
alter table saved_views add column if not exists owner_id int references users(id);
alter table workflows add column if not exists owner_id int references users(id);
alter table mail_accounts add column if not exists owner_id int references users(id);
alter table consume_jobs add column if not exists owner_id int references users(id);
update notes set owner_id = 1 where owner_id is null;
update storage_paths set owner_id = 1 where owner_id is null;
update custom_fields set owner_id = 1 where owner_id is null;
update saved_views set owner_id = 1 where owner_id is null;
update workflows set owner_id = 1 where owner_id is null;
update mail_accounts set owner_id = 1 where owner_id is null;
update consume_jobs set owner_id = 1 where owner_id is null;
create unique index if not exists storage_paths_owner_name_ci on storage_paths (owner_id, lower(name));

create unique index if not exists saved_views_owner_name_ci on saved_views (owner_id, lower(name));
create unique index if not exists custom_fields_owner_name_ci on custom_fields (owner_id, lower(name));

