mod common;

use maileroo::db::{
    DbPool, delete_old_emails, get_email_by_id, insert_email, toggle_email_favorite,
};
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

async fn set_user_is_admin(db: &DbPool, user_id: Uuid, is_admin: bool) {
    let q = "UPDATE users SET is_admin = $1 WHERE id = $2";
    match db {
        DbPool::Postgres(p) => {
            sqlx::query(q)
                .bind(is_admin)
                .bind(user_id)
                .execute(p)
                .await
                .unwrap();
        }
        DbPool::Sqlite(p) => {
            sqlx::query(q)
                .bind(is_admin)
                .bind(user_id)
                .execute(p)
                .await
                .unwrap();
        }
    }
}

/// Admin favorited emails survive autoclean retention.
#[tokio::test]
async fn test_autoclean_admin_favorites_protected() {
    common::run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "admin_fav@example.com", "password123").await;
        set_user_is_admin(&db, user.id, true).await;
        set_disable_autoclean(&db, user.id, false).await;
        let alias =
            common::create_test_alias(&db, user.id, "example.com", "fav", "dest@example.com").await;

        let now = OffsetDateTime::now_utc();
        let old_id = insert_email(
            &db,
            alias.id,
            "sender@example.com",
            "40 days old admin favorite",
            Uuid::new_v4(),
            Some(now - Duration::days(40)),
            None,
            None,
        )
        .await
        .unwrap()
        .id;

        // Toggle favorite to true
        let fav_res = toggle_email_favorite(&db, old_id, user.id).await.unwrap();
        assert_eq!(fav_res, Some(true));

        let deleted_keys = delete_old_emails(&db, 30).await.unwrap();

        assert!(
            received_email_exists(&db, old_id).await,
            "Admin favorited email should survive autoclean"
        );
        assert_eq!(deleted_keys.len(), 0);
    })
    .await;
}

/// Non-admin favorited emails MUST be deleted by autoclean as a safeguard.
#[tokio::test]
async fn test_autoclean_non_admin_favorites_deleted() {
    common::run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "user_fav@example.com", "password123").await;
        set_user_is_admin(&db, user.id, false).await;
        set_disable_autoclean(&db, user.id, false).await;
        let alias =
            common::create_test_alias(&db, user.id, "example.com", "fav2", "dest@example.com")
                .await;

        let now = OffsetDateTime::now_utc();
        let old_id = insert_email(
            &db,
            alias.id,
            "sender@example.com",
            "40 days old non-admin favorite",
            Uuid::new_v4(),
            Some(now - Duration::days(40)),
            None,
            None,
        )
        .await
        .unwrap()
        .id;

        // Toggle favorite to true
        let fav_res = toggle_email_favorite(&db, old_id, user.id).await.unwrap();
        assert_eq!(fav_res, Some(true));

        let deleted_keys = delete_old_emails(&db, 30).await.unwrap();

        assert!(
            !received_email_exists(&db, old_id).await,
            "Non-admin favorited email must NOT survive autoclean"
        );
        assert_eq!(deleted_keys.len(), 1);
    })
    .await;
}

/// Favoriting a child reply in an admin thread protects the entire thread (including root).
#[tokio::test]
async fn test_autoclean_admin_thread_child_favorite_protects_root() {
    common::run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "admin_thread@example.com", "password123").await;
        set_user_is_admin(&db, user.id, true).await;
        set_disable_autoclean(&db, user.id, false).await;
        let alias =
            common::create_test_alias(&db, user.id, "example.com", "favthread", "dest@example.com")
                .await;

        let now = OffsetDateTime::now_utc();
        let root_id = insert_email(
            &db,
            alias.id,
            "sender@example.com",
            "40 days old root",
            Uuid::new_v4(),
            Some(now - Duration::days(40)),
            None,
            None,
        )
        .await
        .unwrap()
        .id;

        let child_id = insert_email(
            &db,
            alias.id,
            "sender@example.com",
            "35 days old reply",
            Uuid::new_v4(),
            Some(now - Duration::days(35)),
            None,
            Some(root_id),
        )
        .await
        .unwrap()
        .id;

        // Favorite ONLY the child email
        let fav_res = toggle_email_favorite(&db, child_id, user.id).await.unwrap();
        assert_eq!(fav_res, Some(true));

        let deleted_keys = delete_old_emails(&db, 30).await.unwrap();

        assert!(
            received_email_exists(&db, root_id).await,
            "Admin root email should be protected by favorited child in the thread"
        );
        assert!(
            received_email_exists(&db, child_id).await,
            "Admin child email should be protected"
        );
        assert_eq!(deleted_keys.len(), 0);
    })
    .await;
}

/// Inbound replies on a viewed thread must reset the root email's viewed status to false (unread).
#[tokio::test]
async fn test_thread_reply_marks_root_unread() {
    common::run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "unread_test@example.com", "password123").await;
        let alias = common::create_test_alias(
            &db,
            user.id,
            "example.com",
            "threadunread",
            "dest@example.com",
        )
        .await;

        // 1. Initial root email received
        let root = insert_email(
            &db,
            alias.id,
            "sender@example.com",
            "Discussion",
            Uuid::new_v4(),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(!root.viewed, "New root email should start unread");

        // 2. User views the root email
        maileroo::db::mark_email_as_viewed(&db, root.id, user.id)
            .await
            .unwrap();
        let viewed_root = get_email_by_id(&db, root.id, user.id)
            .await
            .unwrap()
            .unwrap();
        assert!(viewed_root.viewed, "Root email should be viewed");

        // 3. Inbound reply arrives in the thread
        let _child = insert_email(
            &db,
            alias.id,
            "sender@example.com",
            "Re: Discussion",
            Uuid::new_v4(),
            None,
            None,
            Some(root.id),
        )
        .await
        .unwrap();

        // 4. Root email must now be marked unread (viewed = false)
        let updated_root = get_email_by_id(&db, root.id, user.id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            !updated_root.viewed,
            "Root email must be reset to unread when an inbound reply arrives"
        );
    })
    .await;
}
