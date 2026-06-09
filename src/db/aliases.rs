use crate::db::{DbPool, Alias, AliasLookup};
use time::OffsetDateTime;
use uuid::Uuid;

pub async fn get_taken_subdomains(
    pool: &DbPool,
    domain_id: Uuid,
    candidates: &[String],
) -> Result<Vec<String>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let taken = sqlx::query_scalar::<_, String>(
                "SELECT subdomain FROM aliases WHERE domain_id = $1 AND subdomain = ANY($2)",
            )
            .bind(domain_id)
            .bind(candidates)
            .fetch_all(pool)
            .await?;
            Ok(taken)
        }
        DbPool::Sqlite(pool) => {
            let placeholders = vec!["?"; candidates.len()].join(", ");
            let sql = format!("SELECT subdomain FROM aliases WHERE domain_id = ? AND subdomain IN ({})", placeholders);
            let mut q = sqlx::query_scalar::<sqlx::Sqlite, String>(&sql).bind(domain_id);
            for cand in candidates {
                q = q.bind(cand);
            }
            let taken = q.fetch_all(pool).await?;
            Ok(taken)
        }
    }
}

pub async fn check_valid_subdomain(
    pool: &DbPool,
    subdomain: &str,
    parent_domain: &str,
) -> Result<Option<AliasLookup>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let result = sqlx::query_as::<_, AliasLookup>(
                r#"
                    SELECT 
                    a.id, 
                    a.destination_email,
                    a.auto_forward
                    FROM aliases a
                    JOIN domains d ON a.domain_id = d.id
                    WHERE a.subdomain = $1 AND d.name = $2 AND a.active = true AND d.active = true
                    "#,
            )
            .bind(subdomain)
            .bind(parent_domain)
            .fetch_optional(pool)
            .await?;
            Ok(result)
        }
        DbPool::Sqlite(pool) => {
            let result = sqlx::query_as::<sqlx::Sqlite, AliasLookup>(
                r#"
                SELECT a.id, a.destination_email, a.auto_forward
                FROM aliases a
                JOIN domains d ON a.domain_id = d.id
                WHERE a.subdomain = ? AND d.name = ? AND a.active = 1 AND d.active = 1
                "#,
            )
            .bind(subdomain)
            .bind(parent_domain)
            .fetch_optional(pool)
            .await?;
            Ok(result)
        }
    }
}

pub async fn get_aliases_by_user_id(
    pool: &DbPool,
    user_id: Uuid,
) -> Result<Vec<Alias>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let aliases = sqlx::query_as::<_, Alias>(
                r#"
                SELECT 
                    a.id, 
                    a.user_id, 
                    a.domain_id, 
                    a.subdomain, 
                    a.destination_email, 
                    a.auto_forward,
                    a.active, 
                    a.created_at, 
                    d.name as domain_name
                FROM aliases a
                JOIN domains d ON a.domain_id = d.id
                WHERE a.user_id = $1
                ORDER BY a.created_at DESC
                "#,
            )
            .bind(user_id)
            .fetch_all(pool)
            .await?;
            Ok(aliases)
        }
        DbPool::Sqlite(pool) => {
            let aliases = sqlx::query_as::<sqlx::Sqlite, Alias>(
                r#"
                SELECT 
                    a.id, 
                    a.user_id, 
                    a.domain_id, 
                    a.subdomain, 
                    a.destination_email, 
                    a.auto_forward,
                    a.active, 
                    a.created_at, 
                    d.name as domain_name
                FROM aliases a
                JOIN domains d ON a.domain_id = d.id
                WHERE a.user_id = ?
                ORDER BY a.created_at DESC
                "#,
            )
            .bind(user_id)
            .fetch_all(pool)
            .await?;
            Ok(aliases)
        }
    }
}

pub async fn insert_alias(
    pool: &DbPool,
    user_id: Uuid,
    domain_id: Uuid,
    subdomain: &str,
    destination_email: &str,
    auto_forward: bool,
) -> Result<Alias, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let alias = sqlx::query_as::<_, Alias>(
                r#"
                WITH inserted AS (
                    INSERT INTO aliases (id, user_id, domain_id, subdomain, destination_email, auto_forward, active, created_at)
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    RETURNING id, user_id, domain_id, subdomain, destination_email, auto_forward, active, created_at
                )
                SELECT i.*, d.name as domain_name
                FROM inserted i
                JOIN domains d ON i.domain_id = d.id
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(user_id)
            .bind(domain_id)
            .bind(subdomain)
            .bind(destination_email)
            .bind(auto_forward)
            .bind(true)
            .bind(OffsetDateTime::now_utc())
            .fetch_one(pool)
            .await?;
            Ok(alias)
        }
        DbPool::Sqlite(pool) => {
            let id = Uuid::new_v4();
            let created_at = OffsetDateTime::now_utc();
            
            // 1. Insert the alias
            sqlx::query(
                r#"
                INSERT INTO aliases (id, user_id, domain_id, subdomain, destination_email, auto_forward, active, created_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(id)
            .bind(user_id)
            .bind(domain_id)
            .bind(subdomain)
            .bind(destination_email)
            .bind(auto_forward)
            .bind(true)
            .bind(created_at)
            .execute(pool)
            .await?;

            // 2. Fetch the alias with domain_name joined
            let alias = sqlx::query_as::<sqlx::Sqlite, Alias>(
                r#"
                SELECT a.id, a.user_id, a.domain_id, a.subdomain, a.destination_email, a.auto_forward, a.active, a.created_at, d.name as domain_name
                FROM aliases a
                JOIN domains d ON a.domain_id = d.id
                WHERE a.id = ?
                "#,
            )
            .bind(id)
            .fetch_one(pool)
            .await?;

            Ok(alias)
        }
    }
}

