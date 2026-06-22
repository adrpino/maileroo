pub mod aliases;
pub mod api_keys;
pub mod attachments;
pub mod domains;
pub mod queue;
pub mod replies;
pub mod reply_mappings;
pub mod sent_emails;
pub mod users;

#[cfg(test)]
pub mod sqlite_tests;

pub use aliases::*;
pub use api_keys::{
    ApiKey, delete_api_key, get_api_keys, get_user_by_api_key_hash, insert_api_key,
};
pub use domains::{
    Domain, clear_pending_dkim, delete_domain_by_id, get_dkim_key_by_domain, get_domain_by_id,
    get_domain_count, get_domains, insert_domain, promote_pending_dkim, update_pending_dkim,
};
pub use queue::{QueueJob, delete_job, fetch_next_retryable_jobs, insert_job, update_job_status};
pub use reply_mappings::{
    ReplyMappingLookup, get_or_create_reply_mapping, get_reply_mapping_by_token,
};
pub use users::{
    User, get_user_by_email, get_user_by_id, insert_user, seed_admin_on_startup, update_last_login,
};

use crate::web::ReceivedEmail;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use time::OffsetDateTime;

#[derive(Clone, Debug)]
pub enum DbPool {
    Postgres(PgPool),
    Sqlite(sqlx::SqlitePool),
}

pub async fn init_pool(database_url: &str) -> Result<DbPool, sqlx::Error> {
    if database_url.starts_with("sqlite://") {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(50)
            .connect(database_url)
            .await?;
        Ok(DbPool::Sqlite(pool))
    } else {
        let pool = PgPoolOptions::new()
            .max_connections(50)
            .connect(database_url)
            .await?;
        Ok(DbPool::Postgres(pool))
    }
}

pub async fn run_migrations(pool: &DbPool) -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("SKIP_MIGRATIONS").unwrap_or_default() == "true" {
        tracing::info!("SKIP_MIGRATIONS is set to true. Skipping database schema migrations.");
        return Ok(());
    }

    match pool {
        DbPool::Postgres(pg_pool) => {
            tracing::info!("Running pending Postgres database migrations...");
            sqlx::migrate!("./migrations/postgres").run(pg_pool).await?;
        }
        DbPool::Sqlite(sqlite_pool) => {
            tracing::info!("Running pending SQLite database migrations...");
            sqlx::migrate!("./migrations/sqlite")
                .run(sqlite_pool)
                .await?;
        }
    }

    Ok(())
}

pub async fn get_alias_details_for_email(
    pool: &DbPool,
    email_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<Option<(String, String)>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let result = sqlx::query_as::<_, (String, String)>(
                r#"SELECT a.subdomain, d.name as domain_name
                   FROM received_emails e
                   JOIN aliases a ON e.alias_id = a.id
                   JOIN domains d ON a.domain_id = d.id
                   WHERE e.id = $1 AND a.user_id = $2"#,
            )
            .bind(email_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;

            Ok(result)
        }
        DbPool::Sqlite(pool) => {
            let result = sqlx::query_as::<sqlx::Sqlite, (String, String)>(
                r#"SELECT a.subdomain, d.name as domain_name
                   FROM received_emails e
                   JOIN aliases a ON e.alias_id = a.id
                   JOIN domains d ON a.domain_id = d.id
                   WHERE e.id = ? AND a.user_id = ?"#,
            )
            .bind(email_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;

            Ok(result)
        }
    }
}

#[derive(Debug, Clone)]
pub enum AnyEmail {
    Received(ReceivedEmail),
    Sent(crate::db::sent_emails::SentEmailRow),
}

impl AnyEmail {
    pub fn subject(&self) -> &str {
        match self {
            AnyEmail::Received(e) => &e.subject,
            AnyEmail::Sent(e) => &e.subject,
        }
    }

    pub fn body_key(&self) -> uuid::Uuid {
        match self {
            AnyEmail::Received(e) => e.body_key,
            AnyEmail::Sent(e) => e.body_key,
        }
    }
}

