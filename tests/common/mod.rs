#![allow(dead_code)]

use maileroo::db::{
    DbPool,
    aliases::insert_alias,
    sent_emails::{EmailStatus, SentEmail, insert_sent_email},
    users::insert_user,
};
use sqlx::{PgPool, SqlitePool};
use std::future::Future;
use uuid::Uuid;

static CRYPTO_INIT: std::sync::Once = std::sync::Once::new();

/// Automatically installs the Rustls process-level CryptoProvider exactly once.
pub fn init_crypto_provider() {
    CRYPTO_INIT.call_once(|| {
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Helper to generate dummy throwaway TLS certificates in memory using rcgen and write them to disk.
pub fn generate_dummy_certs(cert_path: &std::path::Path, key_path: &std::path::Path) {
    let subject_alt_names = vec!["localhost".to_string(), "example.com".to_string()];
    let cert = rcgen::generate_simple_self_signed(subject_alt_names).unwrap();
    std::fs::write(cert_path, cert.cert.pem()).unwrap();
    std::fs::write(key_path, cert.signing_key.serialize_pem()).unwrap();
}

/// Helper to boot up a pristine in-memory SQLite database and run all migrations.
pub async fn setup_sqlite_in_memory_db() -> DbPool {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let db_pool = DbPool::Sqlite(pool);
    maileroo::db::run_migrations(&db_pool).await.unwrap();
    db_pool
}

fn get_admin_and_unique_db_urls(base_test_url: &str) -> (String, String, String) {
    let (schema, rest) = base_test_url
        .split_once("://")
        .expect("Invalid TEST_DATABASE_URL format");
    let (host_part, path_and_query) = rest
        .split_once('/')
        .expect("Invalid TEST_DATABASE_URL format");
    let (db_name, query_params) = match path_and_query.split_once('?') {
        Some((db, query)) => (db, format!("?{}", query)),
        None => (path_and_query, String::new()),
    };

    let unique_id = Uuid::new_v4().simple().to_string();
    let unique_db_name = format!("{}_{}", db_name, unique_id);
    let admin_url = format!("{}://{}/postgres{}", schema, host_part, query_params);
    let unique_db_url = format!(
        "{}://{}/{}{}",
        schema, host_part, unique_db_name, query_params
    );

    (admin_url, unique_db_name, unique_db_url)
}

/// Runs the provided async test logic on both SQLite and PostgreSQL (if TEST_DATABASE_URL is set).
pub async fn run_on_all_dbs<F, Fut>(test_fn: F)
where
    F: Fn(DbPool) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    // Ensure the process-level CryptoProvider is initialized before any connection builders start!
    init_crypto_provider();

    // --- 1. RUN ON SQLITE (IN-MEMORY) ---
    {
        let db = setup_sqlite_in_memory_db().await;
        test_fn(db).await;
    }

    // --- 2. RUN ON POSTGRES (IF AVAILABLE) ---
    if let Ok(pg_url) = std::env::var("TEST_DATABASE_URL") {
        if !pg_url.trim().is_empty() {
            let (admin_url, db_name, unique_db_url) = get_admin_and_unique_db_urls(&pg_url);

            // Connect to default administrative database to create our unique isolated test database
            let admin_pool = PgPool::connect(&admin_url).await.unwrap();
            let create_query = format!("CREATE DATABASE {};", db_name);
            sqlx::query(&create_query)
                .execute(&admin_pool)
                .await
                .unwrap();
            admin_pool.close().await;

            // Connect to the unique test database and run migrations
            let unique_pool = PgPool::connect(&unique_db_url).await.unwrap();
            let db = DbPool::Postgres(unique_pool.clone());
            maileroo::db::run_migrations(&db).await.unwrap();

            // Run the test logic catching any panics to guarantee cleanup
            use futures_util::FutureExt;
            let test_result = std::panic::AssertUnwindSafe(test_fn(db))
                .catch_unwind()
                .await;

            // Explicitly close the connection pool so that Postgres releases any locks before dropping
            unique_pool.close().await;

            // Connect back to default administrative database to drop our unique test database
            let cleanup_pool = PgPool::connect(&admin_url).await.unwrap();
            let drop_query = format!("DROP DATABASE IF EXISTS {} WITH (FORCE);", db_name);
            let _ = sqlx::query(&drop_query).execute(&cleanup_pool).await;
            cleanup_pool.close().await;

            // If the test panics, resume the unwind to mark the test as failed
            if let Err(payload) = test_result {
                std::panic::resume_unwind(payload);
            }
        }
    }
}

/// Helper to create a test user with a password hashed using Argon2.
pub async fn create_test_user(
    db: &DbPool,
    email: &str,
    cleartext_password: &str,
) -> maileroo::db::users::User {
    let hash = maileroo::web::auth::hash_password(cleartext_password).unwrap();
    insert_user(db, email, &hash).await.unwrap()
}

/// Helper to create a verified domain and alias for a user.
pub async fn create_test_alias(
    db: &DbPool,
    user_id: Uuid,
    domain_name: &str,
    subdomain: &str,
    destination_email: &str,
) -> maileroo::db::Alias {
    let domain_id = Uuid::new_v4();
    let query_str = "INSERT INTO domains (id, name, active) VALUES ($1, $2, true)";
    match db {
        DbPool::Sqlite(p) => {
            sqlx::query(query_str)
                .bind(domain_id)
                .bind(domain_name)
                .execute(p)
                .await
                .unwrap();
        }
        DbPool::Postgres(p) => {
            sqlx::query(query_str)
                .bind(domain_id)
                .bind(domain_name)
                .execute(p)
                .await
                .unwrap();
        }
    }
    insert_alias(db, user_id, domain_id, subdomain, destination_email, true)
        .await
        .unwrap()
}

/// Helper to insert a draft/sent outbound email into the database.
pub async fn create_test_draft(
    db: &DbPool,
    user_id: Uuid,
    alias_id: Uuid,
    to_address: &str,
    subject: &str,
    status: EmailStatus,
) -> SentEmail {
    let body_key = Uuid::new_v4();
    insert_sent_email(
        db, user_id, alias_id, to_address, subject, body_key, status, None,
    )
    .await
    .unwrap()
}

/// Helper to authenticate a user via HTTP and retrieve the set-cookie header session string.
pub async fn get_auth_cookie(app: axum::Router, email: &str, password_cleartext: &str) -> String {
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, header};
    use std::net::SocketAddr;
    use tower::ServiceExt;

    // Attach ConnectInfo extension to satisfy the Governor rate-limiting IP extractor
    let req = Request::builder()
        .method("POST")
        .uri("/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))))
        .body(Body::from(format!(
            "email={}&password={}",
            email, password_cleartext
        )))
        .unwrap();

    let res = app.oneshot(req).await.unwrap();

    // Extract all set-cookie header values (e.g. csrf_token and session id) and merge them
    let cookies: Vec<String> = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string())
        .collect();

    let cookie_header = cookies.join("; ");

    println!("DEBUG LOGIN COOKIE HEADER FOR CLIENT: {:?}", cookie_header);

    assert!(
        !cookie_header.is_empty(),
        "Login did not return any Set-Cookie headers"
    );
    cookie_header
}

/// Helper to extract the raw CSRF token string from our combined Cookie header.
pub fn extract_csrf_token(cookie_header: &str) -> String {
    cookie_header
        .split(';')
        .find(|s| s.trim().starts_with("csrf_token="))
        .expect("CSRF token not found in cookie header")
        .split('=')
        .nth(1)
        .expect("CSRF token key has no value")
        .to_string()
}

/// Returns true if an outbound email ID exists in either database.
pub async fn email_exists_in_db(db: &DbPool, id: Uuid) -> bool {
    let query_str = "SELECT 1 FROM sent_emails WHERE id = $1";
    match db {
        DbPool::Postgres(p) => sqlx::query(query_str)
            .bind(id)
            .fetch_optional(p)
            .await
            .unwrap()
            .is_some(),
        DbPool::Sqlite(p) => sqlx::query(query_str)
            .bind(id)
            .fetch_optional(p)
            .await
            .unwrap()
            .is_some(),
    }
}

/// Helper to grant full admin and firsthand sender permissions to a test user.
pub async fn grant_user_sender_permissions(db: &DbPool, user_id: Uuid) {
    let query_str = "UPDATE users SET can_send_firsthand = true, is_admin = true WHERE id = $1";
    match db {
        DbPool::Postgres(p) => {
            sqlx::query(query_str)
                .bind(user_id)
                .execute(p)
                .await
                .unwrap();
        }
        DbPool::Sqlite(p) => {
            sqlx::query(query_str)
                .bind(user_id)
                .execute(p)
                .await
                .unwrap();
        }
    }
}
