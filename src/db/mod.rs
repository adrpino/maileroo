pub mod aliases;
pub mod api_keys;
pub mod attachments;
pub mod domains;
pub mod labels;
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
pub use labels::*;
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

    pub fn is_favorite(&self) -> bool {
        match self {
            AnyEmail::Received(e) => e.is_favorite,
            AnyEmail::Sent(e) => e.is_favorite,
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
                    e.is_favorite,
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
                    e.is_favorite,
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
    label: Option<String>,
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
                    e.is_favorite,
                    a.user_id
                 FROM received_emails e 
                 JOIN aliases a on a.id = e.alias_id
                 JOIN domains d on d.id = a.domain_id
                 WHERE
                 a.user_id = $1 AND e.thread_id IS NULL
                 AND ($4::TEXT IS NULL OR (a.subdomain || '@' || d.name) = $4)
                 AND ($5::TEXT IS NULL OR e.subject ILIKE '%' || $5 || '%' OR e.sender_email ILIKE '%' || $5 || '%')
                 AND ($6::TEXT IS NULL OR EXISTS (
                     SELECT 1 FROM email_labels el
                     JOIN labels l ON el.label_id = l.id
                     WHERE el.email_id = e.id AND LOWER(l.name) = LOWER($6)
                 ))
                 ORDER BY e.last_activity_at DESC
                 LIMIT $2 OFFSET $3"#,
            )
            .bind(user_id)
            .bind(limit)
            .bind(offset)
            .bind(alias_address)
            .bind(query)
            .bind(label)
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
                    e.is_favorite,
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

            if label.is_some() {
                sql.push_str(" AND EXISTS (SELECT 1 FROM email_labels el JOIN labels l ON el.label_id = l.id WHERE el.email_id = e.id AND LOWER(l.name) = LOWER(?))");
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

            if let Some(lbl) = &label {
                q = q.bind(lbl);
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
    label: Option<String>,
) -> Result<i64, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let count = sqlx::query_scalar::<_, i64>(
                r#"SELECT COUNT(*) FROM received_emails e
                   JOIN aliases a ON a.id = e.alias_id
                   JOIN domains d on d.id = a.domain_id
                   WHERE a.user_id = $1 AND e.thread_id IS NULL
                   AND ($2::TEXT IS NULL OR (a.subdomain || '@' || d.name) = $2)
                   AND ($3::TEXT IS NULL OR e.subject ILIKE '%' || $3 || '%' OR e.sender_email ILIKE '%' || $3 || '%')
                   AND ($4::TEXT IS NULL OR EXISTS (
                       SELECT 1 FROM email_labels el
                       JOIN labels l ON el.label_id = l.id
                       WHERE el.email_id = e.id AND LOWER(l.name) = LOWER($4)
                   ))"#,
            )
            .bind(user_id)
            .bind(alias_address)
            .bind(query)
            .bind(label)
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

            if label.is_some() {
                sql.push_str(" AND EXISTS (SELECT 1 FROM email_labels el JOIN labels l ON el.label_id = l.id WHERE el.email_id = e.id AND LOWER(l.name) = LOWER(?))");
            }

            let mut q = sqlx::query_scalar::<sqlx::Sqlite, i64>(&sql).bind(user_id);

            if let Some(alias) = &alias_address {
                q = q.bind(alias);
            }

            if let Some(search) = &query {
                let pattern = format!("%{}%", search);
                q = q.bind(pattern.clone()).bind(pattern);
            }

            if let Some(lbl) = &label {
                q = q.bind(lbl);
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
                    e.is_favorite,
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
                    e.is_favorite,
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
            // If this is a reply (has thread_id), update the last_activity_at and reset viewed of the root email
            if let Some(root_id) = thread_id {
                sqlx::query("UPDATE received_emails SET last_activity_at = $1, viewed = FALSE WHERE id = $2")
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
                    i.has_attachments,
                    i.is_favorite
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
                sqlx::query(
                    "UPDATE received_emails SET last_activity_at = ?, viewed = 0 WHERE id = ?",
                )
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
                    e.is_favorite,
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

            // 2. Check if any reference matches one of our outbound replies.
            // The reply may have been sent from a child email, so resolve to its root.
            let outbound_root_id = sqlx::query_scalar::<_, uuid::Uuid>(
                r#"SELECT COALESCE(e.thread_id, e.id)
                   FROM email_replies r
                   JOIN received_emails e ON e.id = r.email_id
                   WHERE r.message_id = ANY($1)
                   LIMIT 1"#,
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

            // 2. Check email_replies, resolving to the root in case the reply was sent from a child email
            let sql_reply = format!(
                "SELECT COALESCE(e.thread_id, e.id) FROM email_replies r JOIN received_emails e ON e.id = r.email_id WHERE r.message_id IN ({}) LIMIT 1",
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
                   AND NOT (
                       EXISTS (
                           SELECT 1 FROM aliases a
                           JOIN users u ON a.user_id = u.id
                           WHERE a.id = received_emails.alias_id
                             AND u.is_admin = TRUE
                       )
                       AND EXISTS (
                           SELECT 1 FROM received_emails t
                           WHERE (t.id = COALESCE(received_emails.thread_id, received_emails.id)
                               OR t.thread_id = COALESCE(received_emails.thread_id, received_emails.id))
                             AND t.is_favorite = TRUE
                       )
                   )
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
                   ))
                   AND NOT (
                       EXISTS (
                           SELECT 1 FROM aliases a
                           JOIN users u ON a.user_id = u.id
                           WHERE a.id = received_emails.alias_id
                             AND u.is_admin = 1
                       )
                       AND EXISTS (
                           SELECT 1 FROM received_emails t
                           WHERE (t.id = COALESCE(received_emails.thread_id, received_emails.id)
                               OR t.thread_id = COALESCE(received_emails.thread_id, received_emails.id))
                             AND t.is_favorite = 1
                       )
                   )"#,
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
                   ))
                   AND NOT (
                       EXISTS (
                           SELECT 1 FROM aliases a
                           JOIN users u ON a.user_id = u.id
                           WHERE a.id = received_emails.alias_id
                             AND u.is_admin = 1
                       )
                       AND EXISTS (
                           SELECT 1 FROM received_emails t
                           WHERE (t.id = COALESCE(received_emails.thread_id, received_emails.id)
                               OR t.thread_id = COALESCE(received_emails.thread_id, received_emails.id))
                             AND t.is_favorite = 1
                       )
                   )"#,
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

pub async fn set_email_viewed(
    pool: &DbPool,
    email_id: uuid::Uuid,
    user_id: uuid::Uuid,
    viewed: bool,
) -> Result<bool, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let res = sqlx::query(
                r#"UPDATE received_emails e
                   SET viewed = $1
                   FROM aliases a
                   WHERE e.alias_id = a.id
                     AND e.id = $2
                     AND a.user_id = $3"#,
            )
            .bind(viewed)
            .bind(email_id)
            .bind(user_id)
            .execute(pool)
            .await?;
            Ok(res.rows_affected() > 0)
        }
        DbPool::Sqlite(pool) => {
            let res = sqlx::query(
                r#"UPDATE received_emails 
                   SET viewed = ?
                   WHERE id = ? 
                     AND alias_id IN (SELECT id FROM aliases WHERE user_id = ?)"#,
            )
            .bind(viewed)
            .bind(email_id)
            .bind(user_id)
            .execute(pool)
            .await?;
            Ok(res.rows_affected() > 0)
        }
    }
}

