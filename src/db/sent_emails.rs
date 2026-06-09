use crate::db::DbPool;
use sqlx::Type;
use time::OffsetDateTime;
use uuid::Uuid;

use std::fmt;

#[derive(Type, Debug, Clone, PartialEq, serde::Serialize)]
#[sqlx(type_name = "email_status", rename_all = "lowercase")]
pub enum EmailStatus {
    Draft,
    Sending,
    Sent,
    Failed,
}

impl fmt::Display for EmailStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            EmailStatus::Draft => "Draft",
            EmailStatus::Sending => "Sending",
            EmailStatus::Sent => "Sent",
            EmailStatus::Failed => "Failed",
        };
        write!(f, "{}", s)
    }
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct SentEmail {
    pub id: Uuid,
    pub user_id: Uuid,
    pub from_alias_id: Uuid,
    pub to_address: String,
    pub cc_addresses: Option<String>,
    pub bcc_addresses: Option<String>,
    pub subject: String,
    pub body_key: Uuid,
    pub status: EmailStatus,
    pub error_message: Option<String>,
    pub message_id: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub sent_at: Option<OffsetDateTime>,
}

pub async fn insert_sent_email(
    pool: &DbPool,
    user_id: Uuid,
    from_alias_id: Uuid,
    to_address: &str,
    subject: &str,
    body_key: Uuid,
    status: EmailStatus,
    error_message: Option<String>,
) -> Result<SentEmail, sqlx::Error> {
    let now = OffsetDateTime::now_utc();
    let sent_at = if status == EmailStatus::Sent {
        Some(now)
    } else {
        None
    };

    match pool {
        DbPool::Postgres(pool) => {
            let email = sqlx::query_as::<_, SentEmail>(
                r#"
                INSERT INTO sent_emails (
                    id, user_id, from_alias_id, to_address, subject, body_key, 
                    status, error_message, created_at, updated_at, sent_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7::email_status, $8, $9, $9, $10)
                RETURNING 
                    id, user_id, from_alias_id, to_address, cc_addresses, bcc_addresses,
                    subject, body_key, status, 
                    error_message, message_id, created_at, updated_at, sent_at
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(user_id)
            .bind(from_alias_id)
            .bind(to_address)
            .bind(subject)
            .bind(body_key)
            .bind(status as EmailStatus)
            .bind(error_message)
            .bind(now)
            .bind(sent_at)
            .fetch_one(pool)
            .await?;
            Ok(email)
        }
        DbPool::Sqlite(pool) => {
            let id = Uuid::new_v4();
            let email = sqlx::query_as::<sqlx::Sqlite, SentEmail>(
                r#"
                INSERT INTO sent_emails (
                    id, user_id, from_alias_id, to_address, subject, body_key, 
                    status, error_message, created_at, updated_at, sent_at
                )
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                RETURNING 
                    id, user_id, from_alias_id, to_address, cc_addresses, bcc_addresses,
                    subject, body_key, status, 
                    error_message, message_id, created_at, updated_at, sent_at
                "#,
            )
            .bind(id)
            .bind(user_id)
            .bind(from_alias_id)
            .bind(to_address)
            .bind(subject)
            .bind(body_key)
            .bind(status)
            .bind(error_message)
            .bind(now) // created_at
            .bind(now) // updated_at
            .bind(sent_at) // sent_at
            .fetch_one(pool)
            .await?;
            Ok(email)
        }
    }
}

pub async fn mark_sent_email_success(
    pool: &DbPool,
    id: Uuid,
    message_id: &str,
) -> Result<(), sqlx::Error> {
    let now = OffsetDateTime::now_utc();
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query(
                r#"
                UPDATE sent_emails
                SET status = 'sent'::email_status, message_id = $1, sent_at = $2, updated_at = $3
                WHERE id = $4
                "#,
            )
            .bind(message_id)
            .bind(now)
            .bind(now)
            .bind(id)
            .execute(pool)
            .await?;
        }
        DbPool::Sqlite(pool) => {
            sqlx::query(
                r#"
                UPDATE sent_emails
                SET status = 'sent', message_id = ?, sent_at = ?, updated_at = ?
                WHERE id = ?
                "#,
            )
            .bind(message_id)
            .bind(now)
            .bind(now)
            .bind(id)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

pub async fn mark_sent_email_failed(
    pool: &DbPool,
    id: Uuid,
    error_msg: &str,
) -> Result<(), sqlx::Error> {
    let now = OffsetDateTime::now_utc();
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query(
                r#"
                UPDATE sent_emails
                SET status = 'failed'::email_status, error_message = $1, updated_at = $2
                WHERE id = $3
                "#,
            )
            .bind(error_msg)
            .bind(now)
            .bind(id)
            .execute(pool)
            .await?;
        }
        DbPool::Sqlite(pool) => {
            sqlx::query(
                r#"
                UPDATE sent_emails
                SET status = 'failed', error_message = ?, updated_at = ?
                WHERE id = ?
                "#,
            )
            .bind(error_msg)
            .bind(now)
            .bind(id)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct SentEmailRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub from_alias_id: Uuid,
    pub to_address: String,
    pub cc_addresses: Option<String>,
    pub bcc_addresses: Option<String>,
    pub subject: String,
    pub body_key: Uuid,
    pub status: EmailStatus,
    pub error_message: Option<String>,
    pub message_id: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub sent_at: Option<OffsetDateTime>,
    pub alias_address: String,
}

pub async fn get_sent_emails_by_user_id(
    pool: &DbPool,
    user_id: uuid::Uuid,
    status: EmailStatus,
    limit: i64,
    offset: i64,
    alias_filter: Option<String>,
    query_filter: Option<String>,
) -> Result<Vec<SentEmailRow>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let mut query = String::from(
                "SELECT s.*, a.subdomain || '@' || d.name as alias_address 
                 FROM sent_emails s
                 JOIN aliases a ON s.from_alias_id = a.id
                 JOIN domains d ON a.domain_id = d.id
                 WHERE s.user_id = $1 AND s.status = $2::email_status"
            );

            if alias_filter.is_some() {
                query.push_str(" AND (a.subdomain || '@' || d.name) = $3");
            }

            if query_filter.is_some() {
                if alias_filter.is_some() {
                    query.push_str(" AND (s.to_address ILIKE $4 OR s.subject ILIKE $4)");
                } else {
                    query.push_str(" AND (s.to_address ILIKE $3 OR s.subject ILIKE $3)");
                }
            }

            let bind_idx_limit = if alias_filter.is_some() && query_filter.is_some() { "5" } else if alias_filter.is_some() || query_filter.is_some() { "4" } else { "3" };
            query.push_str(&format!(" ORDER BY s.created_at DESC LIMIT ${}", bind_idx_limit));
            
            let bind_idx_offset = if alias_filter.is_some() && query_filter.is_some() { "6" } else if alias_filter.is_some() || query_filter.is_some() { "5" } else { "4" };
            query.push_str(&format!(" OFFSET ${}", bind_idx_offset));

            let mut q = sqlx::query_as::<_, SentEmailRow>(&query).bind(user_id).bind(status);

            if let Some(alias) = &alias_filter {
                q = q.bind(alias);
            }
            
            if let Some(search) = &query_filter {
                let pattern = format!("%{}%", search);
                q = q.bind(pattern);
            }

            q = q.bind(limit).bind(offset);

            q.fetch_all(pool).await
        }
        DbPool::Sqlite(pool) => {
            let mut query = String::from(
                "SELECT s.*, a.subdomain || '@' || d.name as alias_address 
                 FROM sent_emails s
                 JOIN aliases a ON s.from_alias_id = a.id
                 JOIN domains d ON a.domain_id = d.id
                 WHERE s.user_id = ? AND s.status = ?"
            );

            if alias_filter.is_some() {
                query.push_str(" AND (a.subdomain || '@' || d.name) = ?");
            }

            if query_filter.is_some() {
                query.push_str(" AND (s.to_address LIKE ? OR s.subject LIKE ?)");
            }

            query.push_str(" ORDER BY s.created_at DESC LIMIT ? OFFSET ?");

            let mut q = sqlx::query_as::<sqlx::Sqlite, SentEmailRow>(&query).bind(user_id).bind(status);

            if let Some(alias) = &alias_filter {
                q = q.bind(alias);
            }
            
            if let Some(search) = &query_filter {
                let pattern = format!("%{}%", search);
                q = q.bind(pattern.clone()).bind(pattern);
            }

            q = q.bind(limit).bind(offset);

            q.fetch_all(pool).await
        }
    }
}

