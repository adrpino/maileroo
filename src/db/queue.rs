use crate::db::DbPool;
use time::OffsetDateTime;
use uuid::Uuid;

fn format_rfc3339(dt: &OffsetDateTime) -> Result<String, sqlx::Error> {
    dt.format(&time::format_description::well_known::Rfc3339)
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct QueueJob {
    pub id: Uuid,
    pub from_envelope: String,
    pub to_recipient: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    pub status: String,
    pub next_retry_at: OffsetDateTime,
    pub created_at: OffsetDateTime,
}

pub async fn insert_job(pool: &DbPool, id: Uuid, from: &str, to: &str) -> Result<(), sqlx::Error> {
    let now = OffsetDateTime::now_utc();
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query(
                r#"
                INSERT INTO outbound_queue (id, from_envelope, to_recipient, next_retry_at, created_at)
                VALUES ($1, $2, $3, $4, $5)
                "#,
            )
            .bind(id)
            .bind(from)
            .bind(to)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await?;
        }
        DbPool::Sqlite(pool) => {
            sqlx::query(
                r#"
                INSERT INTO outbound_queue (id, from_envelope, to_recipient, next_retry_at, created_at)
                VALUES (?, ?, ?, ?, ?)
                "#,
            )
            .bind(id)
            .bind(from)
            .bind(to)
            .bind(now)
            .bind(now)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

pub async fn fetch_next_retryable_jobs(
    pool: &DbPool,
    limit: i64,
) -> Result<Vec<QueueJob>, sqlx::Error> {
    let now = OffsetDateTime::now_utc();
    match pool {
        DbPool::Postgres(pool) => {
            let jobs = sqlx::query_as::<_, QueueJob>(
                r#"
                SELECT id, from_envelope, to_recipient, attempts, max_attempts, last_error, status, next_retry_at, created_at
                FROM outbound_queue
                WHERE status = 'pending' AND next_retry_at <= $1
                ORDER BY next_retry_at ASC
                LIMIT $2
                "#,
            )
            .bind(now)
            .bind(limit)
            .fetch_all(pool)
            .await?;
            Ok(jobs)
        }
        DbPool::Sqlite(pool) => {
            let now_str = format_rfc3339(&now)?;
            let jobs = sqlx::query_as::<sqlx::Sqlite, QueueJob>(
                r#"
                SELECT id, from_envelope, to_recipient, attempts, max_attempts, last_error, status, next_retry_at, created_at
                FROM outbound_queue
                WHERE status = 'pending' AND next_retry_at <= ?
                ORDER BY next_retry_at ASC
                LIMIT ?
                "#,
            )
            .bind(now_str)
            .bind(limit)
            .fetch_all(pool)
            .await?;
            Ok(jobs)
        }
    }
}

pub async fn update_job_status(
    pool: &DbPool,
    id: Uuid,
    status: &str,
    attempts: i32,
    last_error: Option<&str>,
    next_retry_at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query(
                r#"
                UPDATE outbound_queue
                SET status = $1, attempts = $2, last_error = $3, next_retry_at = $4
                WHERE id = $5
                "#,
            )
            .bind(status)
            .bind(attempts)
            .bind(last_error)
            .bind(next_retry_at)
            .bind(id)
            .execute(pool)
            .await?;
        }
        DbPool::Sqlite(pool) => {
            sqlx::query(
                r#"
                UPDATE outbound_queue
                SET status = ?, attempts = ?, last_error = ?, next_retry_at = ?
                WHERE id = ?
                "#,
            )
            .bind(status)
            .bind(attempts)
            .bind(last_error)
            .bind(next_retry_at)
            .bind(id)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

pub async fn delete_job(pool: &DbPool, id: Uuid) -> Result<(), sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query(
                r#"
                DELETE FROM outbound_queue WHERE id = $1
                "#,
            )
            .bind(id)
            .execute(pool)
            .await?;
        }
        DbPool::Sqlite(pool) => {
            sqlx::query(
                r#"
                DELETE FROM outbound_queue WHERE id = ?
                "#,
            )
            .bind(id)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}
