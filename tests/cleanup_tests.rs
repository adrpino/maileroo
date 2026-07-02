mod common;

use maileroo::db::{DbPool, delete_old_emails, insert_email};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Returns true if a received_email row with the given id still exists.
async fn received_email_exists(db: &DbPool, id: Uuid) -> bool {
    let q = "SELECT 1 FROM received_emails WHERE id = $1";
    match db {
        DbPool::Postgres(p) => sqlx::query(q)
            .bind(id)
            .fetch_optional(p)
            .await
            .unwrap()
            .is_some(),
        DbPool::Sqlite(p) => sqlx::query(q)
            .bind(id)
            .fetch_optional(p)
            .await
            .unwrap()
            .is_some(),
    }
}

async fn set_disable_autoclean(db: &DbPool, user_id: Uuid, value: bool) {
    let q = "UPDATE users SET disable_autoclean = $1 WHERE id = $2";
    match db {
        DbPool::Postgres(p) => {
            sqlx::query(q)
                .bind(value)
                .bind(user_id)
                .execute(p)
                .await
                .unwrap();
        }
        DbPool::Sqlite(p) => {
            sqlx::query(q)
                .bind(value)
                .bind(user_id)
                .execute(p)
                .await
                .unwrap();
        }
    }
}

/// The core invariant of autoclean: with a 30-day retention, an email received
/// 40 days ago MUST be deleted, and an email received today (and one received
/// 5 days ago) MUST survive.
///
/// If the retention comparison is inverted (deleting recent mail instead of old
/// mail), this test fails on the "new email survives" assertion.
#[tokio::test]
async fn test_autoclean_deletes_old_keeps_new() {
    common::run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "cleanup@example.com", "password123").await;
        // autoclean enabled (disable_autoclean = false) -> eligible for cleanup
        set_disable_autoclean(&db, user.id, false).await;
        let alias =
            common::create_test_alias(&db, user.id, "example.com", "inbox", "dest@example.com")
                .await;

        let now = OffsetDateTime::now_utc();

        let old_id = insert_email(
            &db,
            alias.id,
            "old-sender@example.com",
            "40 days old",
            Uuid::new_v4(),
            Some(now - Duration::days(40)),
            None,
            None,
        )
        .await
        .unwrap()
        .id;

        let recent_id = insert_email(
            &db,
            alias.id,
            "recent-sender@example.com",
            "5 days old",
            Uuid::new_v4(),
            Some(now - Duration::days(5)),
            None,
            None,
        )
        .await
        .unwrap()
        .id;

        let today_id = insert_email(
            &db,
            alias.id,
            "today-sender@example.com",
            "received today",
            Uuid::new_v4(),
            Some(now),
            None,
            None,
        )
        .await
        .unwrap()
        .id;

        let deleted_keys = delete_old_emails(&db, 30).await.unwrap();

        assert!(
            !received_email_exists(&db, old_id).await,
            "BUG: the 40-day-old email should have been deleted by 30-day retention, but it survived"
        );
        assert!(
            received_email_exists(&db, recent_id).await,
            "BUG: the 5-day-old email was deleted but is well within the 30-day retention window"
        );
        assert!(
            received_email_exists(&db, today_id).await,
            "BUG: an email received TODAY was deleted by the autoclean — retention comparison is inverted"
        );
        assert_eq!(
            deleted_keys.len(),
            1,
            "autoclean should have deleted exactly the one old email, got {} deletions",
            deleted_keys.len()
        );
    })
    .await;
}

/// When a user has disable_autoclean = true, even old mail must be preserved.
#[tokio::test]
async fn test_autoclean_respects_disable_flag() {
    common::run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "protected@example.com", "password123").await;
        set_disable_autoclean(&db, user.id, true).await;
        let alias =
            common::create_test_alias(&db, user.id, "example.com", "inbox", "dest@example.com")
                .await;

        let now = OffsetDateTime::now_utc();

        let old_id = insert_email(
            &db,
            alias.id,
            "old-sender@example.com",
            "100 days old but protected",
            Uuid::new_v4(),
            Some(now - Duration::days(100)),
            None,
            None,
        )
        .await
        .unwrap()
        .id;

        let deleted_keys = delete_old_emails(&db, 30).await.unwrap();

        assert!(
            received_email_exists(&db, old_id).await,
            "BUG: email belongs to a user with disable_autoclean=true and must never be deleted"
        );
        assert_eq!(
            deleted_keys.len(),
            0,
            "no emails should be deleted when the only user has autoclean disabled"
        );
    })
    .await;
}
