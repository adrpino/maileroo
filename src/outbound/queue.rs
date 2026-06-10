use crate::db::DbPool;
use crate::outbound::OutboundService;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use time::OffsetDateTime;
use uuid::Uuid;

/// Resolves the deterministic EML file path inside storage from the job's Uuid
pub fn get_job_file_path(storage_dir: &Path, job_id: Uuid) -> PathBuf {
    storage_dir.join("outbound").join(format!("{}.eml", job_id))
}

/// Enqueues a transient failure into the outbound queue
pub async fn enqueue_job(
    pool: &DbPool,
    storage_dir: &Path,
    from_envelope: &str,
    to_recipient: &str,
    body_bytes: &[u8],
) -> anyhow::Result<Uuid> {
    let id = Uuid::new_v4();
    let file_path = get_job_file_path(storage_dir, id);

    // Create outbound storage directory if not exists
    if let Some(parent) = file_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Save physical file first to prevent dangling DB entries
    tokio::fs::write(&file_path, body_bytes).await?;

    if let Err(e) = crate::db::queue::insert_job(pool, id, from_envelope, to_recipient).await {
        // Rollback filesystem if database insert fails
        let _ = tokio::fs::remove_file(&file_path).await;
        return Err(anyhow::anyhow!(
            "Failed to register queue job in database: {}",
            e
        ));
    }

    Ok(id)
}

/// Computes the exponential backoff retry time capped at 24 hours (1440 minutes)
pub fn calculate_next_retry(attempts: i32) -> OffsetDateTime {
    let base_delay_mins = if attempts <= 0 {
        1
    } else {
        let exponent = (attempts - 1).min(10); // cap exponent to prevent overflow
        2u64.pow(exponent as u32) * 10
    };

    let delay_mins = base_delay_mins.min(1440);
    OffsetDateTime::now_utc() + time::Duration::minutes(delay_mins as i64)
}

/// Executed on a background schedule tick to process eligible queue jobs
pub async fn process_queue_tick(
    pool: &DbPool,
    storage_dir: &Path,
    outbound: Arc<OutboundService>,
) -> anyhow::Result<()> {
    let jobs = crate::db::queue::fetch_next_retryable_jobs(pool, 10).await?;
    if jobs.is_empty() {
        return Ok(());
    }

    for job in jobs {
        let job_id = job.id;

        // 1. Lock job state to 'sending' to prevent parallel worker pick-up
        if let Err(e) = crate::db::queue::update_job_status(
            pool,
            job_id,
            "sending",
            job.attempts,
            job.last_error.as_deref(),
            job.next_retry_at,
        )
        .await
        {
            tracing::error!("Failed to acquire lock for queue job {}: {}", job_id, e);
            continue;
        }

        // 2. Resolve file path and read payload
        let file_path = get_job_file_path(storage_dir, job_id);
        let body_bytes = match tokio::fs::read(&file_path).await {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::error!(
                    "Orphaned queue job {}: body file missing at {:?} ({})",
                    job_id,
                    file_path,
                    e
                );
                let _ = crate::db::queue::update_job_status(
                    pool,
                    job_id,
                    "failed",
                    job.attempts + 1,
                    Some("EML file missing from disk storage"),
                    OffsetDateTime::now_utc() + time::Duration::hours(24),
                )
                .await;
                continue;
            }
        };

        // 3. Retry delivery
        tracing::info!(
            "Queue job {} delivery attempt {}/{}...",
            job_id,
            job.attempts + 1,
            job.max_attempts
        );
        match outbound
            .send_raw(&job.to_recipient, &job.from_envelope, &body_bytes)
            .await
        {
            Ok(_) => {
                tracing::info!("Queue job {} successfully delivered!", job_id);
                let _ = crate::db::queue::delete_job(pool, job_id).await;
                let _ = tokio::fs::remove_file(&file_path).await;
            }
            Err(e) => {
                let err_msg = e.to_string();
                let next_attempt = job.attempts + 1;
                tracing::warn!("Queue job {} delivery attempt failed: {}", job_id, err_msg);

                if next_attempt >= job.max_attempts {
                    tracing::error!(
                        "Queue job {} has exceeded maximum retries. Marking as permanently failed.",
                        job_id
                    );
                    let _ = crate::db::queue::update_job_status(
                        pool,
                        job_id,
                        "failed",
                        next_attempt,
                        Some(&err_msg),
                        OffsetDateTime::now_utc() + time::Duration::days(365),
                    )
                    .await;
                } else {
                    let next_retry = calculate_next_retry(next_attempt);
                    let _ = crate::db::queue::update_job_status(
                        pool,
                        job_id,
                        "pending",
                        next_attempt,
                        Some(&err_msg),
                        next_retry,
                    )
                    .await;
                }
            }
        }
    }

    Ok(())
}

