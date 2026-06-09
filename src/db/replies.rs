use crate::db::DbPool;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct EmailReply {
    pub id: Uuid,
    pub email_id: Uuid,
    pub body_text: String,
    pub sent_at: OffsetDateTime,
    pub message_id: Option<String>,
}

pub async fn insert_reply(
    pool: &DbPool,
    email_id: Uuid,
    body_text: &str,
    message_id: Option<String>,
) -> Result<EmailReply, sqlx::Error> {
    let now = OffsetDateTime::now_utc();

    match pool {
        DbPool::Postgres(pool) => {
            // Update the last_activity_at of the parent email
            sqlx::query(
                "UPDATE received_emails SET last_activity_at = $1 WHERE id = $2",
            )
            .bind(now)
            .bind(email_id)
            .execute(pool)
            .await?;

            sqlx::query_as::<_, EmailReply>(
                r#"INSERT INTO email_replies (id, email_id, body_text, sent_at, message_id)
                   VALUES ($1, $2, $3, $4, $5) RETURNING id, email_id, body_text, sent_at, message_id"#,
            )
            .bind(Uuid::new_v4())
            .bind(email_id)
            .bind(body_text)
            .bind(now)
            .bind(message_id)
            .fetch_one(pool)
            .await
        }
        DbPool::Sqlite(pool) => {
            // Update the last_activity_at of the parent email
            sqlx::query("UPDATE received_emails SET last_activity_at = ? WHERE id = ?")
                .bind(now)
                .bind(email_id)
                .execute(pool)
                .await?;

            let id = Uuid::new_v4();
            sqlx::query_as::<sqlx::Sqlite, EmailReply>(
                r#"INSERT INTO email_replies (id, email_id, body_text, sent_at, message_id)
                   VALUES (?, ?, ?, ?, ?) 
                   RETURNING id, email_id, body_text, sent_at, message_id"#,
            )
            .bind(id)
            .bind(email_id)
            .bind(body_text)
            .bind(now)
            .bind(message_id)
            .fetch_one(pool)
            .await
        }
    }
}

pub async fn get_replies_for_email(
    pool: &DbPool,
    email_id: Uuid,
) -> Result<Vec<EmailReply>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query_as::<_, EmailReply>(
                "SELECT id, email_id, body_text, sent_at, message_id FROM email_replies WHERE email_id = $1 ORDER BY sent_at ASC",
            )
            .bind(email_id)
            .fetch_all(pool)
            .await
        }
        DbPool::Sqlite(pool) => {
            sqlx::query_as::<sqlx::Sqlite, EmailReply>(
                "SELECT id, email_id, body_text, sent_at, message_id FROM email_replies WHERE email_id = ? ORDER BY sent_at ASC",
            )
            .bind(email_id)
            .fetch_all(pool)
            .await
        }
    }
}
