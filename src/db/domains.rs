use crate::db::DbPool;
use time::OffsetDateTime;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct Domain {
    pub id: uuid::Uuid,
    pub name: String,
    pub active: bool,
    pub dkim_private_key: Option<String>,
    pub dkim_public_key: Option<String>,
    pub dkim_selector: String,
    pub pending_dkim_private_key: Option<String>,
    pub pending_dkim_public_key: Option<String>,
    pub pending_dkim_selector: Option<String>,
    pub created_at: OffsetDateTime,
}

pub async fn insert_domain(pool: &DbPool, domain_name: &String) -> Result<Domain, sqlx::Error> {
    // Generate secure DKIM keys using our testable outbound module
    let (private_pem, public_dns) = crate::outbound::generate_dkim_key_pair()
        .map_err(|e| sqlx::Error::Protocol(format!("Failed to generate DKIM key pair: {}", e)))?;

    // Encrypt the private key before persisting
    let encrypted_private_pem = crate::crypto::encrypt(&private_pem)
        .map_err(|e| sqlx::Error::Protocol(format!("Failed to encrypt DKIM key: {}", e)))?;

    match pool {
        DbPool::Postgres(pool) => {
            let domain = sqlx::query_as::<_, Domain>(
                r#"
                INSERT INTO domains (
                    id, name, active, 
                    dkim_private_key, dkim_public_key, dkim_selector, 
                    pending_dkim_private_key, pending_dkim_public_key, pending_dkim_selector,
                    created_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
                RETURNING id, name, active, dkim_private_key, dkim_public_key, dkim_selector, pending_dkim_private_key, pending_dkim_public_key, pending_dkim_selector, created_at"#,
            )
            .bind(uuid::Uuid::new_v4())
            .bind(domain_name)
            .bind(true)
            .bind(encrypted_private_pem)
            .bind(public_dns)
            .bind("maileroo")
            .bind(None::<String>)
            .bind(None::<String>)
            .bind(None::<String>)
            .bind(OffsetDateTime::now_utc())
            .fetch_one(pool)
            .await?;
            Ok(domain)
        }
        DbPool::Sqlite(pool) => {
            let id = uuid::Uuid::new_v4();
            let created_at = OffsetDateTime::now_utc();
            let domain = sqlx::query_as::<sqlx::Sqlite, Domain>(
                r#"
                INSERT INTO domains (
                    id, name, active, 
                    dkim_private_key, dkim_public_key, dkim_selector, 
                    pending_dkim_private_key, pending_dkim_public_key, pending_dkim_selector,
                    created_at
                )
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                RETURNING id, name, active, dkim_private_key, dkim_public_key, dkim_selector, pending_dkim_private_key, pending_dkim_public_key, pending_dkim_selector, created_at"#,
            )
            .bind(id)
            .bind(domain_name)
            .bind(true)
            .bind(encrypted_private_pem)
            .bind(public_dns)
            .bind("maileroo")
            .bind(None::<String>)
            .bind(None::<String>)
            .bind(None::<String>)
            .bind(created_at)
            .fetch_one(pool)
            .await?;
            Ok(domain)
        }
    }
}

pub async fn get_domains(pool: &DbPool) -> Result<Vec<Domain>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let domains = sqlx::query_as::<_, Domain>(
                r#"
                SELECT
                id, name, active, 
                dkim_private_key, dkim_public_key, dkim_selector, 
                pending_dkim_private_key, pending_dkim_public_key, pending_dkim_selector,
                created_at
                from domains
                "#
            )
            .fetch_all(pool)
            .await?;
            Ok(domains)
        }
        DbPool::Sqlite(pool) => {
            let domains = sqlx::query_as::<sqlx::Sqlite, Domain>(
                r#"
                SELECT 
                id, name, active, 
                dkim_private_key, dkim_public_key, dkim_selector, 
                pending_dkim_private_key, pending_dkim_public_key, pending_dkim_selector,
                created_at 
                FROM domains
                "#,
            )
            .fetch_all(pool)
            .await?;
            Ok(domains)
        }
    }
}

pub async fn get_dkim_key_by_domain(
    pool: &DbPool,
    domain_name: &str,
) -> Result<Option<(Option<String>, String)>, sqlx::Error> {
    let dkim_opt = match pool {
        DbPool::Postgres(p) => {
            sqlx::query_as::<_, (Option<String>, String)>(
                "SELECT dkim_private_key, dkim_selector FROM domains WHERE name = $1 AND active = true"
            )
            .bind(domain_name)
            .fetch_optional(p)
            .await?
        }
        DbPool::Sqlite(p) => {
            sqlx::query_as::<sqlx::Sqlite, (Option<String>, String)>(
                "SELECT dkim_private_key, dkim_selector FROM domains WHERE name = ? AND active = true"
            )
            .bind(domain_name)
            .fetch_optional(p)
            .await?
        }
    };

    if let Some((encrypted_key, selector)) = dkim_opt {
        let decrypted_key = if let Some(key) = encrypted_key {
            match crate::crypto::decrypt(&key) {
                Ok(plain) => Some(plain),
                Err(e) => {
                    tracing::error!("Failed to decrypt DKIM private key for domain {}: {}", domain_name, e);
                    return Err(sqlx::Error::Protocol(format!("Failed to decrypt DKIM key: {}", e)));
                }
            }
        } else {
            None
        };
        Ok(Some((decrypted_key, selector)))
    } else {
        Ok(None)
    }
}

