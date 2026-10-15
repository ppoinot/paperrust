-- Speed up OCR/title/filename ILIKE '%q%' search while keeping full-content matching.
create extension if not exists pg_trgm;

create index if not exists documents_content_trgm_idx
  on documents using gin (content gin_trgm_ops);

create index if not exists documents_title_trgm_idx
  on documents using gin (title gin_trgm_ops);

create index if not exists documents_original_filename_trgm_idx
  on documents using gin (original_filename gin_trgm_ops);

create index if not exists documents_owner_added_at_idx
  on documents (owner_id, added_at desc);

create index if not exists documents_owner_created_date_idx
  on documents (owner_id, created_date desc nulls last);
