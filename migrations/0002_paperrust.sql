create table if not exists correspondents (
  id serial primary key,
  name text not null unique,
  match text not null default '',
  matching_algorithm text not null default 'auto',
  is_insensitive boolean not null default true,
  notes text
);

create table if not exists document_types (
  id serial primary key,
  name text not null unique,
  match text not null default '',
  matching_algorithm text not null default 'auto',
  is_insensitive boolean not null default true
);

create table if not exists storage_paths (
  id serial primary key,
  name text not null unique,
  path text not null,
  match text not null default '',
  matching_algorithm text not null default 'none',
  is_insensitive boolean not null default true
);

create table if not exists tags (
  id serial primary key,
  name text not null unique,
  color text not null default '#57534e',
  is_inbox_tag boolean not null default false,
  match text not null default '',
  matching_algorithm text not null default 'none',
  is_insensitive boolean not null default true
);

create table if not exists custom_fields (
  id serial primary key,
  name text not null unique,
  data_type text not null default 'string',
  extra_data jsonb not null default '{}'::jsonb
);

create table if not exists documents (
  id serial primary key,
  title text not null,
  content text not null default '',
  correspondent_id int references correspondents(id) on delete set null,
  document_type_id int references document_types(id) on delete set null,
  storage_path_id int references storage_paths(id) on delete set null,
  created_date date,
  added_at timestamptz not null default now(),
  modified_at timestamptz not null default now(),
  asn int unique,
  checksum text,
  mime_type text not null default 'image/svg+xml',
  original_filename text,
  page_count int not null default 1,
  language text not null default 'fr',
  ocr_status text not null default 'done',
  ocr_provider text,
  thumbnail_svg text,
  file_data text,
  file_size int not null default 0,
  summary text
);

create table if not exists document_tags (
  document_id int not null references documents(id) on delete cascade,
  tag_id int not null references tags(id) on delete cascade,
  primary key (document_id, tag_id)
);

create table if not exists document_custom_values (
  document_id int not null references documents(id) on delete cascade,
  field_id int not null references custom_fields(id) on delete cascade,
  value text,
  primary key (document_id, field_id)
);

create table if not exists notes (
  id serial primary key,
  document_id int not null references documents(id) on delete cascade,
  body text not null,
  created_at timestamptz not null default now()
);

create table if not exists saved_views (
  id serial primary key,
  name text not null,
  show_on_dashboard boolean not null default false,
  show_in_sidebar boolean not null default true,
  sort_field text not null default 'created_date',
  sort_reverse boolean not null default true,
  filter jsonb not null default '{}'::jsonb
);

create table if not exists workflows (
  id serial primary key,
  name text not null,
  enabled boolean not null default true,
  trigger_type text not null default 'document_added',
  filters jsonb not null default '{}'::jsonb,
  actions jsonb not null default '[]'::jsonb
);

create table if not exists mail_accounts (
  id serial primary key,
  name text not null,
  imap_server text,
  username text,
  imap_port int default 993
);

create table if not exists mail_rules (
  id serial primary key,
  name text not null,
  account_id int references mail_accounts(id) on delete cascade,
  folder text default 'INBOX',
  filter_from text,
  filter_subject text,
  enabled boolean not null default true,
  correspondent_id int references correspondents(id) on delete set null,
  document_type_id int references document_types(id) on delete set null
);

create table if not exists consume_jobs (
  id serial primary key,
  filename text not null,
  status text not null default 'queued',
  message text,
  document_id int references documents(id) on delete set null,
  created_at timestamptz not null default now(),
  finished_at timestamptz
);

create table if not exists audit_log (
  id serial primary key,
  document_id int references documents(id) on delete cascade,
  action text not null,
  detail text,
  created_at timestamptz not null default now()
);

create table if not exists app_settings (
  key text primary key,
  value jsonb not null
);

create index if not exists documents_created_date_idx on documents (created_date desc);
create index if not exists documents_added_at_idx on documents (added_at desc);
create index if not exists documents_correspondent_idx on documents (correspondent_id);
create index if not exists documents_type_idx on documents (document_type_id);
create index if not exists documents_asn_idx on documents (asn);
create index if not exists audit_log_document_idx on audit_log (document_id, created_at desc);
