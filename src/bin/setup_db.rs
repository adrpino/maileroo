use argon2::{
    Argon2,
    password_hash::{PasswordHasher, SaltString, rand_core::OsRng},
};
use dotenvy::dotenv;
use maileroo::db::attachments::insert_email_with_attachments;
use maileroo::db::{DbPool, init_pool, run_migrations};
use maileroo::fs::{create_dir_all_async_with_permissions, write_file_async_with_permissions};
use maileroo::inbound::parser::extract_full_metadata;
use std::env;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    // Initialize the pool using standard production logic
    let pool = init_pool(&database_url).await?;

    println!("🚀 Checking and running database migrations...");
    run_migrations(&pool).await?;

    println!("🌱 Seeding generic development data...");
    seed_data(&pool).await?;

    println!("✅ Schema setup and seeding complete.");
    Ok(())
}

async fn seed_data(pool: &DbPool) -> Result<(), Box<dyn std::error::Error>> {
    let storage_dir = env::var("STORAGE_DIR").unwrap_or_else(|_| "storage/emails".to_string());
    let storage_path = std::path::Path::new(&storage_dir);
    create_dir_all_async_with_permissions(storage_path).await?;

    // 1. Create Test User (Admin) - dynamic or generic fallback
    let user_id = Uuid::new_v4();
    let email = env::var("ADMIN_EMAIL").unwrap_or_else(|_| "admin@maileroo.test".to_string());
    let password = env::var("ADMIN_PASSWORD").unwrap_or_else(|_| "admin123".to_string());

    // Hash password using same logic as production auth
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| e.to_string())?
        .to_string();

    let query_insert_user = "INSERT INTO users (id, email, password_hash, is_admin, bypass_alias_limit, disable_autoclean, can_send_firsthand) VALUES ($1, $2, $3, $4, TRUE, TRUE, TRUE) ON CONFLICT (email) DO UPDATE SET password_hash = $3, is_admin = $4";
    let query_insert_user_sqlite = "INSERT INTO users (id, email, password_hash, is_admin, bypass_alias_limit, disable_autoclean, can_send_firsthand) VALUES (?, ?, ?, ?, 1, 1, 1) ON CONFLICT (email) DO UPDATE SET password_hash = ?, is_admin = ?";

    match pool {
        DbPool::Postgres(p) => {
            sqlx::query(query_insert_user)
                .bind(user_id)
                .bind(&email)
                .bind(&password_hash)
                .bind(true)
                .execute(p)
                .await?;
        }
        DbPool::Sqlite(p) => {
            sqlx::query(query_insert_user_sqlite)
                .bind(user_id)
                .bind(&email)
                .bind(&password_hash)
                .bind(true)
                .bind(&password_hash)
                .bind(true)
                .execute(p)
                .await?;
        }
    }

    // Get the user id
    let user_id: Uuid = match pool {
        DbPool::Postgres(p) => {
            sqlx::query_scalar("SELECT id FROM users WHERE email = $1")
                .bind(&email)
                .fetch_one(p)
                .await?
        }
        DbPool::Sqlite(p) => {
            sqlx::query_scalar("SELECT id FROM users WHERE email = ?")
                .bind(&email)
                .fetch_one(p)
                .await?
        }
    };

    // 2. Create Test Domain
    let domain_id = Uuid::new_v4();
    let domain_name = "maileroo.test";

    let insert_domain = "INSERT INTO domains (id, name, active) VALUES ($1, $2, true) ON CONFLICT (name) DO NOTHING";
    let insert_domain_sqlite =
        "INSERT INTO domains (id, name, active) VALUES (?, ?, 1) ON CONFLICT (name) DO NOTHING";

    match pool {
        DbPool::Postgres(p) => {
            sqlx::query(insert_domain)
                .bind(domain_id)
                .bind(domain_name)
                .execute(p)
                .await?;
        }
        DbPool::Sqlite(p) => {
            sqlx::query(insert_domain_sqlite)
                .bind(domain_id)
                .bind(domain_name)
                .execute(p)
                .await?;
        }
    }

    let domain_id: Uuid = match pool {
        DbPool::Postgres(p) => {
            sqlx::query_scalar("SELECT id FROM domains WHERE name = $1")
                .bind(domain_name)
                .fetch_one(p)
                .await?
        }
        DbPool::Sqlite(p) => {
            sqlx::query_scalar("SELECT id FROM domains WHERE name = ?")
                .bind(domain_name)
                .fetch_one(p)
                .await?
        }
    };

    // 3. Create Test Alias
    let alias_id = Uuid::new_v4();
    let subdomain = "hello";

    let insert_alias = "INSERT INTO aliases (id, user_id, domain_id, subdomain, destination_email, auto_forward, active) VALUES ($1, $2, $3, $4, $5, $6, true) ON CONFLICT (subdomain, domain_id) DO NOTHING";
    let insert_alias_sqlite = "INSERT INTO aliases (id, user_id, domain_id, subdomain, destination_email, auto_forward, active) VALUES (?, ?, ?, ?, ?, ?, 1) ON CONFLICT (subdomain, domain_id) DO NOTHING";

    match pool {
        DbPool::Postgres(p) => {
            sqlx::query(insert_alias)
                .bind(alias_id)
                .bind(user_id)
                .bind(domain_id)
                .bind(subdomain)
                .bind(&email)
                .bind(true)
                .execute(p)
                .await?;
        }
        DbPool::Sqlite(p) => {
            sqlx::query(insert_alias_sqlite)
                .bind(alias_id)
                .bind(user_id)
                .bind(domain_id)
                .bind(subdomain)
                .bind(&email)
                .bind(true)
                .execute(p)
                .await?;
        }
    }

    let alias_id: Uuid = match pool {
        DbPool::Postgres(p) => {
            sqlx::query_scalar("SELECT id FROM aliases WHERE subdomain = $1 AND domain_id = $2")
                .bind(subdomain)
                .bind(domain_id)
                .fetch_one(p)
                .await?
        }
        DbPool::Sqlite(p) => {
            sqlx::query_scalar("SELECT id FROM aliases WHERE subdomain = ? AND domain_id = ?")
                .bind(subdomain)
                .bind(domain_id)
                .fetch_one(p)
                .await?
        }
    };

    // 4. Create Test Emails
    let emails = vec![
        ("alice@example.net", "Welcome to Maileroo!"),
        ("bob@work.com", "Project Update"),
        ("newsletter@tech.io", "Weekly Tech News"),
        ("support@service.com", "Your ticket has been resolved"),
    ];

    for (sender, subject) in emails {
        let email_id = Uuid::new_v4();
        let body_key = Uuid::new_v4();

        let insert_email = "INSERT INTO received_emails (id, alias_id, sender_email, subject, body_key, forwarded) VALUES ($1, $2, $3, $4, $5, $6)";
        let insert_email_sqlite = "INSERT INTO received_emails (id, alias_id, sender_email, subject, body_key, forwarded) VALUES (?, ?, ?, ?, ?, ?)";

        match pool {
            DbPool::Postgres(p) => {
                sqlx::query(insert_email)
                    .bind(email_id)
                    .bind(alias_id)
                    .bind(sender)
                    .bind(subject)
                    .bind(body_key)
                    .bind(false)
                    .execute(p)
                    .await?;
            }
            DbPool::Sqlite(p) => {
                sqlx::query(insert_email_sqlite)
                    .bind(email_id)
                    .bind(alias_id)
                    .bind(sender)
                    .bind(subject)
                    .bind(body_key)
                    .bind(false)
                    .execute(p)
                    .await?;
            }
        }

        // Create the .eml file
        let file_path = storage_path.join(format!("{}.eml", body_key));
        let content = format!(
            "From: {}\r\nTo: hello@maileroo.test\r\nSubject: {}\r\nDate: Mon, 9 Feb 2026 12:00:00 +0000\r\n\r\nThis is a seeded test email for the subject: {}\r\n",
            sender, subject, subject
        );
        write_file_async_with_permissions(&file_path, content).await?;
    }

    let email_id = Uuid::new_v4();
    let body_key = Uuid::new_v4();

    let insert_messy = "INSERT INTO received_emails (id, alias_id, sender_email, subject, body_key, forwarded) VALUES ($1, $2, $3, $4, $5, $6)";
    let insert_messy_sqlite = "INSERT INTO received_emails (id, alias_id, sender_email, subject, body_key, forwarded) VALUES (?, ?, ?, ?, ?, ?)";

    match pool {
        DbPool::Postgres(p) => {
            sqlx::query(insert_messy)
                .bind(email_id)
                .bind(alias_id)
                .bind("messy@stylebreaker.com")
                .bind("TEST: Messy global CSS!")
                .bind(body_key)
                .bind(false)
                .execute(p)
                .await?;
        }
        DbPool::Sqlite(p) => {
            sqlx::query(insert_messy_sqlite)
                .bind(email_id)
                .bind(alias_id)
                .bind("messy@stylebreaker.com")
                .bind("TEST: Messy global CSS!")
                .bind(body_key)
                .bind(false)
                .execute(p)
                .await?;
        }
    }

    let file_path = storage_path.join(format!("{}.eml", body_key));
    let messy_content = "From: messy@stylebreaker.com\r\nTo: hello@maileroo.test\r\nSubject: TEST: Messy global CSS!\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html>\n<head>\n<style>\nbody { margin: 0 !important; padding: 0 !important; background-color: purple !important; }\nh1, h2, h3 { color: red !important; font-size: 100px !important; }\nnav, header, .header { display: none !important; }\n* { border: 5px solid lime !important; }\n</style>\n</head>\n<body>\n<h1>MESSY EMAIL</h1>\n<p>If Shadow DOM is working, this won't break your app's main layout.</p>\n</body>\n</html>\n";
    write_file_async_with_permissions(&file_path, messy_content.to_string()).await?;

    // --- Seed Attachment Test Email ---
    let body_key = Uuid::new_v4();
    let email_file_path = storage_path.join(format!("{}.eml", body_key));
    let mock_eml_content = b"From: files@example.com\r\nTo: hello@maileroo.test\r\nSubject: UI Attachment Test\r\nContent-Type: multipart/mixed; boundary=mixed123\r\n\r\n--mixed123\r\nContent-Type: multipart/related; boundary=related123\r\n\r\n--related123\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body><p>Here is an inline company logo!</p><img src=\"cid:company-logo\" style=\"max-width: 200px; border: 1px solid #ccc;\"/><p>Also see the attached invoice.</p></body></html>\r\n--related123\r\nContent-Type: image/png\r\nContent-ID: <company-logo>\r\nContent-Disposition: inline\r\n\r\n\x89\x50\x4e\x47\x0d\x0a\x1a\x0a\x00\x00\x00\x0d\x49\x48\x44\x52\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\x0d\x49\x44\x41\x54\x78\xda\x63\x64\x64\x64\x60\x00\x00\x00\x0e\x00\x02\x42\x33\xf8\x41\x00\x00\x00\x00\x49\x45\x4e\x44\xae\x42\x60\x82\r\n--related123--\r\n--mixed123\r\nContent-Type: application/pdf; name=invoice.pdf\r\nContent-Disposition: attachment; filename=invoice.pdf\r\n\r\n%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>\nendobj\n4 0 obj\n<< /Length 53 >>\nstream\nBT\n/F1 24 Tf\n100 700 Td\n(Fake Invoice PDF Content) Tj\nET\nendstream\nendobj\nxref\n0 5\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000115 00000 n \n0000000289 00000 n \ntrailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n391\n%%EOF\n\r\n--mixed123--\r\n";
    write_file_async_with_permissions(
        &email_file_path,
        String::from_utf8_lossy(mock_eml_content).to_string(),
    )
    .await?;

    let (metadata, attachments) = extract_full_metadata(mock_eml_content, "files@example.com");

    insert_email_with_attachments(
        pool,
        alias_id,
        &metadata.sender,
        &metadata.subject,
        body_key,
        Some(time::OffsetDateTime::now_utc()),
        metadata.message_id,
        None,
        &attachments,
    )
    .await?;

    println!(
        "✨ Seeded admin user: {} with password: {}",
        email, password
    );
    Ok(())
}
