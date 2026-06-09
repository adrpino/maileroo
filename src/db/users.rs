use crate::db::DbPool;
use time::OffsetDateTime;
use uuid::Uuid;
use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
};

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub password_hash: String,
    pub is_admin: bool,
    pub bypass_alias_limit: bool,
    pub disable_autoclean: bool,
    pub can_send_firsthand: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub last_login_at: Option<OffsetDateTime>,
    pub last_login_ip: Option<String>,
}

pub async fn update_last_login(
    pool: &DbPool,
    user_id: Uuid,
    client_ip: Option<String>,
) -> Result<(), sqlx::Error> {
    let now = OffsetDateTime::now_utc();
    match pool {
        DbPool::Postgres(pool) => {
            sqlx::query(
                "UPDATE users SET last_login_at = $1, last_login_ip = $2 WHERE id = $3",
            )
            .bind(now)
            .bind(client_ip)
            .bind(user_id)
            .execute(pool)
            .await?;
        }
        DbPool::Sqlite(pool) => {
            sqlx::query("UPDATE users SET last_login_at = ?, last_login_ip = ? WHERE id = ?")
                .bind(now)
                .bind(client_ip)
                .bind(user_id)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn get_user_by_email(pool: &DbPool, email: &str) -> Result<Option<User>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let user = sqlx::query_as::<_, User>(
                r#"
                SELECT
                    id,
                    email,
                    password_hash,
                    is_admin,
                    bypass_alias_limit,
                    disable_autoclean,
                    can_send_firsthand,
                    created_at,
                    updated_at,
                    last_login_at,
                    last_login_ip
                FROM users where email = $1
                "#,
            )
            .bind(email)
            .fetch_optional(pool)
            .await?;
            Ok(user)
        }
        DbPool::Sqlite(pool) => {
            let user = sqlx::query_as::<sqlx::Sqlite, User>(
                r#"
                SELECT
                    id,
                    email,
                    password_hash,
                    is_admin,
                    bypass_alias_limit,
                    disable_autoclean,
                    can_send_firsthand,
                    created_at,
                    updated_at,
                    last_login_at,
                    last_login_ip
                FROM users where email = ?
                "#,
            )
            .bind(email)
            .fetch_optional(pool)
            .await?;
            Ok(user)
        }
    }
}

pub async fn get_user_by_id(
    pool: &DbPool,
    user_id: uuid::Uuid,
) -> Result<Option<User>, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let user = sqlx::query_as::<_, User>(
                r#"
                SELECT
                    id,
                    email,
                    password_hash,
                    is_admin,
                    bypass_alias_limit,
                    disable_autoclean,
                    can_send_firsthand,
                    created_at,
                    updated_at,
                    last_login_at,
                    last_login_ip
                FROM users where id = $1
                "#,
            )
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
            Ok(user)
        }
        DbPool::Sqlite(pool) => {
            let user = sqlx::query_as::<sqlx::Sqlite, User>(
                r#"
                SELECT
                    id,
                    email,
                    password_hash,
                    is_admin,
                    bypass_alias_limit,
                    disable_autoclean,
                    can_send_firsthand,
                    created_at,
                    updated_at,
                    last_login_at,
                    last_login_ip
                FROM users where id = ?
                "#,
            )
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
            Ok(user)
        }
    }
}

pub async fn insert_user(
    pool: &DbPool,
    email: &str,
    password_hash: &str,
) -> Result<User, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let user = sqlx::query_as::<_, User>(
                r#"
                INSERT INTO users (id, email, password_hash, is_admin)
                VALUES ($1, $2, $3, $4)
                RETURNING id, email, password_hash, is_admin, bypass_alias_limit, disable_autoclean, can_send_firsthand, created_at, updated_at, last_login_at, last_login_ip
                "#,
            )
            .bind(uuid::Uuid::new_v4())
            .bind(email)
            .bind(password_hash)
            .bind(false)
            .fetch_one(pool)
            .await?;
            Ok(user)
        }
        DbPool::Sqlite(pool) => {
            let id = uuid::Uuid::new_v4();
            let user = sqlx::query_as::<sqlx::Sqlite, User>(
                r#"
                INSERT INTO users (id, email, password_hash, is_admin)
                VALUES (?, ?, ?, ?)
                RETURNING id, email, password_hash, is_admin, bypass_alias_limit, disable_autoclean, can_send_firsthand, created_at, updated_at, last_login_at, last_login_ip
                "#,
            )
            .bind(id)
            .bind(email)
            .bind(password_hash)
            .bind(false)
            .fetch_one(pool)
            .await?;
            Ok(user)
        }
    }
}

