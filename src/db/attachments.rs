use time::OffsetDateTime;
use uuid::Uuid;

use crate::db::DbPool;
use crate::web::ReceivedEmail;
use crate::inbound::parser::AttachmentMetadata;

#[derive(sqlx::FromRow, serde::Serialize, Debug, Clone)]
pub struct AttachmentRow {
    pub id: Uuid,
    pub email_id: Uuid,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub size_bytes: i64,
    pub part_index: i32,
    pub is_inline: bool,
    pub content_id: Option<String>,
    pub created_at: OffsetDateTime,
}

pub async fn get_attachments_for_email(
    pool: &DbPool,
    email_id: Uuid,
) -> Result<Vec<AttachmentRow>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let attachments = sqlx::query_as::<_, AttachmentRow>(
                r#"SELECT id, email_id, filename, content_type, size_bytes, part_index, is_inline, content_id, created_at
                   FROM attachments
                   WHERE email_id = $1
                   ORDER BY part_index ASC"#,
            )
            .bind(email_id)
            .fetch_all(pool)
            .await?;
            Ok(attachments)
        }
        DbPool::Sqlite(pool) => {
            let attachments = sqlx::query_as::<sqlx::Sqlite, AttachmentRow>(
                r#"SELECT id, email_id, filename, content_type, size_bytes, part_index, is_inline, content_id, created_at
                   FROM attachments
                   WHERE email_id = ?
                   ORDER BY part_index ASC"#,
            )
            .bind(email_id)
            .fetch_all(pool)
            .await?;
            Ok(attachments)
        }
    }
}

pub async fn get_attachment_by_id(
    pool: &DbPool,
    attachment_id: Uuid,
) -> Result<Option<AttachmentRow>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let attachment = sqlx::query_as::<_, AttachmentRow>(
                r#"SELECT id, email_id, filename, content_type, size_bytes, part_index, is_inline, content_id, created_at
                   FROM attachments
                   WHERE id = $1"#,
            )
            .bind(attachment_id)
            .fetch_optional(pool)
            .await?;
            Ok(attachment)
        }
        DbPool::Sqlite(pool) => {
            let attachment = sqlx::query_as::<sqlx::Sqlite, AttachmentRow>(
                r#"SELECT id, email_id, filename, content_type, size_bytes, part_index, is_inline, content_id, created_at
                   FROM attachments
                   WHERE id = ?"#,
            )
            .bind(attachment_id)
            .fetch_optional(pool)
            .await?;
            Ok(attachment)
        }
    }
}

pub async fn insert_email_with_attachments(
    pool: &DbPool,
    alias_id: Uuid,
    sender: &str,
    subject: &str,
    body_key: Uuid,
    received_at: Option<OffsetDateTime>,
    message_id: Option<String>,
    thread_id: Option<Uuid>,
    attachments: &[AttachmentMetadata],
) -> Result<ReceivedEmail, sqlx::Error> {
    let received_at_val = received_at.unwrap_or_else(OffsetDateTime::now_utc);
    let has_attachments = attachments.iter().any(|a| !a.is_inline);
    let email_id = Uuid::new_v4();

    match pool {
        DbPool::Postgres(pool) => {
            let mut tx = pool.begin().await?;
            if let Some(root_id) = thread_id {
                sqlx::query("UPDATE received_emails SET last_activity_at = $1 WHERE id = $2")
                    .bind(received_at_val)
                    .bind(root_id)
                    .execute(&mut *tx)
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
                        thread_id,
                        has_attachments
                         ) VALUES ($1, $2, $3, $4, $5, $6, $6, false, $7, $8, $9)
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
            .bind(email_id)
            .bind(alias_id)
            .bind(sender)
            .bind(subject)
            .bind(body_key)
            .bind(received_at_val)
            .bind(message_id)
            .bind(thread_id)
            .bind(has_attachments)
            .fetch_one(&mut *tx)
            .await?;

            for att in attachments {
                sqlx::query(
                    r#"INSERT INTO attachments (
                        id, email_id, filename, content_type, size_bytes, part_index, is_inline, content_id, created_at
                    ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
                )
                .bind(Uuid::new_v4())
                .bind(email_id)
                .bind(&att.filename)
                .bind(&att.content_type)
                .bind(att.size_bytes)
                .bind(att.part_index)
                .bind(att.is_inline)
                .bind(&att.content_id)
                .bind(received_at_val)
                .execute(&mut *tx)
                .await?;
            }

            tx.commit().await?;
            Ok(email)
        }
        DbPool::Sqlite(pool) => {
            let mut tx = pool.begin().await?;
            if let Some(root_id) = thread_id {
                sqlx::query("UPDATE received_emails SET last_activity_at = ? WHERE id = ?")
                    .bind(received_at_val)
                    .bind(root_id)
                    .execute(&mut *tx)
                    .await?;
            }

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
                    thread_id,
                    has_attachments
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(email_id)
            .bind(alias_id)
            .bind(sender)
            .bind(subject)
            .bind(body_key)
            .bind(received_at_val)
            .bind(received_at_val)
            .bind(false)
            .bind(message_id)
            .bind(thread_id)
            .bind(has_attachments)
            .execute(&mut *tx)
            .await?;

            for att in attachments {
                sqlx::query(
                    r#"INSERT INTO attachments (
                        id, email_id, filename, content_type, size_bytes, part_index, is_inline, content_id, created_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
                )
                .bind(Uuid::new_v4())
                .bind(email_id)
                .bind(&att.filename)
                .bind(&att.content_type)
                .bind(att.size_bytes)
                .bind(att.part_index)
                .bind(att.is_inline)
                .bind(&att.content_id)
                .bind(received_at_val)
                .execute(&mut *tx)
                .await?;
            }

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
                    e.has_attachments
                FROM received_emails e
                JOIN aliases a ON e.alias_id = a.id
                JOIN domains d ON a.domain_id = d.id
                WHERE e.id = ?
                "#,
            )
            .bind(email_id)
            .fetch_one(&mut *tx)
            .await?;

            tx.commit().await?;
            Ok(email)
        }
    }
}