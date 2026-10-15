-- Corbeille retention (days) per user. Allowed values: 30, 60, 90 (default 30).
alter table user_settings
  add column if not exists trash_retention_days int not null default 30;

-- Clamp existing unexpected values.
update user_settings
  set trash_retention_days = 30
  where trash_retention_days not in (30, 60, 90);
