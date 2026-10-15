-- Soft-delete / Corbeille: documents stay until restore or hard purge.
alter table documents add column if not exists deleted_at timestamptz;

create index if not exists documents_owner_deleted_at_idx
  on documents (owner_id, deleted_at);