pub async fn get_any_email(
    pool: &DbPool,
    id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<Option<AnyEmail>, sqlx::Error> {
    if let Some(e) = get_email_by_id(pool, id, user_id).await? {
        return Ok(Some(AnyEmail::Received(e)));
    }
    if let Some(e) =
        crate::db::sent_emails::get_sent_email_by_id_and_user(pool, id, user_id).await?
    {
        return Ok(Some(AnyEmail::Sent(e)));
    }
    Ok(None)
}

pub async fn get_email_by_id(
    pool: &DbPool,
    email_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<Option<ReceivedEmail>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let email = sqlx::query_as::<_, ReceivedEmail>(
                r#"SELECT
                    e.id,
                    e.alias_id,
                    (a.subdomain || '@' || d.name) as alias_address,
                    e.sender_email,
                    e.subject,
                    e.body_key,
                    e.received_at,
                    e.last_activity_at,
                    e.viewed,
                    e.forwarded,
                    e.message_id,
                    e.thread_id,
                    e.has_attachments,
                    a.user_id
                 FROM received_emails e 
                 JOIN aliases a on a.id = e.alias_id
                 JOIN domains d on d.id = a.domain_id
                 WHERE e.id = $1 AND a.user_id = $2"#,
            )
            .bind(email_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
            Ok(email)
        }
        DbPool::Sqlite(pool) => {
            let email = sqlx::query_as::<sqlx::Sqlite, ReceivedEmail>(
                r#"SELECT
                    e.id,
                    e.alias_id,
                    (a.subdomain || '@' || d.name) as alias_address,
                    e.sender_email,
                    e.subject,
                    e.body_key,
                    e.received_at,
                    e.last_activity_at,
                    e.viewed,
                    e.forwarded,
                    e.message_id,
                    e.thread_id,
                    e.has_attachments,
                    a.user_id
                 FROM received_emails e 
                 JOIN aliases a on a.id = e.alias_id
                 JOIN domains d on d.id = a.domain_id
                 WHERE e.id = ? AND a.user_id = ?"#,
            )
            .bind(email_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
            Ok(email)
        }
    }
}

pub async fn delete_email_by_id(
    pool: &DbPool,
    email_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<bool, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let result = sqlx::query::<sqlx::Postgres>(
                r#"DELETE FROM received_emails e
                   USING aliases a
                   WHERE e.alias_id = a.id
                     AND e.id = $1
                     AND a.user_id = $2"#,
            )
            .bind(email_id)
            .bind(user_id)
            .execute(pool)
            .await?;
            Ok(result.rows_affected() > 0)
        }
        DbPool::Sqlite(pool) => {
            let result = sqlx::query(
                r#"DELETE FROM received_emails 
                   WHERE id = ? 
                     AND alias_id IN (SELECT id FROM aliases WHERE user_id = ?)"#,
            )
            .bind(email_id)
            .bind(user_id)
            .execute(pool)
            .await?;
            Ok(result.rows_affected() > 0)
        }
    }
}

pub async fn get_email_by_user_id(
    pool: &DbPool,
    user_id: uuid::Uuid,
    limit: i64,
    offset: i64,
    alias_address: Option<String>,
    query: Option<String>,
) -> Result<Vec<ReceivedEmail>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let emails: Vec<ReceivedEmail> = sqlx::query_as::<_, ReceivedEmail>(
                r#"SELECT
                    e.id,
                    e.alias_id,
                    (a.subdomain || '@' || d.name) as alias_address,
                    e.sender_email,
                    e.subject,
                    e.body_key,
                    e.received_at,
                    e.last_activity_at,
                    e.viewed,
                    e.forwarded,
                    e.message_id,
                    e.thread_id,
                    e.has_attachments,
                    a.user_id
                 FROM received_emails e 
                 JOIN aliases a on a.id = e.alias_id
                 JOIN domains d on d.id = a.domain_id
                 WHERE
                 a.user_id = $1 AND e.thread_id IS NULL
                 AND ($4::TEXT IS NULL OR (a.subdomain || '@' || d.name) = $4)
                 AND ($5::TEXT IS NULL OR e.subject ILIKE '%' || $5 || '%' OR e.sender_email ILIKE '%' || $5 || '%')
                 ORDER BY e.last_activity_at DESC
                 LIMIT $2 OFFSET $3"#,
            )
            .bind(user_id)
            .bind(limit)
            .bind(offset)
            .bind(alias_address)
            .bind(query)
            .fetch_all(pool)
            .await?;
            Ok(emails)
        }
        DbPool::Sqlite(pool) => {
            let mut sql = String::from(
                r#"SELECT
                    e.id,
                    e.alias_id,
                    (a.subdomain || '@' || d.name) as alias_address,
                    e.sender_email,
                    e.subject,
                    e.body_key,
                    e.received_at,
                    e.last_activity_at,
                    e.viewed,
                    e.forwarded,
                    e.message_id,
                    e.thread_id,
                    e.has_attachments,
                    a.user_id
                 FROM received_emails e 
                 JOIN aliases a on a.id = e.alias_id
                 JOIN domains d on d.id = a.domain_id
                 WHERE a.user_id = ? AND e.thread_id IS NULL"#,
            );

            if alias_address.is_some() {
                sql.push_str(" AND (a.subdomain || '@' || d.name) = ?");
            }

            if query.is_some() {
                sql.push_str(" AND (e.subject LIKE ? OR e.sender_email LIKE ?)");
            }

            sql.push_str(" ORDER BY e.last_activity_at DESC LIMIT ? OFFSET ?");

            let mut q = sqlx::query_as::<sqlx::Sqlite, ReceivedEmail>(&sql).bind(user_id);

            if let Some(alias) = &alias_address {
                q = q.bind(alias);
            }

            if let Some(search) = &query {
                let pattern = format!("%{}%", search);
                q = q.bind(pattern.clone()).bind(pattern);
            }

            q = q.bind(limit).bind(offset);

            let emails = q.fetch_all(pool).await?;
            Ok(emails)
        }
    }
}