pub async fn mark_email_as_viewed(
    pool: &DbPool,
    email_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    set_email_viewed(pool, email_id, user_id, true).await?;
    Ok(())
}

pub async fn batch_set_emails_viewed(
    pool: &DbPool,
    email_ids: &[uuid::Uuid],
    user_id: uuid::Uuid,
    viewed: bool,
) -> Result<u64, sqlx::Error> {
    if email_ids.is_empty() {
        return Ok(0);
    }
    match pool {
        DbPool::Postgres(pool) => {
            let res = sqlx::query(
                r#"UPDATE received_emails e
                   SET viewed = $1
                   FROM aliases a
                   WHERE e.alias_id = a.id
                     AND e.id = ANY($2)
                     AND a.user_id = $3"#,
            )
            .bind(viewed)
            .bind(email_ids)
            .bind(user_id)
            .execute(pool)
            .await?;
            Ok(res.rows_affected())
        }
        DbPool::Sqlite(pool) => {
            let placeholders = vec!["?"; email_ids.len()].join(", ");
            let sql = format!(
                "UPDATE received_emails SET viewed = ? WHERE id IN ({}) AND alias_id IN (SELECT id FROM aliases WHERE user_id = ?)",
                placeholders
            );
            let mut q = sqlx::query(&sql).bind(viewed);
            for id in email_ids {
                q = q.bind(id);
            }
            q = q.bind(user_id);
            let res = q.execute(pool).await?;
            Ok(res.rows_affected())
        }
    }
}

