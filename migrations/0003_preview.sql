-- Original bytes + per-page JPEG preview cache (pgrust, no diesel).
-- Applied at API startup by preview::ensure_schema.

alter table documents add column if not exists original_bytes bytea;

create table if not exists document_preview_pages (
  document_id int not null references documents(id) on delete cascade,
  page int not null,
  jpeg bytea not null,
  primary key (document_id, page)
);