pub async fn delete_alias_by_id(
    pool: &DbPool,
    alias_id: Uuid,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query(
                "DELETE FROM aliases WHERE id = $1 AND user_id = $2",
            )
            .bind(alias_id)
            .bind(user_id)
            .execute(pool)
            .await?;
            Ok(())
        }
        DbPool::Sqlite(pool) => {
            sqlx::query("DELETE FROM aliases WHERE id = ? AND user_id = ?")
                .bind(alias_id)
                .bind(user_id)
                .execute(pool)
                .await?;
            Ok(())
        }
    }
}

pub async fn is_subdomain_available(
    pool: &DbPool,
    domain_id: Uuid,
    subdomain: &str,
) -> Result<bool, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let exists = sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM aliases WHERE domain_id = $1 AND subdomain = $2)",
            )
            .bind(domain_id)
            .bind(subdomain)
            .fetch_one(pool)
            .await?;

            Ok(!exists)
        }
        DbPool::Sqlite(pool) => {
            let exists = sqlx::query_scalar::<sqlx::Sqlite, bool>(
                "SELECT EXISTS(SELECT 1 FROM aliases WHERE domain_id = ? AND subdomain = ?)",
            )
            .bind(domain_id)
            .bind(subdomain)
            .fetch_one(pool)
            .await?;

            Ok(!exists)
        }
    }
}

pub async fn get_alias_by_id_and_user(
    pool: &DbPool,
    alias_id: Uuid,
    user_id: Uuid,
) -> Result<Option<Alias>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query_as::<_, Alias>(
                r#"
                SELECT a.id, a.user_id, a.domain_id, a.subdomain, a.destination_email, 
                       a.auto_forward, a.active, a.created_at, d.name as domain_name
                FROM aliases a
                JOIN domains d ON a.domain_id = d.id
                WHERE a.id = $1 AND a.user_id = $2
                "#,
            )
            .bind(alias_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await
        }
        DbPool::Sqlite(pool) => {
            let alias = sqlx::query_as::<sqlx::Sqlite, Alias>(
                r#"
                SELECT a.id, a.user_id, a.domain_id, a.subdomain, a.destination_email, 
                       a.auto_forward, a.active, a.created_at, d.name as domain_name
                FROM aliases a
                JOIN domains d ON a.domain_id = d.id
                WHERE a.id = ? AND a.user_id = ?
                "#,
            )
            .bind(alias_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
            Ok(alias)
        }
    }
}

pub async fn resolve_recipient_alias(
    pool: &DbPool,
    local_part: &str,
    full_domain: &str,
) -> Result<Option<AliasLookup>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let result = sqlx::query_as::<_, AliasLookup>(
                r#"
                SELECT 
                    a.id, 
                    a.destination_email,
                    a.auto_forward
                FROM aliases a
                JOIN domains d ON a.domain_id = d.id
                WHERE 
                    (
                        (a.subdomain = $1 AND d.name = $2)
                        OR
                        (a.subdomain || '.' || d.name = $1 || '.' || $2)
                    )
                    AND a.active = true 
                    AND d.active = true
                LIMIT 1
                "#,
            )
            .bind(local_part)
            .bind(full_domain)
            .fetch_optional(pool)
            .await?;
            Ok(result)
        }
        DbPool::Sqlite(pool) => {
            let result = sqlx::query_as::<sqlx::Sqlite, AliasLookup>(
                r#"
                SELECT 
                    a.id, 
                    a.destination_email,
                    a.auto_forward
                FROM aliases a
                JOIN domains d ON a.domain_id = d.id
                WHERE 
                    (
                        (a.subdomain = ? AND d.name = ?)
                        OR
                        (a.subdomain || '.' || d.name = ? || '.' || ?)
                    )
                    AND a.active = 1 
                    AND d.active = 1
                LIMIT 1
                "#,
            )
            .bind(local_part)
            .bind(full_domain)
            .bind(local_part)
            .bind(full_domain)
            .fetch_optional(pool)
            .await?;
            Ok(result)
        }
    }
}

pub async fn update_alias_auto_forward(
    pool: &DbPool,
    alias_id: Uuid,
    user_id: Uuid,
    auto_forward: bool,
) -> Result<(), sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query(
                "UPDATE aliases SET auto_forward = $1 WHERE id = $2 AND user_id = $3",
            )
            .bind(auto_forward)
            .bind(alias_id)
            .bind(user_id)
            .execute(pool)
            .await?;
            Ok(())
        }
        DbPool::Sqlite(pool) => {
            sqlx::query("UPDATE aliases SET auto_forward = ? WHERE id = ? AND user_id = ?")
                .bind(auto_forward)
                .bind(alias_id)
                .bind(user_id)
                .execute(pool)
                .await?;
            Ok(())
        }
    }
}
