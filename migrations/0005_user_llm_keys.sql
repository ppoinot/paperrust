-- Per-user OCR LLM API keys + settings. Applied at API startup (llm_keys::ensure_schema).

create table if not exists user_llm_keys (
  user_id int not null references users(id) on delete cascade,
  provider text not null,
  ciphertext bytea not null,
  hint text not null default '',
  updated_at timestamptz not null default now(),
  primary key (user_id, provider)
);

create table if not exists user_settings (
  user_id int primary key references users(id) on delete cascade,
  ocr_provider text not null default 'xai',
  ocr_language text not null default 'fr',
  auto_match boolean not null default true,
  updated_at timestamptz not null default now()
);

create index if not exists user_llm_keys_user_id_idx on user_llm_keys (user_id);
