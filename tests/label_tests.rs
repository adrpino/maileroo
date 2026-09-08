mod common;

use maileroo::db::labels::{
    assign_labels_to_email, delete_filter, delete_label, get_filters_by_user, get_labels_by_user,
    get_labels_for_email, get_labels_for_emails, insert_filter, insert_label,
    remove_label_from_email, update_label,
};
use maileroo::db::{
    delete_email_by_id, delete_old_emails, get_email_by_user_id, get_email_count_by_user_id,
    insert_email,
};
use maileroo::filter::FilterEngine;
use maileroo::inbound::acceptor::HotReloadAcceptor;
use maileroo::inbound::blocklist::Blocklist;
use maileroo::inbound::protocol::SmtpSession;
use maileroo::inbound::rate_limit::{InboundLimits, RateLimiter};
use maileroo::outbound::OutboundService;
use maileroo::web::DashboardEvent;
use std::sync::Arc;
use time::{Duration, OffsetDateTime};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use uuid::Uuid;

#[tokio::test]
async fn test_label_and_filter_crud() {
    common::run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "label_user@example.com", "secretpass").await;

        // 1. Create labels
        let label1 = insert_label(&db, user.id, "Invoices", "#3182ce")
            .await
            .unwrap();
        let label2 = insert_label(&db, user.id, "Receipts", "#38a169")
            .await
            .unwrap();
        assert_eq!(label1.name, "Invoices");
        assert_eq!(label2.name, "Receipts");

        // 2. Fetch labels
        let labels = get_labels_by_user(&db, user.id).await.unwrap();
        assert_eq!(labels.len(), 2);

        // 3. Update label
        let updated = update_label(&db, label1.id, user.id, "Billing", "#e53e3e")
            .await
            .unwrap();
        assert!(updated);
        let labels_after_update = get_labels_by_user(&db, user.id).await.unwrap();
        let updated_label = labels_after_update
            .iter()
            .find(|l| l.id == label1.id)
            .unwrap();
        assert_eq!(updated_label.name, "Billing");
        assert_eq!(updated_label.color, "#e53e3e");

        // 4. Create filter rule
        let filter1 = insert_filter(&db, user.id, "invoice", label1.id)
            .await
            .unwrap();
        assert_eq!(filter1.keyword, "invoice");
        assert_eq!(filter1.label_id, label1.id);

        let filters = get_filters_by_user(&db, user.id).await.unwrap();
        assert_eq!(filters.len(), 1);
        assert_eq!(filters[0].label_name, "Billing");

        // 5. Delete filter
        let filter_deleted = delete_filter(&db, filter1.id, user.id).await.unwrap();
        assert!(filter_deleted);
        let filters_empty = get_filters_by_user(&db, user.id).await.unwrap();
        assert!(filters_empty.is_empty());

        // 6. Delete label
        let label_deleted = delete_label(&db, label2.id, user.id).await.unwrap();
        assert!(label_deleted);
        let labels_final = get_labels_by_user(&db, user.id).await.unwrap();
        assert_eq!(labels_final.len(), 1);
    })
    .await;
}

#[tokio::test]
async fn test_assign_multiple_labels_and_batch_fetch() {
    common::run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "batch_user@example.com", "secretpass").await;
        let alias =
            common::create_test_alias(&db, user.id, "example.com", "testalias", "fwd@example.com")
                .await;

        let label_a = insert_label(&db, user.id, "LabelA", "#3182ce")
            .await
            .unwrap();
        let label_b = insert_label(&db, user.id, "LabelB", "#38a169")
            .await
            .unwrap();
        let label_c = insert_label(&db, user.id, "LabelC", "#d69e2e")
            .await
            .unwrap();

        // Insert two test emails
        let email1 = insert_email(
            &db,
            alias.id,
            "sender1@ext.com",
            "Subj 1",
            Uuid::new_v4(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let email2 = insert_email(
            &db,
            alias.id,
            "sender2@ext.com",
            "Subj 2",
            Uuid::new_v4(),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        // Assign multiple labels: email1 gets A and B, email2 gets B and C
        assign_labels_to_email(&db, email1.id, &[label_a.id, label_b.id])
            .await
            .unwrap();
        assign_labels_to_email(&db, email2.id, &[label_b.id, label_c.id])
            .await
            .unwrap();

        // Verify single email fetch
        let labels_e1 = get_labels_for_email(&db, email1.id).await.unwrap();
        assert_eq!(labels_e1.len(), 2);
        assert!(labels_e1.iter().any(|l| l.id == label_a.id));
        assert!(labels_e1.iter().any(|l| l.id == label_b.id));

        // Verify batch fetch across email IDs (used in dashboard rendering)
        let batch_map = get_labels_for_emails(&db, &[email1.id, email2.id])
            .await
            .unwrap();
        assert_eq!(batch_map.get(&email1.id).unwrap().len(), 2);
        assert_eq!(batch_map.get(&email2.id).unwrap().len(), 2);

        // Remove a single label from email1
        let removed = remove_label_from_email(&db, email1.id, label_a.id)
            .await
            .unwrap();
        assert!(removed);
        let labels_e1_after = get_labels_for_email(&db, email1.id).await.unwrap();
        assert_eq!(labels_e1_after.len(), 1);
        assert_eq!(labels_e1_after[0].id, label_b.id);
    })
    .await;
}