pub async fn get_email_count_by_user_id(
    pool: &DbPool,
    user_id: uuid::Uuid,
    alias_address: Option<String>,
    query: Option<String>,
) -> Result<i64, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let count = sqlx::query_scalar::<_, i64>(
                r#"SELECT COUNT(*) FROM received_emails e
                   JOIN aliases a ON a.id = e.alias_id
                   JOIN domains d on d.id = a.domain_id
                   WHERE a.user_id = $1 AND e.thread_id IS NULL
                   AND ($2::TEXT IS NULL OR (a.subdomain || '@' || d.name) = $2)
                   AND ($3::TEXT IS NULL OR e.subject ILIKE '%' || $3 || '%' OR e.sender_email ILIKE '%' || $3 || '%')"#,
            )
            .bind(user_id)
            .bind(alias_address)
            .bind(query)
            .fetch_one(pool)
            .await?;
            Ok(count)
        }
        DbPool::Sqlite(pool) => {
            let mut sql = String::from(
                r#"SELECT COUNT(*) FROM received_emails e
                   JOIN aliases a ON a.id = e.alias_id
                   JOIN domains d on d.id = a.domain_id
                   WHERE a.user_id = ? AND e.thread_id IS NULL"#,
            );

            if alias_address.is_some() {
                sql.push_str(" AND (a.subdomain || '@' || d.name) = ?");
            }

            if query.is_some() {
                sql.push_str(" AND (e.subject LIKE ? OR e.sender_email LIKE ?)");
            }

            let mut q = sqlx::query_scalar::<sqlx::Sqlite, i64>(&sql).bind(user_id);

            if let Some(alias) = &alias_address {
                q = q.bind(alias);
            }

            if let Some(search) = &query {
                let pattern = format!("%{}%", search);
                q = q.bind(pattern.clone()).bind(pattern);
            }

            let count = q.fetch_one(pool).await?;
            Ok(count)
        }
    }
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct AliasLookup {
    pub id: uuid::Uuid,
    pub destination_email: String,
    pub auto_forward: bool,
}
/// Full alias struct
#[derive(Clone, sqlx::FromRow)]
pub struct Alias {
    pub id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub domain_id: uuid::Uuid,
    pub subdomain: String,
    pub destination_email: String,
    pub auto_forward: bool,
    pub active: bool,
    pub created_at: OffsetDateTime,
    pub domain_name: String, // Joined field
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct UserWithStats {
    pub id: uuid::Uuid,
    pub email: String,
    pub is_admin: bool,
    pub bypass_alias_limit: bool,
    pub disable_autoclean: bool,
    pub can_send_firsthand: bool,
    pub created_at: OffsetDateTime,
    pub last_login_at: Option<OffsetDateTime>,
    pub last_login_ip: Option<String>,
    pub alias_count: i64,
    pub email_count: i64,
}

pub async fn get_child_emails(
    pool: &DbPool,
    thread_id: uuid::Uuid,
) -> Result<Vec<ReceivedEmail>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let emails = sqlx::query_as::<_, ReceivedEmail>(
                r#"SELECT
                    e.id,
                    e.alias_id,
                    (a.subdomain || '@' || d.name) as alias_address,
                    a.user_id,
                    e.sender_email,
                    e.subject,
                    e.body_key,
                    e.received_at,
                    e.last_activity_at,
                    e.viewed,
                    e.forwarded,
                    e.message_id,
                    e.thread_id,
                    e.has_attachments,
                    a.user_id
                 FROM received_emails e 
                 JOIN aliases a on a.id = e.alias_id
                 JOIN domains d on d.id = a.domain_id
                 WHERE e.thread_id = $1
                 ORDER BY e.received_at ASC"#,
            )
            .bind(thread_id)
            .fetch_all(pool)
            .await?;
            Ok(emails)
        }
        DbPool::Sqlite(pool) => {
            let emails = sqlx::query_as::<sqlx::Sqlite, ReceivedEmail>(
                r#"SELECT
                    e.id,
                    e.alias_id,
                    (a.subdomain || '@' || d.name) as alias_address,
                    a.user_id,
                    e.sender_email,
                    e.subject,
                    e.body_key,
                    e.received_at,
                    e.last_activity_at,
                    e.viewed,
                    e.forwarded,
                    e.message_id,
                    e.thread_id,
                    e.has_attachments,
                    a.user_id
                 FROM received_emails e 
                 JOIN aliases a on a.id = e.alias_id
                 JOIN domains d on d.id = a.domain_id
                 WHERE e.thread_id = ?
                 ORDER BY e.received_at ASC"#,
            )
            .bind(thread_id)
            .fetch_all(pool)
            .await?;
            Ok(emails)
        }
    }
}

