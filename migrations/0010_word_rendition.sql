-- Word originals stay in original_bytes. The PDF WordCraft exports is the preview.
alter table documents add column if not exists rendition_pdf bytea;
alter table documents add column if not exists rendition_at timestamptz;