#[tokio::test]
async fn test_email_deletion_cascades_to_email_labels() {
    common::run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "cascade_user@example.com", "secretpass").await;
        let alias = common::create_test_alias(
            &db,
            user.id,
            "example.com",
            "cascadealias",
            "fwd@example.com",
        )
        .await;

        let label1 = insert_label(&db, user.id, "Work", "#3182ce").await.unwrap();
        let label2 = insert_label(&db, user.id, "Urgent", "#e53e3e")
            .await
            .unwrap();

        let email = insert_email(
            &db,
            alias.id,
            "boss@company.com",
            "Urgent Task",
            Uuid::new_v4(),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        assign_labels_to_email(&db, email.id, &[label1.id, label2.id])
            .await
            .unwrap();

        let before_delete = get_labels_for_email(&db, email.id).await.unwrap();
        assert_eq!(before_delete.len(), 2);

        // Delete email via delete_email_by_id
        let deleted = delete_email_by_id(&db, email.id, user.id).await.unwrap();
        assert!(deleted);

        // Verify database engine cascaded delete to email_labels
        let after_delete = get_labels_for_email(&db, email.id).await.unwrap();
        assert_eq!(after_delete.len(), 0);

        // Verify labels themselves still exist
        let all_user_labels = get_labels_by_user(&db, user.id).await.unwrap();
        assert_eq!(all_user_labels.len(), 2);
    })
    .await;
}

#[tokio::test]
async fn test_auto_cleanup_daily_job_cascades_all_labels() {
    common::run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "autoclean_user@example.com", "secretpass").await;
        let alias =
            common::create_test_alias(&db, user.id, "example.com", "autoclean", "fwd@example.com")
                .await;

        let label = insert_label(&db, user.id, "Archive", "#718096")
            .await
            .unwrap();

        // 1. Insert an expired email (45 days ago)
        let past_date = OffsetDateTime::now_utc() - Duration::days(45);
        let old_email = insert_email(
            &db,
            alias.id,
            "old@sender.com",
            "Old Email",
            Uuid::new_v4(),
            Some(past_date),
            None,
            None,
        )
        .await
        .unwrap();
        assign_labels_to_email(&db, old_email.id, &[label.id])
            .await
            .unwrap();

        // 2. Insert a recent email (today)
        let new_email = insert_email(
            &db,
            alias.id,
            "new@sender.com",
            "New Email",
            Uuid::new_v4(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assign_labels_to_email(&db, new_email.id, &[label.id])
            .await
            .unwrap();

        // Run auto-cleanup for emails older than 30 days
        let deleted_keys = delete_old_emails(&db, 30).await.unwrap();
        assert_eq!(deleted_keys.len(), 1);
        assert_eq!(deleted_keys[0], old_email.body_key);

        // Assert: old email labels are completely purged via ON DELETE CASCADE
        let old_labels = get_labels_for_email(&db, old_email.id).await.unwrap();
        assert_eq!(old_labels.len(), 0);

        // Assert: recent email and its label remain intact
        let new_labels = get_labels_for_email(&db, new_email.id).await.unwrap();
        assert_eq!(new_labels.len(), 1);
        assert_eq!(new_labels[0].id, label.id);
    })
    .await;
}

