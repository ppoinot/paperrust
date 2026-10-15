use sqlx::{PgPool, Row};

use crate::paper::{render_paper_svg, PaperSpec};

#[derive(serde::Deserialize)]
struct SeedDoc {
    title: String,
    correspondent: String,
    #[serde(rename = "type")]
    type_: String,
    path: String,
    date: String,
    added: String,
    asn: i32,
    tags: Vec<String>,
    language: String,
    summary: String,
    filename: String,
    #[serde(default)]
    r#ref: Option<String>,
    #[serde(default)]
    stamp: Option<String>,
    #[serde(default)]
    custom: std::collections::HashMap<String, String>,
    lines: Vec<String>,
    content: String,
}

async fn repair_saved_views(pool: &PgPool) -> anyhow::Result<()> {
    let tid: Option<i32> = sqlx::query_scalar("select id from tags where name = 'Santé' limit 1")
        .fetch_optional(pool)
        .await?;
    let Some(tid) = tid else {
        return Ok(());
    };
    let filter = serde_json::json!({ "tagIds": [tid] }).to_string();
    sqlx::query(
        r#"update saved_views
           set filter = $1::jsonb
           where name = 'Santé'
             and (filter is null or filter = '{}'::jsonb)"#,
    )
    .bind(&filter)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn ensure_seeded(pool: &PgPool) -> anyhow::Result<()> {
    let count: i32 = sqlx::query_scalar("select count(*)::int from documents")
        .fetch_one(pool)
        .await?;
    if count > 0 {
        repair_saved_views(pool).await?;
        return Ok(());
    }
    tracing::info!("seeding paperrust catalog and sample documents");

    let tags: &[(&str, &str, bool, &str, &str)] = &[
        ("Inbox", "#9a4a28", true, "", "none"),
        ("Finance", "#3d5a4c", false, "CHF|facture|total TTC|IBAN", "regex"),
        ("Santé", "#5b4e6a", false, "cabinet ordonnance TARMED médecin", "any"),
        ("Logement", "#4a5a6a", false, "loyer bail régie appartement charges", "any"),
        ("Impôts", "#6a5340", false, "fiscal impôt IFD attestation fiscale", "any"),
        ("Garantie", "#4a5e3f", false, "garantie série notice SAV", "any"),
        ("2025", "#57534e", false, "2025", "exact"),
        ("2026", "#57534e", false, "2026", "exact"),
    ];
    for (name, color, inbox, match_t, algo) in tags {
        sqlx::query(
            "insert into tags (name, color, is_inbox_tag, match, matching_algorithm, owner_id) values ($1,$2,$3,$4,$5,1) on conflict do nothing",
        )
        .bind(name)
        .bind(color)
        .bind(inbox)
        .bind(match_t)
        .bind(algo)
        .execute(pool)
        .await?;
    }

    let correspondents: &[(&str, &str, &str)] = &[
        ("Réseau Alpin SA", "réseau alpin kWh électricité", "all"),
        ("Cime Assurance", "cime assurance police", "any"),
        ("Banque des Crêtes", "banque des crêtes relevé IBAN", "any"),
        ("Régie du Vallon", "régie du vallon loyer bail quittance", "any"),
        ("Cabinet du Léman", "cabinet du léman TARMED ordonnance", "any"),
        ("Alpenet SA", "alpenet fibre", "any"),
        ("Commune de Valombre", "valombre fiscal impôt", "any"),
        ("Atelier Nordlicht", "nordlicht pyro garantie", "any"),
        ("Étude Perret & Associés", "perret succession", "any"),
        ("Garage des Cèdres", "garage des cèdres révision", "any"),
        ("Café des Archives", "café des archives espresso", "any"),
    ];
    for (name, match_t, algo) in correspondents {
        sqlx::query(
            "insert into correspondents (name, match, matching_algorithm, owner_id) values ($1,$2,$3,1) on conflict do nothing",
        )
        .bind(name)
        .bind(match_t)
        .bind(algo)
        .execute(pool)
        .await?;
    }

    let types: &[(&str, &str, &str)] = &[
        ("Facture", "facture total TTC échéance", "any"),
        ("Quittance", "quittance encaissé ticket", "any"),
        ("Contrat", "contrat bail durée", "any"),
        ("Relevé", "relevé solde compte", "any"),
        ("Courrier", "courrier nous vous informons", "any"),
        ("Attestation", "attestation police valable", "any"),
        ("Ordonnance", "ordonnance renouvelable µg", "any"),
        ("Garantie", "garantie série notice", "any"),
    ];
    for (name, match_t, algo) in types {
        sqlx::query(
            "insert into document_types (name, match, matching_algorithm, owner_id) values ($1,$2,$3,1) on conflict do nothing",
        )
        .bind(name)
        .bind(match_t)
        .bind(algo)
        .execute(pool)
        .await?;
    }

    let paths: &[(&str, &str)] = &[
        ("Énergie/{created_year}", "{created_year}/{correspondent}/{title}"),
        ("Assurances/{created_year}", "Assurances/{created_year}/{title}"),
        ("Banque/{created_year}", "Banque/{created_year}/{title}"),
        ("Logement/Contrats", "Logement/Contrats/{title}"),
        ("Logement/{created_year}", "Logement/{created_year}/{title}"),
        ("Santé/{created_year}", "Santé/{created_year}/{title}"),
        ("Impôts/{created_year}", "Impôts/{created_year}/{title}"),
        ("Garanties", "Garanties/{title}"),
        ("Courrier/{created_year}", "Courrier/{created_year}/{title}"),
        ("Véhicule/{created_year}", "Véhicule/{created_year}/{title}"),
        ("Divers/{created_year}", "Divers/{created_year}/{title}"),
    ];
    for (name, path) in paths {
        sqlx::query("insert into storage_paths (name, path, owner_id) values ($1,$2,1) on conflict do nothing")
            .bind(name)
            .bind(path)
            .execute(pool)
            .await?;
    }

    sqlx::query(
        r#"insert into custom_fields (name, data_type, owner_id) values
      ('Montant','monetary',1),
      ('Monnaie','string',1),
      ('N° de référence','string',1),
      ('Payé','boolean',1)
     on conflict do nothing"#,
    )
    .execute(pool)
    .await?;

    let tag_rows = sqlx::query("select id, name from tags where owner_id = 1").fetch_all(pool).await?;
    let corr_rows = sqlx::query("select id, name from correspondents where owner_id = 1").fetch_all(pool).await?;
    let type_rows = sqlx::query("select id, name from document_types where owner_id = 1").fetch_all(pool).await?;
    let path_rows = sqlx::query("select id, name from storage_paths where owner_id = 1").fetch_all(pool).await?;
    let field_rows = sqlx::query("select id, name from custom_fields where owner_id = 1").fetch_all(pool).await?;
    let tag_id: std::collections::HashMap<String, i32> = tag_rows
        .iter()
        .map(|r| (r.get::<String, _>("name"), r.get("id")))
        .collect();
    let corr_id: std::collections::HashMap<String, i32> = corr_rows
        .iter()
        .map(|r| (r.get::<String, _>("name"), r.get("id")))
        .collect();
    let type_id: std::collections::HashMap<String, i32> = type_rows
        .iter()
        .map(|r| (r.get::<String, _>("name"), r.get("id")))
        .collect();
    let path_id: std::collections::HashMap<String, i32> = path_rows
        .iter()
        .map(|r| (r.get::<String, _>("name"), r.get("id")))
        .collect();
    let field_id: std::collections::HashMap<String, i32> = field_rows
        .iter()
        .map(|r| (r.get::<String, _>("name"), r.get("id")))
        .collect();

    let docs: Vec<SeedDoc> = serde_json::from_str(include_str!("../seed_docs.json"))?;
    for doc in docs {
        let footer = format!("ASN {}  ·  {}", doc.asn, doc.filename);
        let svg = render_paper_svg(PaperSpec {
            kicker: Some(&doc.type_),
            title: &doc.title,
            correspondent: &doc.correspondent,
            date: &doc.date,
            r#ref: doc.r#ref.as_deref(),
            lines: &doc.lines,
            footer: Some(&footer),
            stamp: doc.stamp.as_deref(),
            accent: None,
        });
        let inserted: Option<i32> = sqlx::query_scalar(
            r#"insert into documents (
        title, content, correspondent_id, document_type_id, storage_path_id,
        created_date, added_at, modified_at, asn, checksum, mime_type,
        original_filename, page_count, language, ocr_status, ocr_provider,
        thumbnail_svg, file_data, file_size, summary, owner_id
      ) values (
        $1,$2,$3,$4,$5,$6::date,$7::timestamptz,$7::timestamptz,$8,$9,'image/svg+xml',$10,1,$11,'done','xai',$12,$12,$13,$14,1
      ) on conflict do nothing returning id"#,
        )
        .bind(&doc.title)
        .bind(&doc.content)
        .bind(corr_id.get(&doc.correspondent).copied())
        .bind(type_id.get(&doc.type_).copied())
        .bind(path_id.get(&doc.path).copied())
        .bind(&doc.date)
        .bind(&doc.added)
        .bind(doc.asn)
        .bind(format!("seed-{}", doc.asn))
        .bind(&doc.filename)
        .bind(&doc.language)
        .bind(&svg)
        .bind(svg.len() as i32)
        .bind(&doc.summary)
        .fetch_optional(pool)
        .await?;
        let id = if let Some(id) = inserted {
            id
        } else {
            sqlx::query_scalar("select id from documents where asn = $1")
                .bind(doc.asn)
                .fetch_one(pool)
                .await?
        };
        for tag in &doc.tags {
            if let Some(tid) = tag_id.get(tag) {
                sqlx::query("insert into document_tags (document_id, tag_id) values ($1,$2) on conflict do nothing")
                    .bind(id)
                    .bind(*tid)
                    .execute(pool)
                    .await?;
            }
        }
        for (name, value) in &doc.custom {
            if let Some(fid) = field_id.get(name) {
                sqlx::query(
                    "insert into document_custom_values (document_id, field_id, value) values ($1,$2,$3) on conflict do nothing",
                )
                .bind(id)
                .bind(*fid)
                .bind(value)
                .execute(pool)
                .await?;
            }
        }
        sqlx::query(
            "insert into audit_log (document_id, action, detail, created_at) values ($1,'consumed',$2,$3::timestamptz)",
        )
        .bind(id)
        .bind(format!("Import initial · OCR xAI · {}", doc.filename))
        .bind(&doc.added)
        .execute(pool)
        .await?;
        sqlx::query("insert into notes (document_id, body, created_at, owner_id) values ($1,$2,$3::timestamptz,1)")
            .bind(id)
            .bind("Classé automatiquement via le moteur de règles Paperrust.")
            .bind(&doc.added)
            .execute(pool)
            .await?;
    }

    let sante_filter = serde_json::json!({
        "tagIds": tag_id.get("Santé").copied().into_iter().collect::<Vec<i32>>(),
    })
    .to_string();
    sqlx::query(
        r#"insert into saved_views (name, show_on_dashboard, show_in_sidebar, sort_field, sort_reverse, filter, owner_id)
     values
      ('Boîte de réception', true, true, 'added_at', true, $1::jsonb, 1),
      ('Finances 2026', true, true, 'created_date', true, $2::jsonb, 1),
      ('Santé', false, true, 'created_date', true, $3::jsonb, 1)"#,
    )
    .bind(r#"{"inboxOnly":true}"#)
    .bind(r#"{"tagIds":[],"q":"CHF"}"#)
    .bind(&sante_filter)
    .execute(pool)
    .await?;

    let finance_tag = tag_id.get("Finance").copied();
    let inbox_tag = tag_id.get("Inbox").copied();
    sqlx::query(
        r#"insert into workflows (name, enabled, trigger_type, filters, actions, owner_id)
     values
      ('Retirer Inbox si classé', true, 'document_updated',
        '{"hasCorrespondent":true}',
        $1::jsonb, 1),
      ('Marquer les factures Finance', true, 'document_added',
        '{"contentContains":"Total TTC"}',
        $2::jsonb, 1)"#,
    )
    .bind(serde_json::json!([{"type":"remove_tag","tagId": inbox_tag}]).to_string())
    .bind(serde_json::json!([{"type":"add_tag","tagId": finance_tag}]).to_string())
    .execute(pool)
    .await?;

    sqlx::query(
        "insert into mail_accounts (name, imap_server, username, imap_port, owner_id) values ('Boîte personnelle', 'mail.proton.me', 'archive@paperrust.example', 993, 1)",
    )
    .execute(pool)
    .await?;
    let account_id: i32 = sqlx::query_scalar("select id from mail_accounts limit 1")
        .fetch_one(pool)
        .await?;
    sqlx::query(
        r#"insert into mail_rules (name, account_id, folder, filter_from, filter_subject, enabled, correspondent_id, document_type_id)
     values
      ('Factures énergie', $1, 'INBOX', 'factures@reseau-alpin.example', 'facture', true, $2, $3),
      ('Régie', $1, 'INBOX', '@regie-du-vallon.example', '', true, $4, $5)"#,
    )
    .bind(account_id)
    .bind(corr_id.get("Réseau Alpin SA").copied())
    .bind(type_id.get("Facture").copied())
    .bind(corr_id.get("Régie du Vallon").copied())
    .bind(type_id.get("Quittance").copied())
    .execute(pool)
    .await?;

    let id_1042: i32 = sqlx::query_scalar("select id from documents where asn = 1042")
        .fetch_one(pool)
        .await?;
    let id_1109: i32 = sqlx::query_scalar("select id from documents where asn = 1109")
        .fetch_one(pool)
        .await?;
    sqlx::query(
        r#"insert into consume_jobs (filename, status, message, document_id, created_at, finished_at, owner_id)
     values
      ('reseau-alpin-2026-01.pdf','success','OCR xAI · 1 page',$1,'2026-01-14T09:12:00Z','2026-01-14T09:12:08Z',1),
      ('scan-notice-pyro.pdf','success','OCR xAI · classé Inbox',$2,'2026-09-02T21:04:00Z','2026-09-02T21:04:09Z',1)"#,
    )
    .bind(id_1042)
    .bind(id_1109)
    .execute(pool)
    .await?;

    sqlx::query(
        r#"insert into app_settings (key, value) values
      ('ocr_provider', '"xai"'),
      ('ocr_language', '"fr"'),
      ('auto_match', 'true')
     on conflict (key) do nothing"#,
    )
    .execute(pool)
    .await?;
    repair_saved_views(pool).await?;
    Ok(())
}