pub async fn insert_email(
    pool: &DbPool,
    alias_id: uuid::Uuid,
    sender: &str,
    subject: &str,
    body_key: uuid::Uuid,
    received_at: Option<OffsetDateTime>,
    message_id: Option<String>,
    thread_id: Option<uuid::Uuid>,
) -> Result<ReceivedEmail, sqlx::Error> {
    let received_at_val = received_at.unwrap_or_else(OffsetDateTime::now_utc);

    match pool {
        DbPool::Postgres(pool) => {
            // If this is a reply (has thread_id), update the last_activity_at of the root email
            if let Some(root_id) = thread_id {
                sqlx::query("UPDATE received_emails SET last_activity_at = $1 WHERE id = $2")
                    .bind(received_at_val)
                    .bind(root_id)
                    .execute(pool)
                    .await?;
            }

            let email = sqlx::query_as::<_, ReceivedEmail>(
                r#"WITH inserted AS (
                   INSERT INTO received_emails (
                        id, 
                        alias_id, 
                        sender_email,
                        subject, 
                        body_key,
                        received_at,
                        last_activity_at,
                        forwarded,
                        message_id,
                        thread_id
                         ) VALUES ($1, $2, $3, $4, $5, $6, $6, false, $7, $8)
                         RETURNING *
                )
                SELECT 
                    i.id,
                    i.alias_id as alias_id,
                    (a.subdomain || '@' || d.name) as alias_address,
                    a.user_id as user_id,
                    i.sender_email as sender_email,
                    i.subject as subject,
                    i.body_key as body_key,
                    i.received_at as received_at,
                    i.last_activity_at as last_activity_at,
                    i.viewed as viewed,
                    i.forwarded as forwarded,
                    i.message_id,
                    i.thread_id,
                    i.has_attachments
                FROM inserted i
                JOIN aliases a ON i.alias_id = a.id
                JOIN domains d ON a.domain_id = d.id"#,
            )
            .bind(uuid::Uuid::new_v4())
            .bind(alias_id)
            .bind(sender)
            .bind(subject)
            .bind(body_key)
            .bind(received_at_val)
            .bind(message_id)
            .bind(thread_id)
            .fetch_one(pool)
            .await?;

            Ok(email)
        }
        DbPool::Sqlite(pool) => {
            if let Some(root_id) = thread_id {
                sqlx::query("UPDATE received_emails SET last_activity_at = ? WHERE id = ?")
                    .bind(received_at_val)
                    .bind(root_id)
                    .execute(pool)
                    .await?;
            }

            let id = uuid::Uuid::new_v4();
            sqlx::query(
                r#"INSERT INTO received_emails (
                    id, 
                    alias_id, 
                    sender_email,
                    subject, 
                    body_key,
                    received_at,
                    last_activity_at,
                    forwarded,
                    message_id,
                    thread_id
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(id)
            .bind(alias_id)
            .bind(sender)
            .bind(subject)
            .bind(body_key)
            .bind(received_at_val)
            .bind(received_at_val)
            .bind(false)
            .bind(message_id)
            .bind(thread_id)
            .execute(pool)
            .await?;

            let email = sqlx::query_as::<sqlx::Sqlite, ReceivedEmail>(
                r#"
                SELECT 
                    e.id,
                    e.alias_id,
                    (a.subdomain || '@' || d.name) as alias_address,
                    a.user_id,
                    e.sender_email,
                    e.subject,
                    e.body_key,
                    e.received_at,
                    e.last_activity_at,
                    e.viewed,
                    e.forwarded,
                    e.message_id,
                    e.thread_id,
                    e.has_attachments,
                    a.user_id
                FROM received_emails e
                JOIN aliases a ON e.alias_id = a.id
                JOIN domains d ON a.domain_id = d.id
                WHERE e.id = ?
                "#,
            )
            .bind(id)
            .fetch_one(pool)
            .await?;

            Ok(email)
        }
    }
}