#[tokio::test]
async fn test_dashboard_label_filtering() {
    common::run_on_all_dbs(|db| async move {
        let user = common::create_test_user(&db, "filter_user@example.com", "secretpass").await;
        let alias = common::create_test_alias(
            &db,
            user.id,
            "example.com",
            "filteralias",
            "fwd@example.com",
        )
        .await;

        let label_finance = insert_label(&db, user.id, "Finance", "#3182ce")
            .await
            .unwrap();
        let label_social = insert_label(&db, user.id, "Social", "#38a169")
            .await
            .unwrap();

        let email1 = insert_email(
            &db,
            alias.id,
            "bank@money.com",
            "Bank Statement",
            Uuid::new_v4(),
            None,
            None,
            None,
        )
        .await
        .unwrap();
        let email2 = insert_email(
            &db,
            alias.id,
            "friend@social.com",
            "Hey there",
            Uuid::new_v4(),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        assign_labels_to_email(&db, email1.id, &[label_finance.id])
            .await
            .unwrap();
        assign_labels_to_email(&db, email2.id, &[label_social.id])
            .await
            .unwrap();

        // 1. Fetch all emails (no label filter)
        let all_emails = get_email_by_user_id(&db, user.id, 10, 0, None, None, None)
            .await
            .unwrap();
        assert_eq!(all_emails.len(), 2);
        let total = get_email_count_by_user_id(&db, user.id, None, None, None)
            .await
            .unwrap();
        assert_eq!(total, 2);

        // 2. Filter by "Finance" label
        let finance_emails =
            get_email_by_user_id(&db, user.id, 10, 0, None, None, Some("Finance".to_string()))
                .await
                .unwrap();
        assert_eq!(finance_emails.len(), 1);
        assert_eq!(finance_emails[0].id, email1.id);
        let finance_count =
            get_email_count_by_user_id(&db, user.id, None, None, Some("Finance".to_string()))
                .await
                .unwrap();
        assert_eq!(finance_count, 1);

        // 3. Filter by "Social" label (case-insensitive test)
        let social_emails =
            get_email_by_user_id(&db, user.id, 10, 0, None, None, Some("social".to_string()))
                .await
                .unwrap();
        assert_eq!(social_emails.len(), 1);
        assert_eq!(social_emails[0].id, email2.id);
    })
    .await;
}

#[tokio::test]
async fn test_smtp_end_to_end_body_filtering_and_multi_labeling() {
    common::run_on_all_dbs(|db| async move {
        let temp_dir = tempfile::tempdir().unwrap();

        let user = common::create_test_user(&db, "smtp_filter_user@example.com", "password").await;
        let alias = common::create_test_alias(&db, user.id, "example.com", "myinbound", "dest@gmail.com").await;

        // Create 2 labels
        let label_invoices = insert_label(&db, user.id, "Invoices", "#3182ce").await.unwrap();
        let label_receipts = insert_label(&db, user.id, "Receipts", "#38a169").await.unwrap();

        // Create filter rules:
        // "invoice" -> Invoices
        // "receipt" -> Receipts
        insert_filter(&db, user.id, "invoice", label_invoices.id).await.unwrap();
        insert_filter(&db, user.id, "receipt", label_receipts.id).await.unwrap();

        let (tx, _) = broadcast::channel::<DashboardEvent>(100);
        let cert_path = temp_dir.path().join("smtp_cert.pem");
        let key_path = temp_dir.path().join("smtp_key.pem");
        common::generate_dummy_certs(&cert_path, &key_path);
        let tls_acceptor = HotReloadAcceptor::new(cert_path, key_path, std::time::Duration::from_millis(100)).unwrap();

        let outbound = Arc::new(OutboundService::new(
            "srs_secret_key_123".to_string(),
            hickory_resolver::TokioResolver::builder_tokio().unwrap().build().unwrap(),
            "example.com".to_string(),
            db.clone(),
            temp_dir.path().to_path_buf(),
        ));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        let rate_limiter = Arc::new(RateLimiter::new());
        let block_path = temp_dir.path().join("blockips.conf");
        let blocklist = Arc::new(Blocklist::new(block_path));
        let limits = InboundLimits {
            tarpit_threshold: 5,
            block_threshold: 10,
            tarpit_duration_secs: 5,
        };

        let filter_engine = FilterEngine::new();

        let db_clone = db.clone();
        let storage_clone = temp_dir.path().to_path_buf();
        let outbound_clone = outbound.clone();
        let tx_clone = tx.clone();
        let rate_limiter_clone = rate_limiter.clone();
        let blocklist_clone = blocklist.clone();
        let filter_engine_clone = filter_engine.clone();

        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut session = SmtpSession::new(
                socket,
                Some(tls_acceptor),
                db_clone,
                storage_clone,
                outbound_clone,
                tx_clone,
                "127.0.0.1".parse().unwrap(),
                rate_limiter_clone,
                blocklist_clone,
                limits,
            )
            .with_filter_engine(filter_engine_clone);

            let _ = session.handle().await;
        });

        // Connect via live TCP client and send email containing BOTH keywords
        let stream = TcpStream::connect(local_addr).await.unwrap();
        let (reader, mut writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        // 220 banner
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("220"));

        writer.write_all(b"EHLO client.test\r\n").await.unwrap();
        loop {
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            if line.starts_with("250 ") {
                break;
            }
        }

        writer.write_all(b"MAIL FROM:<vendor@stripe.com>\r\n").await.unwrap();
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("250"));

        let rcpt_cmd = format!("RCPT TO:<{}@example.com>\r\n", alias.subdomain);
        writer.write_all(rcpt_cmd.as_bytes()).await.unwrap();
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("250"));

        writer.write_all(b"DATA\r\n").await.unwrap();
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("354"));

        // Live email body containing BOTH "invoice" and "receipt"
        let email_payload = "From: vendor@stripe.com\r\nSubject: Monthly Service\r\nContent-Type: text/plain\r\n\r\nHello customer, here is your monthly invoice and payment receipt.\r\n.\r\n";
        writer.write_all(email_payload.as_bytes()).await.unwrap();
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.starts_with("250"));

        writer.write_all(b"QUIT\r\n").await.unwrap();

        // Verify that the email stored in DB was automatically assigned BOTH labels
        let emails = get_email_by_user_id(&db, user.id, 10, 0, None, None, None).await.unwrap();
        assert_eq!(emails.len(), 1);
        let received = &emails[0];

        let applied_labels = get_labels_for_email(&db, received.id).await.unwrap();
        assert_eq!(applied_labels.len(), 2);
        assert!(applied_labels.iter().any(|l| l.id == label_invoices.id));
        assert!(applied_labels.iter().any(|l| l.id == label_receipts.id));
    })
    .await;
}

