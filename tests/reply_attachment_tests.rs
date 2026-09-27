mod common;

use common::run_on_all_dbs;

/// Pure DB-layer regression coverage for the reply attachment feature:
/// reply rows persist a body_key, and attachment metadata rows keyed on the
/// reply id are retrievable in part order on both SQLite and PostgreSQL.
#[tokio::test]
async fn test_reply_with_attachments_persistence() {
    run_on_all_dbs(|db| async move {
        // 1. Setup user, alias and a received email with a stored .eml
        let user = common::create_test_user(&db, "reply_att_user@example.com", "password").await;
        let alias =
            common::create_test_alias(&db, user.id, "example.com", "replyatt", "dest@example.com")
                .await;

        let body_key = uuid::Uuid::new_v4();
        let mock_eml = b"From: sender@example.com\r\nSubject: Parent\r\n\r\nHello";
        let (metadata, _attachments) =
            maileroo::inbound::parser::extract_full_metadata(mock_eml, "sender@example.com");

        let email = maileroo::db::attachments::insert_email_with_attachments(
            &db,
            alias.id,
            &metadata.sender,
            &metadata.subject,
            body_key,
            Some(time::OffsetDateTime::now_utc()),
            metadata.message_id,
            None,
            &[],
        )
        .await
        .unwrap();
        let email_id = email.id;

        // 2. Insert a reply carrying a body_key (the stored reply .eml)
        let reply = maileroo::db::replies::insert_reply(
            &db,
            email_id,
            "Here are the files you asked for",
            Some("<reply-1@example.com>".to_string()),
            Some(uuid::Uuid::new_v4()),
        )
        .await
        .unwrap();

        assert_eq!(reply.email_id, email_id);
        assert!(reply.body_key.is_some(), "body_key must be persisted");

        // 3. Insert two reply attachment rows (out of order on purpose to
        //    verify part_index ordering on retrieval)
        let att_a = uuid::Uuid::new_v4();
        let att_b = uuid::Uuid::new_v4();
        maileroo::db::attachments::insert_reply_attachment(
            &db,
            att_a,
            email_id,
            reply.id,
            Some("report.pdf"),
            Some("application/pdf"),
            2048,
            0,
        )
        .await
        .unwrap();
        maileroo::db::attachments::insert_reply_attachment(
            &db,
            att_b,
            email_id,
            reply.id,
            Some("data.csv"),
            Some("text/csv"),
            512,
            1,
        )
        .await
        .unwrap();

        // 4. Retrieve and verify
        let rows = maileroo::db::attachments::get_attachments_for_reply(&db, reply.id)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, att_a);
        assert_eq!(rows[0].part_index, 0);
        assert_eq!(rows[0].filename.as_deref(), Some("report.pdf"));
        assert_eq!(rows[0].content_type.as_deref(), Some("application/pdf"));
        assert_eq!(rows[0].size_bytes, 2048);
        assert_eq!(rows[1].id, att_b);
        assert_eq!(rows[1].part_index, 1);
        assert_eq!(rows[1].reply_id, reply.id);

        // 5. Reply listing includes the body_key for the download handler
        let replies = maileroo::db::replies::get_replies_for_email(&db, email_id)
            .await
            .unwrap();
        assert_eq!(replies.len(), 1);
        assert!(replies[0].body_key.is_some());

        let fetched = maileroo::db::replies::get_reply_by_id(&db, reply.id)
            .await
            .unwrap()
            .expect("reply must exist");
        assert_eq!(fetched.id, reply.id);
        assert_eq!(fetched.body_key, replies[0].body_key);
    })
    .await;
}

/// A reply without attachments must yield an empty attachment list and a
/// NULL body_key, leaving the previous single-part behavior untouched.
#[tokio::test]
async fn test_reply_without_attachments_is_unchanged() {
    run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "reply_plain@example.com", "password").await;
        let alias = common::create_test_alias(
            &db,
            user.id,
            "example.com",
            "replyplain",
            "dest@example.com",
        )
        .await;

        let body_key = uuid::Uuid::new_v4();
        let mock_eml = b"From: sender@example.com\r\nSubject: Plain\r\n\r\nBody";
        let (metadata, _) =
            maileroo::inbound::parser::extract_full_metadata(mock_eml, "sender@example.com");

        let email = maileroo::db::attachments::insert_email_with_attachments(
            &db,
            alias.id,
            &metadata.sender,
            &metadata.subject,
            body_key,
            None,
            metadata.message_id,
            None,
            &[],
        )
        .await
        .unwrap();

        let reply = maileroo::db::replies::insert_reply(
            &db,
            email.id,
            "text only",
            Some("<reply-2@example.com>".to_string()),
            None,
        )
        .await
        .unwrap();

        let rows = maileroo::db::attachments::get_attachments_for_reply(&db, reply.id)
            .await
            .unwrap();
        assert!(rows.is_empty());

        let fetched = maileroo::db::replies::get_reply_by_id(&db, reply.id)
            .await
            .unwrap()
            .unwrap();
        assert!(fetched.body_key.is_none());

        // Email-level attachments remain untouched by reply inserts.
        let email_atts = maileroo::db::attachments::get_attachments_for_email(&db, email.id)
            .await
            .unwrap();
        assert!(email_atts.is_empty());
    })
    .await;
}

/// Attachment rows must never leak across replies: two replies on the same
/// parent email see strictly their own rows.
#[tokio::test]
async fn test_reply_attachments_are_scoped_per_reply() {
    run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "reply_scope@example.com", "password").await;
        let alias = common::create_test_alias(
            &db,
            user.id,
            "example.com",
            "replyscope",
            "dest@example.com",
        )
        .await;

        let body_key = uuid::Uuid::new_v4();
        let mock_eml = b"From: sender@example.com\r\nSubject: Scoped\r\n\r\nBody";
        let (metadata, _) =
            maileroo::inbound::parser::extract_full_metadata(mock_eml, "sender@example.com");

        let email = maileroo::db::attachments::insert_email_with_attachments(
            &db,
            alias.id,
            &metadata.sender,
            &metadata.subject,
            body_key,
            None,
            metadata.message_id,
            None,
            &[],
        )
        .await
        .unwrap();

        let r1 = maileroo::db::replies::insert_reply(&db, email.id, "one", None, None)
            .await
            .unwrap();
        let r2 = maileroo::db::replies::insert_reply(&db, email.id, "two", None, None)
            .await
            .unwrap();

        let only_r1 = uuid::Uuid::new_v4();
        maileroo::db::attachments::insert_reply_attachment(
            &db,
            only_r1,
            email.id,
            r1.id,
            Some("only-first.txt"),
            Some("text/plain"),
            42,
            0,
        )
        .await
        .unwrap();

        let r1_atts = maileroo::db::attachments::get_attachments_for_reply(&db, r1.id)
            .await
            .unwrap();
        let r2_atts = maileroo::db::attachments::get_attachments_for_reply(&db, r2.id)
            .await
            .unwrap();

        assert_eq!(r1_atts.len(), 1);
        assert_eq!(r1_atts[0].filename.as_deref(), Some("only-first.txt"));
        assert!(r2_atts.is_empty());
    })
    .await;
}