/// Starts the background Tokio daemon checking the queue on a periodic interval
pub fn start_queue_daemon(
    pool: DbPool,
    storage_dir: PathBuf,
    outbound: Arc<OutboundService>,
    tick_interval: std::time::Duration,
) {
    tokio::spawn(async move {
        // Use immediate/missed tick behavior that matches standard async scheduling
        let mut interval = tokio::time::interval(tick_interval);
        loop {
            interval.tick().await;
            tracing::debug!("Scanning outbound queue for eligible retryable jobs...");
            if let Err(e) = process_queue_tick(&pool, &storage_dir, outbound.clone()).await {
                tracing::error!("Error during outbound queue tick processing: {}", e);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn setup_sqlite_db() -> DbPool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        let db_pool = DbPool::Sqlite(pool);
        crate::db::run_migrations(&db_pool).await.unwrap();
        db_pool
    }

    #[test]
    fn test_exponential_backoff_math() {
        let base_time = OffsetDateTime::now_utc();

        // 0 attempts -> 1 minute delay
        let retry_0 = calculate_next_retry(0);
        assert!(retry_0 >= base_time + time::Duration::seconds(55));
        assert!(retry_0 <= base_time + time::Duration::seconds(65));

        // 1 attempt -> 10 minutes delay (2^0 * 10)
        let retry_1 = calculate_next_retry(1);
        assert!(retry_1 >= base_time + time::Duration::minutes(9));
        assert!(retry_1 <= base_time + time::Duration::minutes(11));

        // 3 attempts -> 40 minutes delay (2^2 * 10)
        let retry_3 = calculate_next_retry(3);
        assert!(retry_3 >= base_time + time::Duration::minutes(39));
        assert!(retry_3 <= base_time + time::Duration::minutes(41));

        // 12 attempts -> capped at 24 hours (1440 minutes)
        let retry_12 = calculate_next_retry(12);
        assert!(retry_12 >= base_time + time::Duration::hours(23));
        assert!(retry_12 <= base_time + time::Duration::hours(25));
    }

    #[tokio::test]
    async fn test_enqueue_and_cleanup_lifecycle() {
        let db = setup_sqlite_db().await;
        let temp_dir = tempdir().unwrap();
        let body = b"Subject: Unit Test\r\n\r\nHello delivery queue!";

        // 1. Enqueue job
        let job_id = enqueue_job(
            &db,
            temp_dir.path(),
            "sender@test.com",
            "rcpt@test.com",
            body,
        )
        .await
        .unwrap();

        // Assert DB entry exists by fetching it
        let jobs = crate::db::queue::fetch_next_retryable_jobs(&db, 10)
            .await
            .unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, job_id);
        assert_eq!(jobs[0].status, "pending");
        assert_eq!(jobs[0].from_envelope, "sender@test.com");
        assert_eq!(jobs[0].to_recipient, "rcpt@test.com");

        // Assert deterministic file exists on disk with identical body bytes
        let file_path = get_job_file_path(temp_dir.path(), job_id);
        assert!(file_path.exists());
        let read_bytes = tokio::fs::read(&file_path).await.unwrap();
        assert_eq!(read_bytes, body);

        // 2. Cleanup job
        crate::db::queue::delete_job(&db, job_id).await.unwrap();
        tokio::fs::remove_file(&file_path).await.unwrap();

        // Assert DB record is completely removed
        let jobs_after = crate::db::queue::fetch_next_retryable_jobs(&db, 10)
            .await
            .unwrap();
        assert!(jobs_after.is_empty());

        // Assert file is completely gone from storage
        assert!(!file_path.exists());
    }
}