#[tokio::test]
async fn test_backfill_existing_emails_flow() {
    common::run_on_all_dbs(|db| async move {
        let temp_dir = tempfile::tempdir().unwrap();
        let user = common::create_test_user(&db, "backfill_test_user@example.com", "secretpass").await;
        let alias = common::create_test_alias(&db, user.id, "example.com", "backfill", "fwd@example.com").await;

        let label = insert_label(&db, user.id, "Receipts", "#38a169").await.unwrap();

        // 1. Create 3 existing historical emails
        let key1 = Uuid::new_v4();
        let key2 = Uuid::new_v4();
        let key3 = Uuid::new_v4();

        // Email 1 has "receipt" in plain text body
        let email1 = insert_email(&db, alias.id, "taxi@uber.com", "Ride", key1, None, None, None).await.unwrap();
        tokio::fs::write(
            temp_dir.path().join(format!("{}.eml", key1)),
            b"From: taxi@uber.com\r\nSubject: Ride\r\nContent-Type: text/plain\r\n\r\nHere is your ride receipt of $15.",
        ).await.unwrap();

        // Email 2 has "receipt" in HTML body
        let email2 = insert_email(&db, alias.id, "hotel@hilton.com", "Booking", key2, None, None, None).await.unwrap();
        tokio::fs::write(
            temp_dir.path().join(format!("{}.eml", key2)),
            b"From: hotel@hilton.com\r\nSubject: Booking\r\nContent-Type: text/html\r\n\r\n<p>Thank you for your stay. Here is your <b>Receipt</b>.</p>",
        ).await.unwrap();

        // Email 3 has NO keyword match
        let email3 = insert_email(&db, alias.id, "friend@hey.com", "Hello", key3, None, None, None).await.unwrap();
        tokio::fs::write(
            temp_dir.path().join(format!("{}.eml", key3)),
            b"From: friend@hey.com\r\nSubject: Hello\r\nContent-Type: text/plain\r\n\r\nJust saying hi!",
        ).await.unwrap();

        // Verify none of them have labels before backfill
        assert_eq!(get_labels_for_email(&db, email1.id).await.unwrap().len(), 0);
        assert_eq!(get_labels_for_email(&db, email2.id).await.unwrap().len(), 0);
        assert_eq!(get_labels_for_email(&db, email3.id).await.unwrap().len(), 0);

        // Run the backfill job
        let job = maileroo::filter::BackfillJob {
            user_id: user.id,
            keyword: "receipt".to_string(),
            label_id: label.id,
        };
        maileroo::filter::BackfillEngine::process_job(&db, temp_dir.path(), &job).await;

        // Verify: email1 and email2 are now labeled with "Receipts"
        let labels1 = get_labels_for_email(&db, email1.id).await.unwrap();
        assert_eq!(labels1.len(), 1);
        assert_eq!(labels1[0].id, label.id);

        let labels2 = get_labels_for_email(&db, email2.id).await.unwrap();
        assert_eq!(labels2.len(), 1);
        assert_eq!(labels2[0].id, label.id);

        // Verify: email3 remains unlabeled
        let labels3 = get_labels_for_email(&db, email3.id).await.unwrap();
        assert_eq!(labels3.len(), 0);

        // Verify idempotency: running again causes no errors or duplicate labels
        maileroo::filter::BackfillEngine::process_job(&db, temp_dir.path(), &job).await;
        assert_eq!(get_labels_for_email(&db, email1.id).await.unwrap().len(), 1);
    })
    .await;
}