pub async fn get_sent_email_count_by_user_id(
    pool: &DbPool,
    user_id: uuid::Uuid,
    status: EmailStatus,
    alias_filter: Option<String>,
    query_filter: Option<String>,
) -> Result<i64, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let mut query = String::from(
                "SELECT COUNT(*) FROM sent_emails s
                 JOIN aliases a ON s.from_alias_id = a.id
                 JOIN domains d ON a.domain_id = d.id
                 WHERE s.user_id = $1 AND s.status = $2::email_status"
            );

            if alias_filter.is_some() {
                query.push_str(" AND (a.subdomain || '@' || d.name) = $3");
            }

            if query_filter.is_some() {
                if alias_filter.is_some() {
                    query.push_str(" AND (s.to_address ILIKE $4 OR s.subject ILIKE $4)");
                } else {
                    query.push_str(" AND (s.to_address ILIKE $3 OR s.subject ILIKE $3)");
                }
            }

            let mut q = sqlx::query_scalar::<_, Option<i64>>(&query).bind(user_id).bind(status);

            if let Some(alias) = &alias_filter {
                q = q.bind(alias);
            }
            
            if let Some(search) = &query_filter {
                let pattern = format!("%{}%", search);
                q = q.bind(pattern);
            }

            q.fetch_one(pool).await.map(|count| count.unwrap_or(0))
        }
        DbPool::Sqlite(pool) => {
            let mut query = String::from(
                "SELECT COUNT(*) FROM sent_emails s
                 JOIN aliases a ON s.from_alias_id = a.id
                 JOIN domains d ON a.domain_id = d.id
                 WHERE s.user_id = ? AND s.status = ?"
            );

            if alias_filter.is_some() {
                query.push_str(" AND (a.subdomain || '@' || d.name) = ?");
            }

            if query_filter.is_some() {
                query.push_str(" AND (s.to_address LIKE ? OR s.subject LIKE ?)");
            }

            let mut q = sqlx::query_scalar::<sqlx::Sqlite, Option<i64>>(&query).bind(user_id).bind(status);

            if let Some(alias) = &alias_filter {
                q = q.bind(alias);
            }
            
            if let Some(search) = &query_filter {
                let pattern = format!("%{}%", search);
                q = q.bind(pattern.clone()).bind(pattern);
            }

            q.fetch_one(pool).await.map(|count| count.unwrap_or(0))
        }
    }
}