pub async fn get_domain_count(pool: &DbPool) -> Result<i64, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM domains")
                .fetch_one(pool)
                .await?;
            Ok(count)
        }
        DbPool::Sqlite(pool) => {
            let count = sqlx::query_scalar::<sqlx::Sqlite, i64>("SELECT COUNT(*) FROM domains")
                .fetch_one(pool)
                .await?;
            Ok(count)
        }
    }
}

pub async fn delete_domain_by_id(pool: &DbPool, domain_id: uuid::Uuid) -> Result<(), sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query("DELETE FROM domains WHERE id = $1")
                .bind(domain_id)
                .execute(pool)
                .await?;
            Ok(())
        }
        DbPool::Sqlite(pool) => {
            sqlx::query("DELETE FROM domains WHERE id = ?")
                .bind(domain_id)
                .execute(pool)
                .await?;
            Ok(())
        }
    }
}

pub async fn get_domain_by_id(pool: &DbPool, id: uuid::Uuid) -> Result<Option<Domain>, sqlx::Error> {
    match pool {
        DbPool::Postgres(p) => {
            let domain = sqlx::query_as::<_, Domain>(
                r#"
                SELECT id, name, active, dkim_private_key, dkim_public_key, dkim_selector,
                       pending_dkim_private_key, pending_dkim_public_key, pending_dkim_selector, created_at
                FROM domains WHERE id = $1
                "#
            )
            .bind(id)
            .fetch_optional(p)
            .await?;
            Ok(domain)
        }
        DbPool::Sqlite(p) => {
            let domain = sqlx::query_as::<sqlx::Sqlite, Domain>(
                r#"
                SELECT id, name, active, dkim_private_key, dkim_public_key, dkim_selector,
                       pending_dkim_private_key, pending_dkim_public_key, pending_dkim_selector, created_at
                FROM domains WHERE id = ?
                "#
            )
            .bind(id)
            .fetch_optional(p)
            .await?;
            Ok(domain)
        }
    }
}

pub async fn update_pending_dkim(
    pool: &DbPool,
    id: uuid::Uuid,
    private_key: Option<String>,
    public_key: Option<String>,
    selector: Option<String>,
) -> Result<(), sqlx::Error> {
    match pool {
        DbPool::Postgres(p) => {
            sqlx::query(
                r#"
                UPDATE domains
                SET pending_dkim_private_key = $1,
                    pending_dkim_public_key = $2,
                    pending_dkim_selector = $3
                WHERE id = $4
                "#
            )
            .bind(private_key)
            .bind(public_key)
            .bind(selector)
            .bind(id)
            .execute(p)
            .await?;
            Ok(())
        }
        DbPool::Sqlite(p) => {
            sqlx::query(
                r#"
                UPDATE domains
                SET pending_dkim_private_key = ?,
                    pending_dkim_public_key = ?,
                    pending_dkim_selector = ?
                WHERE id = ?
                "#
            )
            .bind(private_key)
            .bind(public_key)
            .bind(selector)
            .bind(id)
            .execute(p)
            .await?;
            Ok(())
        }
    }
}

pub async fn promote_pending_dkim(pool: &DbPool, id: uuid::Uuid) -> Result<(), sqlx::Error> {
    match pool {
        DbPool::Postgres(p) => {
            sqlx::query(
                r#"
                UPDATE domains
                SET dkim_private_key = pending_dkim_private_key,
                    dkim_public_key = pending_dkim_public_key,
                    dkim_selector = COALESCE(pending_dkim_selector, dkim_selector),
                    pending_dkim_private_key = NULL,
                    pending_dkim_public_key = NULL,
                    pending_dkim_selector = NULL
                WHERE id = $1
                "#
            )
            .bind(id)
            .execute(p)
            .await?;
            Ok(())
        }
        DbPool::Sqlite(p) => {
            sqlx::query(
                r#"
                UPDATE domains
                SET dkim_private_key = pending_dkim_private_key,
                    dkim_public_key = pending_dkim_public_key,
                    dkim_selector = COALESCE(pending_dkim_selector, dkim_selector),
                    pending_dkim_private_key = NULL,
                    pending_dkim_public_key = NULL,
                    pending_dkim_selector = NULL
                WHERE id = ?
                "#
            )
            .bind(id)
            .execute(p)
            .await?;
            Ok(())
        }
    }
}

pub async fn clear_pending_dkim(pool: &DbPool, id: uuid::Uuid) -> Result<(), sqlx::Error> {
    match pool {
        DbPool::Postgres(p) => {
            sqlx::query(
                r#"
                UPDATE domains
                SET pending_dkim_private_key = NULL,
                    pending_dkim_public_key = NULL,
                    pending_dkim_selector = NULL
                WHERE id = $1
                "#
            )
            .bind(id)
            .execute(p)
            .await?;
            Ok(())
        }
        DbPool::Sqlite(p) => {
            sqlx::query(
                r#"
                UPDATE domains
                SET pending_dkim_private_key = NULL,
                    pending_dkim_public_key = NULL,
                    pending_dkim_selector = NULL
                WHERE id = ?
                "#
            )
            .bind(id)
            .execute(p)
            .await?;
            Ok(())
        }
    }
}
