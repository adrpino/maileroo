mod common;

use maileroo::db::{
    find_thread_id_by_references, get_email_by_id, insert_email, replies::insert_reply,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// An inbound email that only references an outbound reply sent from a child email
/// must be threaded under the root, otherwise it is hidden from both the inbox and the thread view.
#[tokio::test]
async fn test_thread_id_resolves_to_root_via_reply_on_child() {
    common::run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "threads@example.com", "password123").await;
        let alias =
            common::create_test_alias(&db, user.id, "example.com", "inbox", "dest@example.com")
                .await;

        let root = insert_email(
            &db,
            alias.id,
            "sender@external.com",
            "Thread subject",
            Uuid::new_v4(),
            None,
            Some("<root@external.com>".to_string()),
            None,
        )
        .await
        .unwrap();

        let child = insert_email(
            &db,
            alias.id,
            "sender@external.com",
            "Re: Thread subject",
            Uuid::new_v4(),
            None,
            Some("<child@external.com>".to_string()),
            Some(root.id),
        )
        .await
        .unwrap();

        // Outbound reply sent while viewing the child email
        let reply_msg_id = "<outbound-reply@example.com>";
        insert_reply(
            &db,
            child.id,
            "Reply body",
            Some(reply_msg_id.to_string()),
            None,
        )
        .await
        .unwrap();

        let found_id = find_thread_id_by_references(&db, &[reply_msg_id.to_string()])
            .await
            .unwrap();
        assert_eq!(found_id, Some(root.id));
    })
    .await;
}

/// Sending a reply from a child email must bump the thread root's activity,
/// since the inbox is ordered by the root's last_activity_at.
#[tokio::test]
async fn test_reply_on_child_bumps_root_last_activity() {
    common::run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "activity@example.com", "password123").await;
        let alias =
            common::create_test_alias(&db, user.id, "example.com", "inbox", "dest@example.com")
                .await;

        let past = OffsetDateTime::now_utc() - Duration::days(3);

        let root = insert_email(
            &db,
            alias.id,
            "sender@external.com",
            "Thread subject",
            Uuid::new_v4(),
            Some(past),
            Some("<root@external.com>".to_string()),
            None,
        )
        .await
        .unwrap();

        let child = insert_email(
            &db,
            alias.id,
            "sender@external.com",
            "Re: Thread subject",
            Uuid::new_v4(),
            Some(past),
            Some("<child@external.com>".to_string()),
            Some(root.id),
        )
        .await
        .unwrap();

        insert_reply(&db, child.id, "Reply body", None, None)
            .await
            .unwrap();

        let root_after = get_email_by_id(&db, root.id, user.id)
            .await
            .unwrap()
            .unwrap();
        assert!(root_after.last_activity_at > past + Duration::days(1));
    })
    .await;
}
