#[cfg(test)]
mod tests {
    use crate::db::{
        DbPool, insert_domain,
        aliases::{insert_alias, get_taken_subdomains, resolve_recipient_alias},
        users::{insert_user, get_user_by_id, update_last_login},
        api_keys::{insert_api_key, get_api_keys},
        sent_emails::{insert_sent_email, get_sent_emails_by_user_id, EmailStatus},
        reply_mappings::{get_or_create_reply_mapping, get_reply_mapping_by_token},
        insert_email, find_thread_id_by_references, get_child_emails,
    };
    use sqlx::SqlitePool;
    use uuid::Uuid;

    async fn setup_sqlite_in_memory_db() -> DbPool {
        // 1. Boot up an in-memory SQLite pool
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let db_pool = DbPool::Sqlite(pool);

        // 2. Setup the exact schema using our unified sqlite migrations!
        crate::db::run_migrations(&db_pool).await.unwrap();

        db_pool
    }

    #[tokio::test]
    async fn test_sqlite_users_and_api_keys_flow() {
        let db = setup_sqlite_in_memory_db().await;

        // 1. Insert user
        let user = insert_user(&db, "developer@example.com", "my_secure_hash")
            .await
            .expect("Failed to insert user");
        
        assert_eq!(user.email, "developer@example.com");
        assert!(!user.is_admin);

        // 2. Fetch user
        let fetched = get_user_by_id(&db, user.id).await.unwrap().unwrap();
        assert_eq!(fetched.id, user.id);

        // 3. Update last login
        update_last_login(&db, user.id, Some("127.0.0.1".to_string())).await.unwrap();

        // 4. Insert Api Key
        let api_key = insert_api_key(&db, user.id, "some_key_hash", "Admin Token")
            .await
            .unwrap();
        assert_eq!(api_key.name, "Admin Token");

        // 5. Get Api keys
        let keys = get_api_keys(&db, user.id).await.unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].id, api_key.id);
    }

    #[tokio::test]
    async fn test_sqlite_alias_suggestions_and_taken_subdomains() {
        let db = setup_sqlite_in_memory_db().await;

        // Insert user
        let user = insert_user(&db, "test@suggestions.com", "hash").await.unwrap();

        // Insert domain
        let domain = insert_domain(&db, &"suggestions-domain.com".to_string())
            .await
            .unwrap();

        // Insert some aliases
        let alias1 = insert_alias(&db, user.id, domain.id, "support", "dest1@gmail.com", true).await.unwrap();
        let _alias2 = insert_alias(&db, user.id, domain.id, "billing", "dest2@gmail.com", true).await.unwrap();

        assert_eq!(alias1.subdomain, "support");
        assert_eq!(alias1.domain_name, "suggestions-domain.com");

        // Check taken subdomains
        let candidates = vec!["support".to_string(), "billing".to_string(), "sales".to_string()];
        let taken = get_taken_subdomains(&db, domain.id, &candidates).await.unwrap();
        assert_eq!(taken.len(), 2);
        assert!(taken.contains(&"support".to_string()));
        assert!(taken.contains(&"billing".to_string()));
        assert!(!taken.contains(&"sales".to_string()));

        // Resolve alias
        let lookup = resolve_recipient_alias(&db, "support", "suggestions-domain.com").await.unwrap().unwrap();
        assert_eq!(lookup.destination_email, "dest1@gmail.com");
    }

    #[tokio::test]
    async fn test_sqlite_replies_and_sent_emails() {
        let db = setup_sqlite_in_memory_db().await;

        // Setup basic records
        let user = insert_user(&db, "forwarder@test.com", "hash").await.unwrap();

        let domain = insert_domain(&db, &"forward-domain.com".to_string())
            .await
            .unwrap();

        let alias = insert_alias(&db, user.id, domain.id, "hello", "user@gmail.com", true).await.unwrap();

        // Insert sent email draft
        let body_key = Uuid::new_v4();
        let sent_email = insert_sent_email(
            &db,
            user.id,
            alias.id,
            "recipient@external.com",
            "Hello World Subject",
            body_key,
            EmailStatus::Draft,
            None,
        )
        .await
        .unwrap();

        assert_eq!(sent_email.subject, "Hello World Subject");
        assert_eq!(sent_email.status, EmailStatus::Draft);

        // List sent emails by user
        let sent_emails_list = get_sent_emails_by_user_id(
            &db,
            user.id,
            EmailStatus::Draft,
            10,
            0,
            None,
            None,
        )
        .await
        .unwrap();

        assert_eq!(sent_emails_list.len(), 1);
        assert_eq!(sent_emails_list[0].id, sent_email.id);
        assert_eq!(sent_emails_list[0].alias_address, "hello@forward-domain.com");
    }

    #[tokio::test]
    async fn test_sqlite_reply_mappings_lifecycle() {
        let db = setup_sqlite_in_memory_db().await;

        let user = insert_user(&db, "replies@test.com", "hash").await.unwrap();

        let domain = insert_domain(&db, &"replies-domain.com".to_string())
            .await
            .unwrap();

        let alias = insert_alias(&db, user.id, domain.id, "replier", "user@gmail.com", true).await.unwrap();

        let sender = "customer@external.com";

        // 1. Create mapping
        let map1 = get_or_create_reply_mapping(&db, alias.id, sender).await.unwrap();
        assert_eq!(map1.alias_id, alias.id);
        assert_eq!(map1.original_sender, sender);
        assert!(map1.anonymous_token.starts_with("reply-"));

        // 2. Fetch existing mapping
        let map2 = get_or_create_reply_mapping(&db, alias.id, sender).await.unwrap();
        assert_eq!(map1.id, map2.id);
        assert_eq!(map1.anonymous_token, map2.anonymous_token);

        // 3. Lookup by token
        let lookup = get_reply_mapping_by_token(&db, &map1.anonymous_token).await.unwrap().unwrap();
        assert_eq!(lookup.id, map1.id);
        assert_eq!(lookup.original_sender, sender);
        assert_eq!(lookup.destination_email, "user@gmail.com");
    }

    #[tokio::test]
    async fn test_sqlite_find_thread_id_by_references() {
        let db = setup_sqlite_in_memory_db().await;

        let user = insert_user(&db, "threads@test.com", "hash").await.unwrap();

        let domain = insert_domain(&db, &"threads-domain.com".to_string())
            .await
            .unwrap();

        let alias = insert_alias(&db, user.id, domain.id, "threading", "user@gmail.com", true).await.unwrap();

        // 1. Insert root email
        let root_msg_id = "<root-msg-123@external.com>";
        let root_email = insert_email(
            &db,
            alias.id,
            "sender@external.com",
            "Initial thread subject",
            Uuid::new_v4(),
            None,
            Some(root_msg_id.to_string()),
            None,
        )
        .await
        .unwrap();

        // 2. Insert child email on thread
        let child_msg_id = "<reply-msg-456@example.com>";
        let _child_email = insert_email(
            &db,
            alias.id,
            "sender@external.com",
            "Re: Initial thread subject",
            Uuid::new_v4(),
            None,
            Some(child_msg_id.to_string()),
            Some(root_email.id),
        )
        .await
        .unwrap();

        // Test resolving thread ID by references
        let references = vec![
            "non-existent-msg-id".to_string(),
            root_msg_id.to_string(),
        ];

        let found_id = find_thread_id_by_references(&db, &references).await.unwrap().unwrap();
        assert_eq!(found_id, root_email.id);

        // Fetch children
        let children = get_child_emails(&db, root_email.id).await.unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].thread_id, Some(root_email.id));
    }
}