pub async fn find_thread_id_by_references(
    pool: &DbPool,
    references: &[String],
) -> Result<Option<uuid::Uuid>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            if references.is_empty() {
                return Ok(None);
            }

            // 1. Check if any reference matches a root email's message_id
            let root_id = sqlx::query_scalar::<_, uuid::Uuid>(
                r#"SELECT COALESCE(thread_id, id)
                   FROM received_emails 
                   WHERE message_id = ANY($1) 
                   LIMIT 1"#,
            )
            .bind(references)
            .fetch_optional(pool)
            .await?;

            if let Some(id) = root_id {
                return Ok(Some(id));
            }

            // 2. Check if any reference matches one of our outbound replies
            let outbound_root_id = sqlx::query_scalar::<_, uuid::Uuid>(
                r#"SELECT email_id FROM email_replies WHERE message_id = ANY($1) LIMIT 1"#,
            )
            .bind(references)
            .fetch_optional(pool)
            .await?;

            Ok(outbound_root_id)
        }
        DbPool::Sqlite(pool) => {
            if references.is_empty() {
                return Ok(None);
            }

            let placeholders = vec!["?"; references.len()].join(", ");

            // 1. Check received_emails
            let sql_root = format!(
                "SELECT COALESCE(thread_id, id) FROM received_emails WHERE message_id IN ({}) LIMIT 1",
                placeholders
            );

            let mut q1 = sqlx::query_scalar::<sqlx::Sqlite, uuid::Uuid>(&sql_root);
            for r in references {
                q1 = q1.bind(r);
            }

            if let Some(id) = q1.fetch_optional(pool).await? {
                return Ok(Some(id));
            }

            // 2. Check email_replies
            let sql_reply = format!(
                "SELECT email_id FROM email_replies WHERE message_id IN ({}) LIMIT 1",
                placeholders
            );

            let mut q2 = sqlx::query_scalar::<sqlx::Sqlite, uuid::Uuid>(&sql_reply);
            for r in references {
                q2 = q2.bind(r);
            }

            let outbound_root_id = q2.fetch_optional(pool).await?;
            Ok(outbound_root_id)
        }
    }
}

pub async fn mark_email_as_forwarded(
    pool: &DbPool,
    email_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query("UPDATE received_emails SET forwarded = true WHERE id = $1")
                .bind(email_id)
                .execute(pool)
                .await?;
            Ok(())
        }
        DbPool::Sqlite(pool) => {
            sqlx::query("UPDATE received_emails SET forwarded = 1 WHERE id = ?")
                .bind(email_id)
                .execute(pool)
                .await?;
            Ok(())
        }
    }
}