pub async fn toggle_email_favorite(
    pool: &DbPool,
    email_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<Option<bool>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let received_res = sqlx::query_scalar::<_, bool>(
                r#"UPDATE received_emails e
                   SET is_favorite = NOT e.is_favorite
                   FROM aliases a
                   WHERE e.alias_id = a.id
                     AND e.id = $1
                     AND a.user_id = $2
                   RETURNING e.is_favorite"#,
            )
            .bind(email_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;

            if let Some(fav) = received_res {
                return Ok(Some(fav));
            }

            let sent_res = sqlx::query_scalar::<_, bool>(
                r#"UPDATE sent_emails
                   SET is_favorite = NOT is_favorite
                   WHERE id = $1 AND user_id = $2
                   RETURNING is_favorite"#,
            )
            .bind(email_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;

            Ok(sent_res)
        }
        DbPool::Sqlite(pool) => {
            let received_res = sqlx::query_scalar::<sqlx::Sqlite, bool>(
                r#"UPDATE received_emails
                   SET is_favorite = NOT is_favorite
                   WHERE id = ? AND alias_id IN (SELECT id FROM aliases WHERE user_id = ?)
                   RETURNING is_favorite"#,
            )
            .bind(email_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;

            if let Some(fav) = received_res {
                return Ok(Some(fav));
            }

            let sent_res = sqlx::query_scalar::<sqlx::Sqlite, bool>(
                r#"UPDATE sent_emails
                   SET is_favorite = NOT is_favorite
                   WHERE id = ? AND user_id = ?
                   RETURNING is_favorite"#,
            )
            .bind(email_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;

            Ok(sent_res)
        }
    }
}

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct FavoriteEmailRow {
    pub id: uuid::Uuid,
    pub alias_address: String,
    pub correspondent_email: String,
    pub subject: String,
    pub date: time::OffsetDateTime,
    pub is_sent_folder: bool,
    pub is_viewed: bool,
    pub outbound_status: Option<String>,
    pub has_attachments: bool,
    pub is_favorite: bool,
}

pub async fn get_favorite_emails_by_user_id(
    pool: &DbPool,
    user_id: uuid::Uuid,
    limit: i64,
    offset: i64,
    alias_address: Option<String>,
    query: Option<String>,
) -> Result<Vec<FavoriteEmailRow>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let emails = sqlx::query_as::<_, FavoriteEmailRow>(
                r#"SELECT * FROM (
                    SELECT 
                        e.id,
                        (a.subdomain || '@' || d.name) as alias_address,
                        e.sender_email as correspondent_email,
                        e.subject,
                        e.last_activity_at as date,
                        FALSE as is_sent_folder,
                        e.viewed as is_viewed,
                        NULL::text as outbound_status,
                        e.has_attachments,
                        e.is_favorite
                    FROM received_emails e
                    JOIN aliases a ON e.alias_id = a.id
                    JOIN domains d ON a.domain_id = d.id
                    WHERE a.user_id = $1 AND e.thread_id IS NULL AND e.is_favorite = TRUE
                      AND ($4::TEXT IS NULL OR (a.subdomain || '@' || d.name) = $4)
                      AND ($5::TEXT IS NULL OR e.subject ILIKE '%' || $5 || '%' OR e.sender_email ILIKE '%' || $5 || '%')

                    UNION ALL

                    SELECT
                        s.id,
                        (a.subdomain || '@' || d.name) as alias_address,
                        s.to_address as correspondent_email,
                        s.subject,
                        COALESCE(s.sent_at, s.created_at) as date,
                        TRUE as is_sent_folder,
                        TRUE as is_viewed,
                        s.status::text as outbound_status,
                        s.has_attachments,
                        s.is_favorite
                    FROM sent_emails s
                    JOIN aliases a ON s.from_alias_id = a.id
                    JOIN domains d ON a.domain_id = d.id
                    WHERE s.user_id = $1 AND s.is_favorite = TRUE AND s.status != 'draft'::email_status
                      AND ($4::TEXT IS NULL OR (a.subdomain || '@' || d.name) = $4)
                      AND ($5::TEXT IS NULL OR s.subject ILIKE '%' || $5 || '%' OR s.to_address ILIKE '%' || $5 || '%')
                ) combined
                ORDER BY date DESC
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
                r#"SELECT * FROM (
                    SELECT 
                        e.id,
                        (a.subdomain || '@' || d.name) as alias_address,
                        e.sender_email as correspondent_email,
                        e.subject,
                        e.last_activity_at as date,
                        0 as is_sent_folder,
                        e.viewed as is_viewed,
                        NULL as outbound_status,
                        e.has_attachments,
                        e.is_favorite
                    FROM received_emails e
                    JOIN aliases a ON e.alias_id = a.id
                    JOIN domains d ON a.domain_id = d.id
                    WHERE a.user_id = ? AND e.thread_id IS NULL AND e.is_favorite = 1"#,
            );

            if alias_address.is_some() {
                sql.push_str(" AND (a.subdomain || '@' || d.name) = ?");
            }
            if query.is_some() {
                sql.push_str(" AND (e.subject LIKE ? OR e.sender_email LIKE ?)");
            }

            sql.push_str(
                r#"
                    UNION ALL

                    SELECT
                        s.id,
                        (a.subdomain || '@' || d.name) as alias_address,
                        s.to_address as correspondent_email,
                        s.subject,
                        COALESCE(s.sent_at, s.created_at) as date,
                        1 as is_sent_folder,
                        1 as is_viewed,
                        s.status as outbound_status,
                        s.has_attachments,
                        s.is_favorite
                    FROM sent_emails s
                    JOIN aliases a ON s.from_alias_id = a.id
                    JOIN domains d ON a.domain_id = d.id
                    WHERE s.user_id = ? AND s.is_favorite = 1 AND s.status != 'draft'"#,
            );

            if alias_address.is_some() {
                sql.push_str(" AND (a.subdomain || '@' || d.name) = ?");
            }
            if query.is_some() {
                sql.push_str(" AND (s.subject LIKE ? OR s.to_address LIKE ?)");
            }

            sql.push_str(") ORDER BY date DESC LIMIT ? OFFSET ?");

            let mut q = sqlx::query_as::<sqlx::Sqlite, FavoriteEmailRow>(&sql).bind(user_id);

            if let Some(alias) = &alias_address {
                q = q.bind(alias);
            }
            if let Some(search) = &query {
                let pattern = format!("%{}%", search);
                q = q.bind(pattern.clone()).bind(pattern);
            }

            q = q.bind(user_id);

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

pub async fn get_favorite_email_count_by_user_id(
    pool: &DbPool,
    user_id: uuid::Uuid,
    alias_address: Option<String>,
    query: Option<String>,
) -> Result<i64, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let count = sqlx::query_scalar::<_, i64>(
                r#"SELECT (
                    (SELECT COUNT(*) FROM received_emails e
                     JOIN aliases a ON e.alias_id = a.id
                     JOIN domains d ON a.domain_id = d.id
                     WHERE a.user_id = $1 AND e.thread_id IS NULL AND e.is_favorite = TRUE
                       AND ($2::TEXT IS NULL OR (a.subdomain || '@' || d.name) = $2)
                       AND ($3::TEXT IS NULL OR e.subject ILIKE '%' || $3 || '%' OR e.sender_email ILIKE '%' || $3 || '%')
                    )
                    +
                    (SELECT COUNT(*) FROM sent_emails s
                     JOIN aliases a ON s.from_alias_id = a.id
                     JOIN domains d ON a.domain_id = d.id
                     WHERE s.user_id = $1 AND s.is_favorite = TRUE AND s.status != 'draft'::email_status
                       AND ($2::TEXT IS NULL OR (a.subdomain || '@' || d.name) = $2)
                       AND ($3::TEXT IS NULL OR s.subject ILIKE '%' || $3 || '%' OR s.to_address ILIKE '%' || $3 || '%')
                    )
                ) as total"#,
            )
            .bind(user_id)
            .bind(alias_address)
            .bind(query)
            .fetch_one(pool)
            .await?;
            Ok(count)
        }
        DbPool::Sqlite(pool) => {
            let mut sql1 = String::from(
                "SELECT COUNT(*) FROM received_emails e JOIN aliases a ON e.alias_id = a.id JOIN domains d ON a.domain_id = d.id WHERE a.user_id = ? AND e.thread_id IS NULL AND e.is_favorite = 1",
            );
            if alias_address.is_some() {
                sql1.push_str(" AND (a.subdomain || '@' || d.name) = ?");
            }
            if query.is_some() {
                sql1.push_str(" AND (e.subject LIKE ? OR e.sender_email LIKE ?)");
            }

            let mut q1 = sqlx::query_scalar::<sqlx::Sqlite, i64>(&sql1).bind(user_id);
            if let Some(alias) = &alias_address {
                q1 = q1.bind(alias);
            }
            if let Some(search) = &query {
                let pattern = format!("%{}%", search);
                q1 = q1.bind(pattern.clone()).bind(pattern);
            }
            let c1 = q1.fetch_one(pool).await?;

            let mut sql2 = String::from(
                "SELECT COUNT(*) FROM sent_emails s JOIN aliases a ON s.from_alias_id = a.id JOIN domains d ON a.domain_id = d.id WHERE s.user_id = ? AND s.is_favorite = 1 AND s.status != 'draft'",
            );
            if alias_address.is_some() {
                sql2.push_str(" AND (a.subdomain || '@' || d.name) = ?");
            }
            if query.is_some() {
                sql2.push_str(" AND (s.subject LIKE ? OR s.to_address LIKE ?)");
            }

            let mut q2 = sqlx::query_scalar::<sqlx::Sqlite, i64>(&sql2).bind(user_id);
            if let Some(alias) = &alias_address {
                q2 = q2.bind(alias);
            }
            if let Some(search) = &query {
                let pattern = format!("%{}%", search);
                q2 = q2.bind(pattern.clone()).bind(pattern);
            }
            let c2 = q2.fetch_one(pool).await?;

            Ok(c1 + c2)
        }
    }
}