pub async fn upsert_draft(
    pool: &DbPool,
    draft_id: Option<Uuid>,
    user_id: Uuid,
    from_alias_id: Uuid,
    to_address: &str,
    subject: &str,
    body_key: Uuid,
) -> Result<Uuid, sqlx::Error> {
    let now = OffsetDateTime::now_utc();

    match pool {
        DbPool::Postgres(pool) => {
            if let Some(id) = draft_id {
                // Update existing draft
                sqlx::query(
                    r#"
                    UPDATE sent_emails 
                    SET from_alias_id = $1, to_address = $2, subject = $3, body_key = $4, updated_at = $5
                    WHERE id = $6 AND user_id = $7 AND status = 'draft'::email_status
                    "#,
                )
                .bind(from_alias_id)
                .bind(to_address)
                .bind(subject)
                .bind(body_key)
                .bind(now)
                .bind(id)
                .bind(user_id)
                .execute(pool)
                .await?;
                Ok(id)
            } else {
                // Insert new draft
                let new_id = Uuid::new_v4();
                sqlx::query(
                    r#"
                    INSERT INTO sent_emails (
                        id, user_id, from_alias_id, to_address, subject, body_key, 
                        status, created_at, updated_at
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, 'draft'::email_status, $7, $7)
                    "#,
                )
                .bind(new_id)
                .bind(user_id)
                .bind(from_alias_id)
                .bind(to_address)
                .bind(subject)
                .bind(body_key)
                .bind(now)
                .execute(pool)
                .await?;
                Ok(new_id)
            }
        }
        DbPool::Sqlite(pool) => {
            if let Some(id) = draft_id {
                // Update existing draft
                sqlx::query(
                    r#"
                    UPDATE sent_emails 
                    SET from_alias_id = ?, to_address = ?, subject = ?, body_key = ?, updated_at = ?
                    WHERE id = ? AND user_id = ? AND status = 'draft'
                    "#,
                )
                .bind(from_alias_id)
                .bind(to_address)
                .bind(subject)
                .bind(body_key)
                .bind(now)
                .bind(id)
                .bind(user_id)
                .execute(pool)
                .await?;
                Ok(id)
            } else {
                // Insert new draft
                let new_id = Uuid::new_v4();
                sqlx::query(
                    r#"
                    INSERT INTO sent_emails (
                        id, user_id, from_alias_id, to_address, subject, body_key, 
                        status, created_at, updated_at
                    )
                    VALUES (?, ?, ?, ?, ?, ?, 'draft', ?, ?)
                    "#,
                )
                .bind(new_id)
                .bind(user_id)
                .bind(from_alias_id)
                .bind(to_address)
                .bind(subject)
                .bind(body_key)
                .bind(now)
                .bind(now)
                .execute(pool)
                .await?;
                Ok(new_id)
            }
        }
    }
}