pub async fn get_all_users_with_stats(pool: &DbPool) -> Result<Vec<UserWithStats>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let users = sqlx::query_as::<_, UserWithStats>(
                r#"
                SELECT
                    u.id,
                    u.email,
                    u.is_admin,
                    u.bypass_alias_limit,
                    u.disable_autoclean,
                    u.can_send_firsthand,
                    u.created_at,
                    u.last_login_at,
                    u.last_login_ip,
                    COUNT(DISTINCT a.id) as alias_count,
                    COUNT(DISTINCT re.id) as email_count
                FROM users u
                LEFT JOIN aliases a ON u.id = a.user_id
                LEFT JOIN received_emails re ON a.id = re.alias_id
                GROUP BY u.id
                ORDER BY u.created_at DESC
                "#,
            )
            .fetch_all(pool)
            .await?;

            Ok(users)
        }
        DbPool::Sqlite(pool) => {
            let users = sqlx::query_as::<sqlx::Sqlite, UserWithStats>(
                r#"
                SELECT
                    u.id,
                    u.email,
                    u.is_admin,
                    u.bypass_alias_limit,
                    u.disable_autoclean,
                    u.can_send_firsthand,
                    u.created_at,
                    u.last_login_at,
                    u.last_login_ip,
                    COUNT(DISTINCT a.id) as alias_count,
                    COUNT(DISTINCT re.id) as email_count
                FROM users u
                LEFT JOIN aliases a ON u.id = a.user_id
                LEFT JOIN received_emails re ON a.id = re.alias_id
                GROUP BY u.id
                ORDER BY u.created_at DESC
                "#,
            )
            .fetch_all(pool)
            .await?;

            Ok(users)
        }
    }
}

pub async fn delete_old_emails(
    pool: &DbPool,
    retention_days: i64,
) -> Result<Vec<uuid::Uuid>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let old_emails = sqlx::query_scalar::<_, uuid::Uuid>(
                r#"DELETE FROM received_emails
                   WHERE received_at < NOW() - ($1 * INTERVAL '1 day')
                   AND (alias_id IS NULL OR EXISTS (
                       SELECT 1 FROM aliases a
                       JOIN users u ON a.user_id = u.id
                       WHERE a.id = received_emails.alias_id
                       AND u.disable_autoclean = FALSE
                   ))
                   RETURNING body_key"#,
            )
            .bind(retention_days as f64)
            .fetch_all(pool)
            .await?;

            Ok(old_emails)
        }
        DbPool::Sqlite(pool) => {
            let modifier = format!("-{} days", retention_days);

            let old_emails = sqlx::query_scalar::<sqlx::Sqlite, uuid::Uuid>(
                r#"SELECT body_key FROM received_emails
                   WHERE received_at < datetime('now', ?)
                   AND (alias_id IS NULL OR EXISTS (
                       SELECT 1 FROM aliases a
                       JOIN users u ON a.user_id = u.id
                       WHERE a.id = received_emails.alias_id
                       AND u.disable_autoclean = 0
                   ))"#,
            )
            .bind(&modifier)
            .fetch_all(pool)
            .await?;

            sqlx::query(
                r#"DELETE FROM received_emails
                   WHERE received_at < datetime('now', ?)
                   AND (alias_id IS NULL OR EXISTS (
                       SELECT 1 FROM aliases a
                       JOIN users u ON a.user_id = u.id
                       WHERE a.id = received_emails.alias_id
                       AND u.disable_autoclean = 0
                   ))"#,
            )
            .bind(&modifier)
            .execute(pool)
            .await?;

            Ok(old_emails)
        }
    }
}

pub async fn get_alias_count(pool: &DbPool, user_id: uuid::Uuid) -> Result<i64, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let count =
                sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM aliases WHERE user_id = $1")
                    .bind(user_id)
                    .fetch_one(pool)
                    .await?;
            Ok(count)
        }
        DbPool::Sqlite(pool) => {
            let count = sqlx::query_scalar::<sqlx::Sqlite, i64>(
                "SELECT COUNT(*) FROM aliases WHERE user_id = ?",
            )
            .bind(user_id)
            .fetch_one(pool)
            .await?;
            Ok(count)
        }
    }
}

pub async fn mark_email_as_viewed(
    pool: &DbPool,
    email_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query(
                r#"UPDATE received_emails e
                   SET viewed = true
                   FROM aliases a
                   WHERE e.alias_id = a.id
                     AND e.id = $1
                     AND a.user_id = $2"#,
            )
            .bind(email_id)
            .bind(user_id)
            .execute(pool)
            .await?;
            Ok(())
        }
        DbPool::Sqlite(pool) => {
            sqlx::query(
                r#"UPDATE received_emails 
                   SET viewed = 1
                   WHERE id = ? 
                     AND alias_id IN (SELECT id FROM aliases WHERE user_id = ?)"#,
            )
            .bind(email_id)
            .bind(user_id)
            .execute(pool)
            .await?;
            Ok(())
        }
    }
}
