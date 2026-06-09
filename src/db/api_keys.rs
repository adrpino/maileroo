use crate::db::DbPool;
use crate::db::users::User;
use time::OffsetDateTime;

#[derive(Clone, Debug, sqlx::FromRow)]
pub struct ApiKey {
    pub id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub key_hash: String,
    pub name: String,
    pub created_at: OffsetDateTime,
}

pub async fn insert_api_key(
    pool: &DbPool,
    user_id: uuid::Uuid,
    key_hash: &str,
    name: &str,
) -> Result<ApiKey, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let api_key = sqlx::query_as::<_, ApiKey>(
                r#"
                INSERT INTO api_keys (id, user_id, key_hash, name, created_at)
                VALUES ($1, $2, $3, $4, $5)
                RETURNING id, user_id, key_hash, name, created_at
                "#,
            )
            .bind(uuid::Uuid::new_v4())
            .bind(user_id)
            .bind(key_hash)
            .bind(name)
            .bind(OffsetDateTime::now_utc())
            .fetch_one(pool)
            .await?;
            Ok(api_key)
        }
        DbPool::Sqlite(pool) => {
            let id = uuid::Uuid::new_v4();
            let created_at = OffsetDateTime::now_utc();
            let api_key = sqlx::query_as::<sqlx::Sqlite, ApiKey>(
                r#"
                INSERT INTO api_keys (id, user_id, key_hash, name, created_at)
                VALUES (?, ?, ?, ?, ?)
                RETURNING id, user_id, key_hash, name, created_at
                "#,
            )
            .bind(id)
            .bind(user_id)
            .bind(key_hash)
            .bind(name)
            .bind(created_at)
            .fetch_one(pool)
            .await?;
            Ok(api_key)
        }
    }
}

pub async fn get_api_keys(pool: &DbPool, user_id: uuid::Uuid) -> Result<Vec<ApiKey>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let keys = sqlx::query_as::<_, ApiKey>(
                "SELECT id, user_id, key_hash, name, created_at FROM api_keys WHERE user_id = $1 ORDER BY created_at DESC",
            )
            .bind(user_id)
            .fetch_all(pool)
            .await?;
            Ok(keys)
        }
        DbPool::Sqlite(pool) => {
            let keys = sqlx::query_as::<sqlx::Sqlite, ApiKey>(
                "SELECT id, user_id, key_hash, name, created_at FROM api_keys WHERE user_id = ? ORDER BY created_at DESC",
            )
            .bind(user_id)
            .fetch_all(pool)
            .await?;
            Ok(keys)
        }
    }
}

pub async fn delete_api_key(
    pool: &DbPool,
    key_id: uuid::Uuid,
    user_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query(
                "DELETE FROM api_keys WHERE id = $1 AND user_id = $2",
            )
            .bind(key_id)
            .bind(user_id)
            .execute(pool)
            .await?;
        }
        DbPool::Sqlite(pool) => {
            sqlx::query("DELETE FROM api_keys WHERE id = ? AND user_id = ?")
                .bind(key_id)
                .bind(user_id)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn get_user_by_api_key_hash(
    pool: &DbPool,
    key_hash: &str,
) -> Result<Option<User>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let user = sqlx::query_as::<_, User>(
                r#"
                SELECT u.id, u.email, u.password_hash, u.is_admin, u.bypass_alias_limit, u.disable_autoclean, u.can_send_firsthand, u.created_at, u.updated_at, u.last_login_at, u.last_login_ip
                FROM users u
                JOIN api_keys ak ON ak.user_id = u.id
                WHERE ak.key_hash = $1
                "#,
            )
            .bind(key_hash)
            .fetch_optional(pool)
            .await?;
            Ok(user)
        }
        DbPool::Sqlite(pool) => {
            let user = sqlx::query_as::<sqlx::Sqlite, User>(
                r#"
                SELECT u.id, u.email, u.password_hash, u.is_admin, u.bypass_alias_limit, u.disable_autoclean, u.can_send_firsthand, u.created_at, u.updated_at, u.last_login_at, u.last_login_ip
                FROM users u
                JOIN api_keys ak ON ak.user_id = u.id
                WHERE ak.key_hash = ?
                "#,
            )
            .bind(key_hash)
            .fetch_optional(pool)
            .await?;
            Ok(user)
        }
    }
}