pub async fn get_sent_email_by_id_and_user(
    pool: &DbPool,
    id: Uuid,
    user_id: Uuid,
) -> Result<Option<SentEmailRow>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query_as::<_, SentEmailRow>(
                r#"
                SELECT s.id, s.user_id, s.from_alias_id, s.to_address, s.cc_addresses, s.bcc_addresses, s.subject, s.body_key, s.status, s.error_message, s.message_id, s.created_at, s.updated_at, s.sent_at, a.subdomain || '@' || d.name as alias_address
                FROM sent_emails s
                JOIN aliases a ON s.from_alias_id = a.id
                JOIN domains d ON a.domain_id = d.id
                WHERE s.id = $1 AND s.user_id = $2
                "#,
            )
            .bind(id)
            .bind(user_id)
            .fetch_optional(pool)
            .await
        }
        DbPool::Sqlite(pool) => {
            let email = sqlx::query_as::<sqlx::Sqlite, SentEmailRow>(
                r#"
                SELECT s.id, s.user_id, s.from_alias_id, s.to_address, s.cc_addresses, s.bcc_addresses, s.subject, s.body_key, s.status, s.error_message, s.message_id, s.created_at, s.updated_at, s.sent_at, a.subdomain || '@' || d.name as alias_address
                FROM sent_emails s
                JOIN aliases a ON s.from_alias_id = a.id
                JOIN domains d ON a.domain_id = d.id
                WHERE s.id = ? AND s.user_id = ?
                "#,
            )
            .bind(id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
            Ok(email)
        }
    }
}

pub async fn delete_sent_email_by_id(
    pool: &DbPool,
    id: Uuid,
    user_id: Uuid,
) -> Result<bool, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let result = sqlx::query::<sqlx::Postgres>(
                "DELETE FROM sent_emails WHERE id = $1 AND user_id = $2",
            )
            .bind(id)
            .bind(user_id)
            .execute(pool)
            .await?;
            Ok(result.rows_affected() > 0)
        }
        DbPool::Sqlite(pool) => {
            let result = sqlx::query::<sqlx::Sqlite>(
                "DELETE FROM sent_emails WHERE id = ? AND user_id = ?",
            )
            .bind(id)
            .bind(user_id)
            .execute(pool)
            .await?;
            Ok(result.rows_affected() > 0)
        }
    }
}
