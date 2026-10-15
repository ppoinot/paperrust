use sqlx::{PgPool, Row};

use crate::models::*;


pub async fn ensure_trash_schema(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::query("alter table documents add column if not exists deleted_at timestamptz")
        .execute(pool)
        .await?;
    sqlx::query(
        "create index if not exists documents_owner_deleted_at_idx on documents (owner_id, deleted_at)",
    )
    .execute(pool)
    .await?;
    Ok(())
}


/// Hard-delete soft-deleted docs older than each owner's retention (30/60/90, default 30).
pub async fn purge_expired_trash(pool: &PgPool) -> sqlx::Result<u64> {
    // Portable interval: avoid make_interval(days => …) (named-arg SETOF path
    // fails on some Postgres builds: expression_returns_set / T_NamedArgExpr).
    let res = sqlx::query(
        r#"delete from documents d
           where d.deleted_at is not null
             and d.deleted_at < now() - (
               interval '1 day' * coalesce(
                 (select case
                    when s.trash_retention_days in (60, 90) then s.trash_retention_days
                    else 30
                  end
                  from user_settings s
                  where s.user_id = d.owner_id),
                 30
               )
             )"#,
    )
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

pub async fn purge_expired_trash_for_owner(pool: &PgPool, owner_id: i32) -> sqlx::Result<u64> {
    let days: i32 = sqlx::query_scalar(
        "select coalesce(trash_retention_days, 30)::int from user_settings where user_id = $1",
    )
    .bind(owner_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(30);
    let days = match days {
        60 | 90 => days,
        _ => 30,
    };
    let res = sqlx::query(
        r#"delete from documents
           where owner_id = $1
             and deleted_at is not null
             and deleted_at < now() - (interval '1 day' * $2)"#,
    )
    .bind(owner_id)
    .bind(days)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

pub async fn list_tags(pool: &PgPool, owner_id: i32) -> sqlx::Result<Vec<Tag>> {
    sqlx::query_as::<_, Tag>(
        "select id, name, color, is_inbox_tag, match, matching_algorithm, is_insensitive from tags where owner_id = $1 order by name",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await
}

pub async fn get_tag(pool: &PgPool, id: i32, owner_id: i32) -> sqlx::Result<Option<Tag>> {
    sqlx::query_as::<_, Tag>(
        "select id, name, color, is_inbox_tag, match, matching_algorithm, is_insensitive from tags where id = $1 and owner_id = $2",
    )
    .bind(id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await
}

pub async fn find_tag_by_name_ci(pool: &PgPool, name: &str, owner_id: i32) -> sqlx::Result<Option<Tag>> {
    sqlx::query_as::<_, Tag>(
        "select id, name, color, is_inbox_tag, match, matching_algorithm, is_insensitive from tags where owner_id = $1 and lower(name) = lower($2) order by id limit 1",
    )
    .bind(owner_id)
    .bind(name)
    .fetch_optional(pool)
    .await
}

pub async fn insert_tag(
    pool: &PgPool,
    owner_id: i32,
    name: &str,
    color: &str,
    is_inbox_tag: bool,
    match_text: &str,
    matching_algorithm: &str,
) -> sqlx::Result<Tag> {
    sqlx::query_as::<_, Tag>(
        "insert into tags (name, color, is_inbox_tag, match, matching_algorithm, owner_id) values ($1,$2,$3,$4,$5,$6) returning id, name, color, is_inbox_tag, match, matching_algorithm, is_insensitive",
    )
    .bind(name)
    .bind(color)
    .bind(is_inbox_tag)
    .bind(match_text)
    .bind(matching_algorithm)
    .bind(owner_id)
    .fetch_one(pool)
    .await
}

pub async fn list_correspondents(pool: &PgPool, owner_id: i32) -> sqlx::Result<Vec<Correspondent>> {
    sqlx::query_as::<_, Correspondent>(
        "select id, name, match, matching_algorithm, is_insensitive, notes from correspondents where owner_id = $1 order by name",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await
}

pub async fn get_correspondent(pool: &PgPool, id: i32, owner_id: i32) -> sqlx::Result<Option<Correspondent>> {
    sqlx::query_as::<_, Correspondent>(
        "select id, name, match, matching_algorithm, is_insensitive, notes from correspondents where id = $1 and owner_id = $2",
    )
    .bind(id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await
}

pub async fn find_correspondent_by_name_ci(pool: &PgPool, name: &str, owner_id: i32) -> sqlx::Result<Option<Correspondent>> {
    sqlx::query_as::<_, Correspondent>(
        "select id, name, match, matching_algorithm, is_insensitive, notes from correspondents where owner_id = $1 and lower(name) = lower($2) order by id limit 1",
    )
    .bind(owner_id)
    .bind(name)
    .fetch_optional(pool)
    .await
}

pub async fn insert_correspondent(
    pool: &PgPool,
    owner_id: i32,
    name: &str,
    match_text: &str,
    matching_algorithm: &str,
    notes: Option<&str>,
) -> sqlx::Result<Correspondent> {
    sqlx::query_as::<_, Correspondent>(
        "insert into correspondents (name, match, matching_algorithm, notes, owner_id) values ($1,$2,$3,$4,$5) returning id, name, match, matching_algorithm, is_insensitive, notes",
    )
    .bind(name)
    .bind(match_text)
    .bind(matching_algorithm)
    .bind(notes)
    .bind(owner_id)
    .fetch_one(pool)
    .await
}

pub async fn list_types(pool: &PgPool, owner_id: i32) -> sqlx::Result<Vec<DocumentType>> {
    sqlx::query_as::<_, DocumentType>(
        "select id, name, match, matching_algorithm, is_insensitive from document_types where owner_id = $1 order by name",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await
}

pub async fn get_type(pool: &PgPool, id: i32, owner_id: i32) -> sqlx::Result<Option<DocumentType>> {
    sqlx::query_as::<_, DocumentType>(
        "select id, name, match, matching_algorithm, is_insensitive from document_types where id = $1 and owner_id = $2",
    )
    .bind(id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await
}

pub async fn find_type_by_name_ci(pool: &PgPool, name: &str, owner_id: i32) -> sqlx::Result<Option<DocumentType>> {
    sqlx::query_as::<_, DocumentType>(
        "select id, name, match, matching_algorithm, is_insensitive from document_types where owner_id = $1 and lower(name) = lower($2) order by id limit 1",
    )
    .bind(owner_id)
    .bind(name)
    .fetch_optional(pool)
    .await
}

pub async fn insert_type(
    pool: &PgPool,
    owner_id: i32,
    name: &str,
    match_text: &str,
    matching_algorithm: &str,
) -> sqlx::Result<DocumentType> {
    sqlx::query_as::<_, DocumentType>(
        "insert into document_types (name, match, matching_algorithm, owner_id) values ($1,$2,$3,$4) returning id, name, match, matching_algorithm, is_insensitive",
    )
    .bind(name)
    .bind(match_text)
    .bind(matching_algorithm)
    .bind(owner_id)
    .fetch_one(pool)
    .await
}

pub async fn list_paths(pool: &PgPool, owner_id: i32) -> sqlx::Result<Vec<StoragePath>> {
    sqlx::query_as::<_, StoragePath>(
        "select id, name, path, match, matching_algorithm, is_insensitive from storage_paths where owner_id = $1 order by name",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await
}

pub async fn list_custom_fields(pool: &PgPool, owner_id: i32) -> sqlx::Result<Vec<CustomField>> {
    let rows = sqlx::query("select id, name, data_type from custom_fields where owner_id = $1 order by name")
        .bind(owner_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| CustomField {
            id: r.get("id"),
            name: r.get("name"),
            data_type: r.get("data_type"),
            extra_data: serde_json::Map::new(),
        })
        .collect())
}

pub async fn list_saved_views(pool: &PgPool, owner_id: i32) -> sqlx::Result<Vec<SavedView>> {
    let rows = sqlx::query(
        "select id, name, show_on_dashboard, show_in_sidebar, sort_field, sort_reverse, filter::text as filter from saved_views where owner_id = $1 order by id",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let filter_s: String = r.get("filter");
            SavedView {
                id: r.get("id"),
                name: r.get("name"),
                show_on_dashboard: r.get("show_on_dashboard"),
                show_in_sidebar: r.get("show_in_sidebar"),
                sort_field: r.get("sort_field"),
                sort_reverse: r.get("sort_reverse"),
                filter: serde_json::from_str(&filter_s).unwrap_or_default(),
            }
        })
        .collect())
}

pub async fn list_workflows(pool: &PgPool, owner_id: i32) -> sqlx::Result<Vec<Workflow>> {
    let rows = sqlx::query(
        "select id, name, enabled, trigger_type, filters::text as filters, actions::text as actions from workflows where owner_id = $1 order by id",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let filters: String = r.get("filters");
            let actions: String = r.get("actions");
            Workflow {
                id: r.get("id"),
                name: r.get("name"),
                enabled: r.get("enabled"),
                trigger_type: r.get("trigger_type"),
                filters: serde_json::from_str(&filters).unwrap_or(WorkflowFilters {
                    content_contains: None,
                    has_correspondent: None,
                }),
                actions: serde_json::from_str(&actions).unwrap_or_default(),
            }
        })
        .collect())
}

pub async fn get_catalog(pool: &PgPool, owner_id: i32) -> sqlx::Result<Catalog> {
    let (tags, correspondents, document_types, storage_paths, custom_fields, saved_views, workflows) =
        tokio::try_join!(
            list_tags(pool, owner_id),
            list_correspondents(pool, owner_id),
            list_types(pool, owner_id),
            list_paths(pool, owner_id),
            list_custom_fields(pool, owner_id),
            list_saved_views(pool, owner_id),
            list_workflows(pool, owner_id),
        )?;
    let mail_accounts = sqlx::query(
        "select id, name, imap_server, username, imap_port from mail_accounts where owner_id = $1 order by name",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| MailAccount {
        id: r.get("id"),
        name: r.get("name"),
        imap_server: r.get("imap_server"),
        username: r.get("username"),
        imap_port: r.get("imap_port"),
    })
    .collect();
    let mail_rules = sqlx::query(
        "select r.id, r.name, r.account_id, r.folder, r.filter_from, r.filter_subject, r.enabled, r.correspondent_id, r.document_type_id from mail_rules r join mail_accounts a on a.id = r.account_id where a.owner_id = $1 order by r.name",
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| MailRule {
        id: r.get("id"),
        name: r.get("name"),
        account_id: r.get("account_id"),
        folder: r.get("folder"),
        filter_from: r.get("filter_from"),
        filter_subject: r.get("filter_subject"),
        enabled: r.get("enabled"),
        correspondent_id: r.get("correspondent_id"),
        document_type_id: r.get("document_type_id"),
    })
    .collect();
    Ok(Catalog {
        tags,
        correspondents,
        document_types,
        storage_paths,
        custom_fields,
        saved_views,
        workflows,
        mail_accounts,
        mail_rules,
        ai_available: ai_available(),
    })
}

pub async fn load_tags_by_document(pool: &PgPool, ids: &[i32]) -> sqlx::Result<std::collections::HashMap<i32, Vec<Tag>>> {
    let mut map = std::collections::HashMap::new();
    if ids.is_empty() {
        return Ok(map);
    }
    let rows = sqlx::query_as::<_, TagRowDoc>(
        r#"select dt.document_id, t.id, t.name, t.color, t.is_inbox_tag, t.match, t.matching_algorithm, t.is_insensitive
           from document_tags dt join tags t on t.id = dt.tag_id
           where dt.document_id = any($1)
           order by t.name"#,
    )
    .bind(ids)
    .fetch_all(pool)
    .await?;
    for row in rows {
        map.entry(row.document_id).or_insert_with(Vec::new).push(Tag {
            id: row.id,
            name: row.name,
            color: row.color,
            is_inbox_tag: row.is_inbox_tag,
            match_text: row.match_text,
            matching_algorithm: row.matching_algorithm,
            is_insensitive: row.is_insensitive,
        });
    }
    Ok(map)
}

#[derive(sqlx::FromRow)]
struct TagRowDoc {
    document_id: i32,
    id: i32,
    name: String,
    color: String,
    is_inbox_tag: bool,
    #[sqlx(rename = "match")]
    match_text: String,
    matching_algorithm: String,
    is_insensitive: bool,
}

fn merge_filter(request: &DocumentFilter, view: &DocumentFilter, sort_field: &str, sort_reverse: bool) -> DocumentFilter {
    let nonempty = |a: &Option<String>, b: &Option<String>| {
        a.as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                b.as_ref()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
    };
    DocumentFilter {
        q: nonempty(&request.q, &view.q),
        tag_ids: request.tag_ids.clone().or_else(|| view.tag_ids.clone()),
        correspondent_id: request.correspondent_id.or(view.correspondent_id),
        document_type_id: request.document_type_id.or(view.document_type_id),
        storage_path_id: request.storage_path_id.or(view.storage_path_id),
        inbox_only: request.inbox_only.or(view.inbox_only),
        trash_only: request.trash_only.or(view.trash_only),
        ocr_status: nonempty(&request.ocr_status, &view.ocr_status),
        date_from: nonempty(&request.date_from, &view.date_from),
        date_to: nonempty(&request.date_to, &view.date_to),
        view_id: request.view_id,
        sort: nonempty(&request.sort, &Some(sort_field.to_string()))
            .or_else(|| nonempty(&request.sort, &view.sort)),
        sort_reverse: request.sort_reverse.or(Some(sort_reverse)).or(view.sort_reverse),
    }
}

async fn resolve_filter(pool: &PgPool, filter: &DocumentFilter, owner_id: i32) -> sqlx::Result<DocumentFilter> {
    let Some(view_id) = filter.view_id else {
        return Ok(filter.clone());
    };
    let row = sqlx::query(
        "select filter::text as filter, sort_field, sort_reverse from saved_views where id = $1 and owner_id = $2",
    )
    .bind(view_id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(filter.clone());
    };
    let filter_s: String = row.get("filter");
    let view_filter: DocumentFilter = serde_json::from_str(&filter_s).unwrap_or_default();
    let sort_field: String = row.get("sort_field");
    let sort_reverse: bool = row.get("sort_reverse");
    Ok(merge_filter(filter, &view_filter, &sort_field, sort_reverse))
}

pub async fn list_documents(pool: &PgPool, filter: &DocumentFilter, owner_id: i32) -> sqlx::Result<Vec<DocumentListItem>> {
    list_documents_limited(pool, filter, owner_id, None).await
}

pub async fn list_documents_limited(
    pool: &PgPool,
    filter: &DocumentFilter,
    owner_id: i32,
    limit: Option<i32>,
) -> sqlx::Result<Vec<DocumentListItem>> {
    let filter = resolve_filter(pool, filter, owner_id).await?;
    let sql = build_list_sql(&filter, owner_id, limit);
    let mut query = sqlx::query_as::<_, DocRow>(&sql.0);
    for b in sql.1 {
        query = match b {
            Bind::Str(s) => query.bind(s),
            Bind::I32(i) => query.bind(i),
        };
    }
    let rows = query.fetch_all(pool).await?;
    let ids: Vec<i32> = rows.iter().map(|r| r.id).collect();
    let tag_map = load_tags_by_document(pool, &ids).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let tags = tag_map.get(&r.id).cloned().unwrap_or_default();
            map_doc(r, tags)
        })
        .collect())
}

enum Bind {
    Str(String),
    I32(i32),
}

fn build_list_sql(filter: &DocumentFilter, owner_id: i32, limit: Option<i32>) -> (String, Vec<Bind>) {
    let mut clauses = vec!["d.owner_id = $1".to_string()];
    if filter.trash_only == Some(true) {
        clauses.push("d.deleted_at is not null".into());
    } else {
        clauses.push("d.deleted_at is null".into());
    }
    let mut binds: Vec<Bind> = vec![Bind::I32(owner_id)];
    let mut n = 1usize;
    let add = |clause: &str, b: Bind, binds: &mut Vec<Bind>, clauses: &mut Vec<String>, n: &mut usize| {
        *n += 1;
        clauses.push(clause.replace('?', &format!("${n}")));
        binds.push(b);
    };

    if let Some(q) = filter.q.as_ref().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        n += 1;
        let p = format!("%{q}%");
        clauses.push(format!(
            "(d.title ilike ${n} or d.content ilike ${n} or c.name ilike ${n} or d.original_filename ilike ${n})"
        ));
        binds.push(Bind::Str(p));
    }
    if let Some(id) = filter.correspondent_id.filter(|v| *v != 0) {
        add("d.correspondent_id = ?", Bind::I32(id), &mut binds, &mut clauses, &mut n);
    }
    if let Some(id) = filter.document_type_id.filter(|v| *v != 0) {
        add("d.document_type_id = ?", Bind::I32(id), &mut binds, &mut clauses, &mut n);
    }
    if let Some(id) = filter.storage_path_id.filter(|v| *v != 0) {
        add("d.storage_path_id = ?", Bind::I32(id), &mut binds, &mut clauses, &mut n);
    }
    if let Some(st) = filter.ocr_status.as_ref().filter(|s| !s.is_empty()) {
        add("d.ocr_status = ?", Bind::Str(st.clone()), &mut binds, &mut clauses, &mut n);
    }
    if let Some(d) = filter.date_from.as_ref().filter(|s| !s.is_empty()) {
        add("d.created_date >= ?::date", Bind::Str(d.clone()), &mut binds, &mut clauses, &mut n);
    }
    if let Some(d) = filter.date_to.as_ref().filter(|s| !s.is_empty()) {
        add("d.created_date <= ?::date", Bind::Str(d.clone()), &mut binds, &mut clauses, &mut n);
    }
    if filter.inbox_only == Some(true) {
        // Match SPA isInboxTag(): flagged OR name "inbox" (case-insensitive).
        clauses.push(
            "exists (select 1 from document_tags dt join tags t on t.id = dt.tag_id where dt.document_id = d.id and (t.is_inbox_tag or lower(trim(t.name)) = 'inbox'))"
                .into(),
        );
    }
    if let Some(tags) = &filter.tag_ids {
        for tag_id in tags {
            add(
                "exists (select 1 from document_tags dt where dt.document_id = d.id and dt.tag_id = ?)",
                Bind::I32(*tag_id),
                &mut binds,
                &mut clauses,
                &mut n,
            );
        }
    }

    let sort_col = match filter.sort.as_deref().unwrap_or("created_date") {
        "added_at" => "d.added_at",
        "title" => "d.title",
        "correspondent" => "c.name",
        "asn" => "d.asn",
        "modified_at" => "d.modified_at",
        _ => "d.created_date",
    };
    let dir = if filter.sort_reverse == Some(false) { "asc" } else { "desc" };
    let mut sql = format!(
        "{} where {} order by {sort_col} {dir} nulls last, d.id desc",
        DOC_SELECT,
        clauses.join(" and ")
    );
    if let Some(lim) = limit {
        n += 1;
        sql.push_str(&format!(" limit ${n}"));
        binds.push(Bind::I32(lim));
    }
    (sql, binds)
}

pub async fn get_document(pool: &PgPool, id: i32, owner_id: i32) -> sqlx::Result<Option<DocumentDetail>> {
    let row = sqlx::query_as::<_, DocRowFull>(
        r#"select
      d.id, d.title, d.content, d.correspondent_id, c.name as correspondent_name,
      d.document_type_id, dt.name as document_type_name,
      d.storage_path_id, sp.name as storage_path_name,
      d.created_date::text as created_date, d.added_at::text as added_at,
      d.modified_at::text as modified_at, d.asn, d.mime_type, d.original_filename,
      d.page_count, d.language, d.ocr_status, d.ocr_provider, d.thumbnail_svg,
      d.file_size, d.summary, d.file_data,
      (d.original_bytes is not null and octet_length(d.original_bytes) > 0) as has_original
    from documents d
    left join correspondents c on c.id = d.correspondent_id and c.owner_id = d.owner_id
    left join document_types dt on dt.id = d.document_type_id and dt.owner_id = d.owner_id
    left join storage_paths sp on sp.id = d.storage_path_id and sp.owner_id = d.owner_id
    where d.id = $1 and d.owner_id = $2"#,
    )
    .bind(id)
    .bind(owner_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let tag_map = load_tags_by_document(pool, &[id]).await?;
    let notes = sqlx::query(
        "select id, body, created_at::text as created_at from notes where document_id = $1 and owner_id = $2 order by created_at desc",
    )
    .bind(id)
    .bind(owner_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|n| DocumentNote {
        id: n.get("id"),
        body: n.get("body"),
        created_at: n.get("created_at"),
    })
    .collect();
    // never leak another tenant's field catalog via this document.
    let custom = sqlx::query(
        r#"select f.id as field_id, f.name, f.data_type, v.value
           from custom_fields f
           left join document_custom_values v on v.field_id = f.id and v.document_id = $1
           where f.owner_id = $2
           order by f.name"#,
    )
    .bind(id)
    .bind(owner_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|c| CustomValue {
        field_id: c.get("field_id"),
        name: c.get("name"),
        data_type: c.get("data_type"),
        value: c.get("value"),
    })
    .collect();
    let history = sqlx::query(
        "select id, action, detail, created_at::text as created_at from audit_log where document_id = $1 order by created_at desc, id desc",
    )
    .bind(id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|h| HistoryItem {
        id: h.get("id"),
        action: h.get("action"),
        detail: h.get("detail"),
        created_at: h.get("created_at"),
    })
    .collect();
    let file_data = sanitize_svg(row.file_data.clone());
    let item = map_doc(
        DocRow {
            id: row.id,
            title: row.title,
            content: row.content,
            correspondent_id: row.correspondent_id,
            correspondent_name: row.correspondent_name,
            document_type_id: row.document_type_id,
            document_type_name: row.document_type_name,
            storage_path_id: row.storage_path_id,
            storage_path_name: row.storage_path_name,
            created_date: row.created_date,
            added_at: row.added_at,
            modified_at: row.modified_at,
            asn: row.asn,
            mime_type: row.mime_type,
            original_filename: row.original_filename,
            page_count: row.page_count,
            language: row.language,
            ocr_status: row.ocr_status,
            ocr_provider: row.ocr_provider,
            thumbnail_svg: row.thumbnail_svg,
            file_size: row.file_size,
            summary: row.summary,
        },
        tag_map.get(&id).cloned().unwrap_or_default(),
    );
    Ok(Some(DocumentDetail {
        item,
        file_data,
        has_original: row.has_original,
        notes,
        custom_values: custom,
        history,
    }))
}

#[derive(sqlx::FromRow)]
struct DocRowFull {
    id: i32,
    title: String,
    content: String,
    correspondent_id: Option<i32>,
    correspondent_name: Option<String>,
    document_type_id: Option<i32>,
    document_type_name: Option<String>,
    storage_path_id: Option<i32>,
    storage_path_name: Option<String>,
    created_date: Option<String>,
    added_at: String,
    modified_at: String,
    asn: Option<i32>,
    mime_type: String,
    original_filename: Option<String>,
    page_count: i32,
    language: String,
    ocr_status: String,
    ocr_provider: Option<String>,
    thumbnail_svg: Option<String>,
    file_size: i32,
    summary: Option<String>,
    file_data: Option<String>,
    has_original: bool,
}

pub async fn get_dashboard(pool: &PgPool, owner_id: i32) -> sqlx::Result<DashboardStats> {
    // pgrust rejects InitPlan scalar subselects in the same SELECT (EEOP_PARAM_EXEC).
    // Keep document aggregates plain; fetch correspondent/tag counts as separate statements.
    let counts = sqlx::query(
        r#"select
      count(*)::int as documents,
      coalesce(sum(page_count),0)::int as pages,
      coalesce(sum(file_size),0)::int as storage,
      count(*) filter (where ocr_status = 'done')::int as ocr_done,
      count(*) filter (where ocr_status in ('pending','processing'))::int as ocr_pending
    from documents where owner_id = $1 and deleted_at is null"#,
    )
    .bind(owner_id)
    .fetch_one(pool)
    .await?;
    let correspondents: i32 = sqlx::query_scalar("select count(*)::int from correspondents where owner_id = $1")
        .bind(owner_id)
        .fetch_one(pool)
        .await?;
    let tags: i32 = sqlx::query_scalar("select count(*)::int from tags where owner_id = $1")
        .bind(owner_id)
        .fetch_one(pool)
        .await?;
    let inbox: i32 = sqlx::query_scalar(
        r#"select count(distinct d.id)::int as n
           from documents d
           join document_tags dt on dt.document_id = d.id
           join tags t on t.id = dt.tag_id and (t.is_inbox_tag or lower(trim(t.name)) = 'inbox')
           where d.owner_id = $1 and d.deleted_at is null"#,
    )
    .bind(owner_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let recent = list_documents_limited(
        pool,
        &DocumentFilter {
            sort: Some("added_at".into()),
            sort_reverse: Some(true),
            ..Default::default()
        },
        owner_id,
        Some(6),
    )
    .await?;
    let by_month = sqlx::query(
        r#"select substring(created_date::text, 1, 7) as month, count(*)::int as count
           from documents where owner_id = $1 and deleted_at is null and created_date is not null group by 1 order by 1"#,
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| MonthCount {
        month: r.get("month"),
        count: r.get("count"),
    })
    .collect();
    let by_type = sqlx::query(
        r#"select coalesce(dt.name, 'Sans type') as name, count(*)::int as count
           from documents d
           left join document_types dt on dt.id = d.document_type_id
           where d.owner_id = $1 and d.deleted_at is null
           group by 1 order by 2 desc"#,
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| TypeCount {
        name: r.get("name"),
        count: r.get("count"),
    })
    .collect();
    let jobs = sqlx::query(
        r#"select j.id, j.filename, j.status, j.message, j.document_id, j.created_at::text as created_at, j.finished_at::text as finished_at
           from consume_jobs j
           left join documents d on d.id = j.document_id
           where j.owner_id = $1 or (j.owner_id is null and d.owner_id = $1)
           order by j.created_at desc limit 8"#,
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|j| ConsumeJob {
        id: j.get("id"),
        filename: j.get("filename"),
        status: j.get("status"),
        message: j.get("message"),
        document_id: j.get("document_id"),
        created_at: j.get("created_at"),
        finished_at: j.get("finished_at"),
    })
    .collect();
    Ok(DashboardStats {
        documents: counts.get("documents"),
        inbox,
        correspondents,
        tags,
        pages: counts.get("pages"),
        storage_bytes: counts.get("storage"),
        ocr_done: counts.get("ocr_done"),
        ocr_pending: counts.get("ocr_pending"),
        recent,
        by_month,
        by_type,
        jobs,
        views: list_saved_views(pool, owner_id).await?,
    })
}

/// next ASN under a per-owner advisory transaction lock (call inside a txn
/// that also inserts the document). Key namespace `0x50415253` ("PARS") + owner_id.
pub async fn allocate_asn(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    owner_id: i32,
) -> sqlx::Result<i32> {
    sqlx::query("select pg_advisory_xact_lock($1, $2)")
        .bind(0x5041_5253_i32)
        .bind(owner_id)
        .execute(&mut **tx)
        .await?;
    let n: i32 = sqlx::query_scalar(
        "select coalesce(max(asn), 1000)::int as n from documents where owner_id = $1",
    )
    .bind(owner_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(n + 1)
}

pub async fn log_audit(pool: &PgPool, document_id: i32, action: &str, detail: Option<&str>) -> sqlx::Result<()> {
    sqlx::query("insert into audit_log (document_id, action, detail) values ($1,$2,$3)")
        .bind(document_id)
        .bind(action)
        .bind(detail)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_jobs(pool: &PgPool, owner_id: i32) -> sqlx::Result<Vec<JobOut>> {
    let rows = sqlx::query(
        r#"select j.id, j.filename, j.status, j.message, j.document_id, j.created_at::text as created_at, j.finished_at::text as finished_at
           from consume_jobs j
           left join documents d on d.id = j.document_id
           where j.owner_id = $1 or (j.owner_id is null and d.owner_id = $1)
           order by j.created_at desc limit 50"#,
    )
    .bind(owner_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|j| {
            JobOut::new(
                j.get("id"),
                j.get("filename"),
                j.get("status"),
                j.get("message"),
                j.get("document_id"),
                j.get("created_at"),
                j.get("finished_at"),
            )
        })
        .collect())
}
