use crate::db::DbPool;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, PartialEq, Eq)]
pub struct Label {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub color: String,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct EmailFilter {
    pub id: Uuid,
    pub user_id: Uuid,
    pub keyword: String,
    pub label_id: Uuid,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct EmailFilterWithLabel {
    pub id: Uuid,
    pub user_id: Uuid,
    pub keyword: String,
    pub label_id: Uuid,
    pub label_name: String,
    pub label_color: String,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct EmailLabelRow {
    email_id: Uuid,
    id: Uuid,
    user_id: Uuid,
    name: String,
    color: String,
    created_at: OffsetDateTime,
}

pub async fn get_labels_by_user(pool: &DbPool, user_id: Uuid) -> Result<Vec<Label>, sqlx::Error> {
    match pool {
        DbPool::Postgres(p) => {
            sqlx::query_as::<_, Label>(
                "SELECT id, user_id, name, color, created_at FROM labels WHERE user_id = $1 ORDER BY name ASC",
            )
            .bind(user_id)
            .fetch_all(p)
            .await
        }
        DbPool::Sqlite(p) => {
            sqlx::query_as::<_, Label>(
                "SELECT id, user_id, name, color, created_at FROM labels WHERE user_id = ? ORDER BY name ASC",
            )
            .bind(user_id)
            .fetch_all(p)
            .await
        }
    }
}

pub async fn get_label_by_id(
    pool: &DbPool,
    label_id: Uuid,
    user_id: Uuid,
) -> Result<Option<Label>, sqlx::Error> {
    match pool {
        DbPool::Postgres(p) => {
            sqlx::query_as::<_, Label>(
                "SELECT id, user_id, name, color, created_at FROM labels WHERE id = $1 AND user_id = $2",
            )
            .bind(label_id)
            .bind(user_id)
            .fetch_optional(p)
            .await
        }
        DbPool::Sqlite(p) => {
            sqlx::query_as::<_, Label>(
                "SELECT id, user_id, name, color, created_at FROM labels WHERE id = ? AND user_id = ?",
            )
            .bind(label_id)
            .bind(user_id)
            .fetch_optional(p)
            .await
        }
    }
}

pub async fn get_label_by_name(
    pool: &DbPool,
    user_id: Uuid,
    name: &str,
) -> Result<Option<Label>, sqlx::Error> {
    match pool {
        DbPool::Postgres(p) => {
            sqlx::query_as::<_, Label>(
                "SELECT id, user_id, name, color, created_at FROM labels WHERE user_id = $1 AND LOWER(name) = LOWER($2)",
            )
            .bind(user_id)
            .bind(name)
            .fetch_optional(p)
            .await
        }
        DbPool::Sqlite(p) => {
            sqlx::query_as::<_, Label>(
                "SELECT id, user_id, name, color, created_at FROM labels WHERE user_id = ? AND LOWER(name) = LOWER(?)",
            )
            .bind(user_id)
            .bind(name)
            .fetch_optional(p)
            .await
        }
    }
}

pub async fn insert_label(
    pool: &DbPool,
    user_id: Uuid,
    name: &str,
    color: &str,
) -> Result<Label, sqlx::Error> {
    let id = Uuid::new_v4();
    match pool {
        DbPool::Postgres(p) => {
            sqlx::query_as::<_, Label>(
                "INSERT INTO labels (id, user_id, name, color) VALUES ($1, $2, $3, $4) RETURNING id, user_id, name, color, created_at",
            )
            .bind(id)
            .bind(user_id)
            .bind(name)
            .bind(color)
            .fetch_one(p)
            .await
        }
        DbPool::Sqlite(p) => {
            sqlx::query(
                "INSERT INTO labels (id, user_id, name, color) VALUES (?, ?, ?, ?)",
            )
            .bind(id)
            .bind(user_id)
            .bind(name)
            .bind(color)
            .execute(p)
            .await?;

            sqlx::query_as::<_, Label>(
                "SELECT id, user_id, name, color, created_at FROM labels WHERE id = ?",
            )
            .bind(id)
            .fetch_one(p)
            .await
        }
    }
}

pub async fn update_label(
    pool: &DbPool,
    label_id: Uuid,
    user_id: Uuid,
    name: &str,
    color: &str,
) -> Result<bool, sqlx::Error> {
    match pool {
        DbPool::Postgres(p) => {
            let res = sqlx::query(
                "UPDATE labels SET name = $1, color = $2 WHERE id = $3 AND user_id = $4",
            )
            .bind(name)
            .bind(color)
            .bind(label_id)
            .bind(user_id)
            .execute(p)
            .await?;
            Ok(res.rows_affected() > 0)
        }
        DbPool::Sqlite(p) => {
            let res =
                sqlx::query("UPDATE labels SET name = ?, color = ? WHERE id = ? AND user_id = ?")
                    .bind(name)
                    .bind(color)
                    .bind(label_id)
                    .bind(user_id)
                    .execute(p)
                    .await?;
            Ok(res.rows_affected() > 0)
        }
    }
}

pub async fn delete_label(
    pool: &DbPool,
    label_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    match pool {
        DbPool::Postgres(p) => {
            let res = sqlx::query("DELETE FROM labels WHERE id = $1 AND user_id = $2")
                .bind(label_id)
                .bind(user_id)
                .execute(p)
                .await?;
            Ok(res.rows_affected() > 0)
        }
        DbPool::Sqlite(p) => {
            let res = sqlx::query("DELETE FROM labels WHERE id = ? AND user_id = ?")
                .bind(label_id)
                .bind(user_id)
                .execute(p)
                .await?;
            Ok(res.rows_affected() > 0)
        }
    }
}

pub async fn get_filters_by_user(
    pool: &DbPool,
    user_id: Uuid,
) -> Result<Vec<EmailFilterWithLabel>, sqlx::Error> {
    match pool {
        DbPool::Postgres(p) => {
            sqlx::query_as::<_, EmailFilterWithLabel>(
                r#"SELECT ef.id, ef.user_id, ef.keyword, ef.label_id, l.name as label_name, l.color as label_color, ef.created_at
                   FROM email_filters ef
                   JOIN labels l ON ef.label_id = l.id
                   WHERE ef.user_id = $1
                   ORDER BY ef.created_at DESC"#,
            )
            .bind(user_id)
            .fetch_all(p)
            .await
        }
        DbPool::Sqlite(p) => {
            sqlx::query_as::<_, EmailFilterWithLabel>(
                r#"SELECT ef.id, ef.user_id, ef.keyword, ef.label_id, l.name as label_name, l.color as label_color, ef.created_at
                   FROM email_filters ef
                   JOIN labels l ON ef.label_id = l.id
                   WHERE ef.user_id = ?
                   ORDER BY ef.created_at DESC"#,
            )
            .bind(user_id)
            .fetch_all(p)
            .await
        }
    }
}

pub async fn insert_filter(
    pool: &DbPool,
    user_id: Uuid,
    keyword: &str,
    label_id: Uuid,
) -> Result<EmailFilter, sqlx::Error> {
    let id = Uuid::new_v4();
    match pool {
        DbPool::Postgres(p) => {
            sqlx::query_as::<_, EmailFilter>(
                "INSERT INTO email_filters (id, user_id, keyword, label_id) VALUES ($1, $2, $3, $4) RETURNING id, user_id, keyword, label_id, created_at",
            )
            .bind(id)
            .bind(user_id)
            .bind(keyword)
            .bind(label_id)
            .fetch_one(p)
            .await
        }
        DbPool::Sqlite(p) => {
            sqlx::query(
                "INSERT INTO email_filters (id, user_id, keyword, label_id) VALUES (?, ?, ?, ?)",
            )
            .bind(id)
            .bind(user_id)
            .bind(keyword)
            .bind(label_id)
            .execute(p)
            .await?;

            sqlx::query_as::<_, EmailFilter>(
                "SELECT id, user_id, keyword, label_id, created_at FROM email_filters WHERE id = ?",
            )
            .bind(id)
            .fetch_one(p)
            .await
        }
    }
}

pub async fn delete_filter(
    pool: &DbPool,
    filter_id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    match pool {
        DbPool::Postgres(p) => {
            let res = sqlx::query("DELETE FROM email_filters WHERE id = $1 AND user_id = $2")
                .bind(filter_id)
                .bind(user_id)
                .execute(p)
                .await?;
            Ok(res.rows_affected() > 0)
        }
        DbPool::Sqlite(p) => {
            let res = sqlx::query("DELETE FROM email_filters WHERE id = ? AND user_id = ?")
                .bind(filter_id)
                .bind(user_id)
                .execute(p)
                .await?;
            Ok(res.rows_affected() > 0)
        }
    }
}

pub async fn assign_labels_to_email(
    pool: &DbPool,
    email_id: Uuid,
    label_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    if label_ids.is_empty() {
        return Ok(());
    }

    match pool {
        DbPool::Postgres(p) => {
            for &label_id in label_ids {
                sqlx::query(
                    "INSERT INTO email_labels (email_id, label_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                )
                .bind(email_id)
                .bind(label_id)
                .execute(p)
                .await?;
            }
            Ok(())
        }
        DbPool::Sqlite(p) => {
            for &label_id in label_ids {
                sqlx::query(
                    "INSERT OR IGNORE INTO email_labels (email_id, label_id) VALUES (?, ?)",
                )
                .bind(email_id)
                .bind(label_id)
                .execute(p)
                .await?;
            }
            Ok(())
        }
    }
}

pub async fn remove_label_from_email(
    pool: &DbPool,
    email_id: Uuid,
    label_id: Uuid,
) -> Result<bool, sqlx::Error> {
    match pool {
        DbPool::Postgres(p) => {
            let res = sqlx::query("DELETE FROM email_labels WHERE email_id = $1 AND label_id = $2")
                .bind(email_id)
                .bind(label_id)
                .execute(p)
                .await?;
            Ok(res.rows_affected() > 0)
        }
        DbPool::Sqlite(p) => {
            let res = sqlx::query("DELETE FROM email_labels WHERE email_id = ? AND label_id = ?")
                .bind(email_id)
                .bind(label_id)
                .execute(p)
                .await?;
            Ok(res.rows_affected() > 0)
        }
    }
}

pub async fn get_labels_for_email(
    pool: &DbPool,
    email_id: Uuid,
) -> Result<Vec<Label>, sqlx::Error> {
    match pool {
        DbPool::Postgres(p) => {
            sqlx::query_as::<_, Label>(
                r#"SELECT l.id, l.user_id, l.name, l.color, l.created_at
                   FROM email_labels el
                   JOIN labels l ON el.label_id = l.id
                   WHERE el.email_id = $1
                   ORDER BY l.name ASC"#,
            )
            .bind(email_id)
            .fetch_all(p)
            .await
        }
        DbPool::Sqlite(p) => {
            sqlx::query_as::<_, Label>(
                r#"SELECT l.id, l.user_id, l.name, l.color, l.created_at
                   FROM email_labels el
                   JOIN labels l ON el.label_id = l.id
                   WHERE el.email_id = ?
                   ORDER BY l.name ASC"#,
            )
            .bind(email_id)
            .fetch_all(p)
            .await
        }
    }
}

pub async fn get_labels_for_emails(
    pool: &DbPool,
    email_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<Label>>, sqlx::Error> {
    if email_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows: Vec<EmailLabelRow> = match pool {
        DbPool::Postgres(p) => {
            sqlx::query_as::<_, EmailLabelRow>(
                r#"SELECT el.email_id, l.id, l.user_id, l.name, l.color, l.created_at
                   FROM email_labels el
                   JOIN labels l ON el.label_id = l.id
                   WHERE el.email_id = ANY($1)
                   ORDER BY l.name ASC"#,
            )
            .bind(email_ids)
            .fetch_all(p)
            .await?
        }
        DbPool::Sqlite(p) => {
            let placeholders = vec!["?"; email_ids.len()].join(", ");
            let sql = format!(
                r#"SELECT el.email_id, l.id, l.user_id, l.name, l.color, l.created_at
                   FROM email_labels el
                   JOIN labels l ON el.label_id = l.id
                   WHERE el.email_id IN ({})
                   ORDER BY l.name ASC"#,
                placeholders
            );
            let mut query = sqlx::query_as::<sqlx::Sqlite, EmailLabelRow>(&sql);
            for id in email_ids {
                query = query.bind(id);
            }
            query.fetch_all(p).await?
        }
    };

    let mut map: HashMap<Uuid, Vec<Label>> = HashMap::new();
    for row in rows {
        map.entry(row.email_id).or_default().push(Label {
            id: row.id,
            user_id: row.user_id,
            name: row.name,
            color: row.color,
            created_at: row.created_at,
        });
    }

    Ok(map)
}

pub async fn get_filters_for_engine(
    pool: &DbPool,
    user_id: Uuid,
) -> Result<Vec<(String, Uuid)>, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct FilterPair {
        keyword: String,
        label_id: Uuid,
    }

    let pairs: Vec<FilterPair> = match pool {
        DbPool::Postgres(p) => {
            sqlx::query_as::<_, FilterPair>(
                "SELECT keyword, label_id FROM email_filters WHERE user_id = $1",
            )
            .bind(user_id)
            .fetch_all(p)
            .await?
        }
        DbPool::Sqlite(p) => {
            sqlx::query_as::<_, FilterPair>(
                "SELECT keyword, label_id FROM email_filters WHERE user_id = ?",
            )
            .bind(user_id)
            .fetch_all(p)
            .await?
        }
    };

    Ok(pairs.into_iter().map(|p| (p.keyword, p.label_id)).collect())
}

pub async fn get_unlabeled_emails_for_user(
    pool: &DbPool,
    user_id: Uuid,
    label_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<(Uuid, Uuid)>, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct EmailKey {
        id: Uuid,
        body_key: Uuid,
    }

    let rows: Vec<EmailKey> = match pool {
        DbPool::Postgres(p) => {
            sqlx::query_as::<_, EmailKey>(
                r#"SELECT e.id, e.body_key
                   FROM received_emails e
                   JOIN aliases a ON e.alias_id = a.id
                   WHERE a.user_id = $1
                     AND NOT EXISTS (
                         SELECT 1 FROM email_labels el
                         WHERE el.email_id = e.id AND el.label_id = $2
                     )
                   ORDER BY e.received_at DESC
                   LIMIT $3 OFFSET $4"#,
            )
            .bind(user_id)
            .bind(label_id)
            .bind(limit)
            .bind(offset)
            .fetch_all(p)
            .await?
        }
        DbPool::Sqlite(p) => {
            sqlx::query_as::<sqlx::Sqlite, EmailKey>(
                r#"SELECT e.id, e.body_key
                   FROM received_emails e
                   JOIN aliases a ON e.alias_id = a.id
                   WHERE a.user_id = ?
                     AND NOT EXISTS (
                         SELECT 1 FROM email_labels el
                         WHERE el.email_id = e.id AND el.label_id = ?
                     )
                   ORDER BY e.received_at DESC
                   LIMIT ? OFFSET ?"#,
            )
            .bind(user_id)
            .bind(label_id)
            .bind(limit)
            .bind(offset)
            .fetch_all(p)
            .await?
        }
    };

    Ok(rows.into_iter().map(|r| (r.id, r.body_key)).collect())
}

pub async fn batch_assign_label_to_emails(
    pool: &DbPool,
    email_ids: &[Uuid],
    label_id: Uuid,
) -> Result<(), sqlx::Error> {
    if email_ids.is_empty() {
        return Ok(());
    }

    match pool {
        DbPool::Postgres(p) => {
            for &email_id in email_ids {
                sqlx::query(
                    "INSERT INTO email_labels (email_id, label_id) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                )
                .bind(email_id)
                .bind(label_id)
                .execute(p)
                .await?;
            }
            Ok(())
        }
        DbPool::Sqlite(p) => {
            for &email_id in email_ids {
                sqlx::query(
                    "INSERT OR IGNORE INTO email_labels (email_id, label_id) VALUES (?, ?)",
                )
                .bind(email_id)
                .bind(label_id)
                .execute(p)
                .await?;
            }
            Ok(())
        }
    }
}
