use crate::db::DbPool;
use time::OffsetDateTime;

#[allow(dead_code)]
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReplyMapping {
    pub id: uuid::Uuid,
    pub alias_id: uuid::Uuid,
    pub original_sender: String,
    pub anonymous_token: String,
    pub created_at: OffsetDateTime,
}

#[allow(dead_code)]
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReplyMappingLookup {
    pub id: uuid::Uuid,
    pub alias_id: uuid::Uuid,
    pub original_sender: String,
    pub anonymous_token: String,
    pub created_at: OffsetDateTime,
    pub destination_email: String,
    pub alias_subdomain: String,
    pub domain_name: String,
}

pub async fn get_or_create_reply_mapping(
    pool: &DbPool,
    alias_id: uuid::Uuid,
    original_sender: &str,
) -> Result<ReplyMapping, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            // 1. Try to find an existing mapping first
            let existing = sqlx::query_as::<_, ReplyMapping>(
                r#"
                SELECT id, alias_id, original_sender, anonymous_token, created_at
                FROM reply_mappings
                WHERE alias_id = $1 AND original_sender = $2
                "#,
            )
            .bind(alias_id)
            .bind(original_sender)
            .fetch_optional(pool)
            .await?;

            if let Some(mapping) = existing {
                return Ok(mapping);
            }

            // 2. Generate a secure, unique anonymous token: "reply-<uuid-hex>"
            let token = format!("reply-{}", uuid::Uuid::new_v4().simple());

            // 3. Insert and return the new mapping
            let mapping = sqlx::query_as::<_, ReplyMapping>(
                r#"
                INSERT INTO reply_mappings (id, alias_id, original_sender, anonymous_token, created_at)
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT (alias_id, original_sender) DO UPDATE SET alias_id = EXCLUDED.alias_id
                RETURNING id, alias_id, original_sender, anonymous_token, created_at
                "#,
            )
            .bind(uuid::Uuid::new_v4())
            .bind(alias_id)
            .bind(original_sender)
            .bind(token)
            .bind(OffsetDateTime::now_utc())
            .fetch_one(pool)
            .await?;

            Ok(mapping)
        }
        DbPool::Sqlite(pool) => {
            // 1. Try to find an existing mapping first
            let existing = sqlx::query_as::<sqlx::Sqlite, ReplyMapping>(
                r#"
                SELECT id, alias_id, original_sender, anonymous_token, created_at
                FROM reply_mappings
                WHERE alias_id = ? AND original_sender = ?
                "#,
            )
            .bind(alias_id)
            .bind(original_sender)
            .fetch_optional(pool)
            .await?;

            if let Some(mapping) = existing {
                return Ok(mapping);
            }

            // 2. Generate a secure, unique anonymous token: "reply-<uuid-hex>"
            let token = format!("reply-{}", uuid::Uuid::new_v4().simple());

            // 3. Insert and return the new mapping
            let id = uuid::Uuid::new_v4();
            let created_at = OffsetDateTime::now_utc();
            let mapping = sqlx::query_as::<sqlx::Sqlite, ReplyMapping>(
                r#"
                INSERT INTO reply_mappings (id, alias_id, original_sender, anonymous_token, created_at)
                VALUES (?, ?, ?, ?, ?)
                ON CONFLICT (alias_id, original_sender) DO UPDATE SET alias_id = EXCLUDED.alias_id
                RETURNING id, alias_id, original_sender, anonymous_token, created_at
                "#,
            )
            .bind(id)
            .bind(alias_id)
            .bind(original_sender)
            .bind(token)
            .bind(created_at)
            .fetch_one(pool)
            .await?;

            Ok(mapping)
        }
    }
}

pub async fn get_reply_mapping_by_token(
    pool: &DbPool,
    token: &str,
) -> Result<Option<ReplyMappingLookup>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let mapping = sqlx::query_as::<_, ReplyMappingLookup>(
                r#"
                SELECT 
                    rm.id, 
                    rm.alias_id, 
                    rm.original_sender, 
                    rm.anonymous_token, 
                    rm.created_at,
                    a.destination_email,
                    a.subdomain as alias_subdomain,
                    d.name as domain_name
                FROM reply_mappings rm
                JOIN aliases a ON rm.alias_id = a.id
                JOIN domains d ON a.domain_id = d.id
                WHERE rm.anonymous_token = $1 AND a.active = true AND d.active = true
                "#,
            )
            .bind(token)
            .fetch_optional(pool)
            .await?;

            Ok(mapping)
        }
        DbPool::Sqlite(pool) => {
            let mapping = sqlx::query_as::<sqlx::Sqlite, ReplyMappingLookup>(
                r#"
                SELECT 
                    rm.id, 
                    rm.alias_id, 
                    rm.original_sender, 
                    rm.anonymous_token, 
                    rm.created_at,
                    a.destination_email,
                    a.subdomain as alias_subdomain,
                    d.name as domain_name
                FROM reply_mappings rm
                JOIN aliases a ON rm.alias_id = a.id
                JOIN domains d ON a.domain_id = d.id
                WHERE rm.anonymous_token = ? AND a.active = 1 AND d.active = 1
                "#,
            )
            .bind(token)
            .fetch_optional(pool)
            .await?;

            Ok(mapping)
        }
    }
}