pub async fn toggle_bypass_alias_limit(
    pool: &DbPool,
    user_id: uuid::Uuid,
) -> Result<bool, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let new_val: bool = sqlx::query_scalar::<_, bool>(
                "UPDATE users SET bypass_alias_limit = NOT bypass_alias_limit WHERE id = $1 RETURNING bypass_alias_limit",
            )
            .bind(user_id)
            .fetch_one(pool)
            .await?;
            Ok(new_val)
        }
        DbPool::Sqlite(pool) => {
            let new_val: bool = sqlx::query_scalar::<sqlx::Sqlite, bool>(
                "UPDATE users SET bypass_alias_limit = NOT bypass_alias_limit WHERE id = ? RETURNING bypass_alias_limit",
            )
            .bind(user_id)
            .fetch_one(pool)
            .await?;
            Ok(new_val)
        }
    }
}

pub async fn toggle_disable_autoclean(
    pool: &DbPool,
    user_id: uuid::Uuid,
) -> Result<bool, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let new_val: bool = sqlx::query_scalar::<_, bool>(
                "UPDATE users SET disable_autoclean = NOT disable_autoclean WHERE id = $1 RETURNING disable_autoclean",
            )
            .bind(user_id)
            .fetch_one(pool)
            .await?;
            Ok(new_val)
        }
        DbPool::Sqlite(pool) => {
            let new_val: bool = sqlx::query_scalar::<sqlx::Sqlite, bool>(
                "UPDATE users SET disable_autoclean = NOT disable_autoclean WHERE id = ? RETURNING disable_autoclean",
            )
            .bind(user_id)
            .fetch_one(pool)
            .await?;
            Ok(new_val)
        }
    }
}

pub async fn toggle_can_send_firsthand(
    pool: &DbPool,
    user_id: uuid::Uuid,
) -> Result<bool, sqlx::Error> {
    match pool {
        DbPool::Postgres(pool) => {
            let new_val: bool = sqlx::query_scalar::<_, bool>(
                "UPDATE users SET can_send_firsthand = NOT can_send_firsthand WHERE id = $1 RETURNING can_send_firsthand",
            )
            .bind(user_id)
            .fetch_one(pool)
            .await?;
            Ok(new_val)
        }
        DbPool::Sqlite(pool) => {
            let new_val: bool = sqlx::query_scalar::<sqlx::Sqlite, bool>(
                "UPDATE users SET can_send_firsthand = NOT can_send_firsthand WHERE id = ? RETURNING can_send_firsthand",
            )
            .bind(user_id)
            .fetch_one(pool)
            .await?;
            Ok(new_val)
        }
    }
}

pub async fn seed_admin_on_startup(
    pool: &DbPool,
    config: &crate::config::AdminConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let admin_email = config.email.trim().to_string();
    let admin_password = config.password.clone();

    let mut errors = Vec::new();
    if admin_email.is_empty() || !admin_email.contains('@') {
        errors.push("ADMIN_EMAIL must be a non-empty, valid email address.");
    }
    if admin_password.len() < 8 {
        errors.push("ADMIN_PASSWORD must be at least 8 characters long for adequate security.");
    }

    if !errors.is_empty() {
        eprintln!("\n❌ Security Hardening Error: Admin credentials validation failed!");
        for err in &errors {
            eprintln!("   - {}", err);
        }
        eprintln!("\nPlease define ADMIN_EMAIL and ADMIN_PASSWORD in your environment or .env file before running Maileroo.");
        std::process::exit(1);
    }

    // Hash password with Argon2id
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(admin_password.as_bytes(), &salt)
        .map_err(|e| format!("Password hashing failed: {}", e))?
        .to_string();

    match pool {
        DbPool::Postgres(p) => {
            sqlx::query(
                r#"
                INSERT INTO users (id, email, password_hash, is_admin, bypass_alias_limit, disable_autoclean, can_send_firsthand)
                VALUES ($1, $2, $3, true, true, true, true)
                ON CONFLICT (email)
                DO UPDATE SET password_hash = $3,
                              is_admin = true,
                              bypass_alias_limit = true,
                              disable_autoclean = true,
                              can_send_firsthand = true,
                              updated_at = NOW()
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(&admin_email)
            .bind(&password_hash)
            .execute(p)
            .await?;
        }
        DbPool::Sqlite(p) => {
            sqlx::query(
                r#"
                INSERT INTO users (id, email, password_hash, is_admin, bypass_alias_limit, disable_autoclean, can_send_firsthand)
                VALUES (?, ?, ?, 1, 1, 1, 1)
                ON CONFLICT (email)
                DO UPDATE SET password_hash = ?,
                              is_admin = 1,
                              bypass_alias_limit = 1,
                              disable_autoclean = 1,
                              can_send_firsthand = 1,
                              updated_at = CURRENT_TIMESTAMP
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(&admin_email)
            .bind(&password_hash)
            .bind(&password_hash)
            .execute(p)
            .await?;
        }
    }

    println!("👤 Seeded / verified secure administrator: {}", admin_email);
    Ok(())
}
