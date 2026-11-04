mod common;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode, header};
use maileroo::db::get_domains;
use std::net::SocketAddr;
use tower::ServiceExt;
use uuid::Uuid;

fn connect_info() -> ConnectInfo<SocketAddr> {
    ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345)))
}

/// Reproduces the production 403: an authenticated session whose POST carries no
/// `X-CSRF-Token` header is rejected by the `AuthenticatedUser` extractor, while the
/// very same session succeeds once the header is present.
///
/// This is the shape a user hit when their session was created by a flow that never
/// issued a CSRF token: the cookie is absent, so the htmx hook sends no header.
#[tokio::test]
async fn test_post_without_csrf_header_is_rejected_but_get_is_allowed() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage = tempfile::tempdir().unwrap();
        let app = common::build_test_app(&db, temp_storage.path().to_path_buf()).await;

        let email = format!("csrf-{}@example.com", Uuid::new_v4());
        let user = common::create_test_user(&db, &email, "super-secure-password").await;
        common::create_test_alias(&db, user.id, "csrf-test.com", "seed", &email).await;

        let domain_id = get_domains(&db)
            .await
            .unwrap()
            .into_iter()
            .find(|d| d.name == "csrf-test.com")
            .expect("test domain should exist")
            .id;

        let auth_cookie =
            common::get_auth_cookie(app.clone(), &email, "super-secure-password").await;
        let csrf_token = common::extract_csrf_token(&auth_cookie);

        // GET is exempt from CSRF: browsing works even with no token.
        let get_req = Request::builder()
            .method("GET")
            .uri("/aliases")
            .header(header::COOKIE, &auth_cookie)
            .extension(connect_info())
            .body(Body::empty())
            .unwrap();
        let get_res = app.clone().oneshot(get_req).await.unwrap();
        assert_eq!(
            get_res.status(),
            StatusCode::OK,
            "GET /aliases must succeed without a CSRF header"
        );

        // Same session, same cookie, state-changing request with no header: 403.
        let body = format!(
            "domain_id={}&custom_subdomain=nocsrf&auto_forward=true",
            domain_id
        );
        let no_header_req = Request::builder()
            .method("POST")
            .uri("/aliases")
            .header(header::COOKIE, &auth_cookie)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .extension(connect_info())
            .body(Body::from(body.clone()))
            .unwrap();
        let no_header_res = app.clone().oneshot(no_header_req).await.unwrap();
        assert_eq!(
            no_header_res.status(),
            StatusCode::FORBIDDEN,
            "POST /aliases without X-CSRF-Token must be rejected with 403"
        );

        // With the header, the identical request is accepted.
        let with_header_req = Request::builder()
            .method("POST")
            .uri("/aliases")
            .header(header::COOKIE, &auth_cookie)
            .header("X-CSRF-Token", &csrf_token)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .extension(connect_info())
            .body(Body::from(body))
            .unwrap();
        let with_header_res = app.clone().oneshot(with_header_req).await.unwrap();
        assert_ne!(
            with_header_res.status(),
            StatusCode::FORBIDDEN,
            "POST /aliases with a matching X-CSRF-Token must not be rejected"
        );
    })
    .await;
}

/// Guards the regression directly: logging in must hand the client a `csrf_token`
/// cookie, otherwise nothing state-changing can ever be submitted from that session.
#[tokio::test]
async fn test_login_issues_csrf_cookie() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage = tempfile::tempdir().unwrap();
        let app = common::build_test_app(&db, temp_storage.path().to_path_buf()).await;

        let email = format!("login-csrf-{}@example.com", Uuid::new_v4());
        common::create_test_user(&db, &email, "super-secure-password").await;

        let req = Request::builder()
            .method("POST")
            .uri("/login")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .extension(connect_info())
            .body(Body::from(format!(
                "email={}&password=super-secure-password",
                email
            )))
            .unwrap();

        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let cookies = common::collect_cookies(&res);
        assert!(
            cookies.contains("csrf_token="),
            "login must set a csrf_token cookie, got: {}",
            cookies
        );
        assert!(
            !common::extract_csrf_token(&cookies).is_empty(),
            "csrf_token cookie must not be empty"
        );
    })
    .await;
}

/// The actual bug: registration logged the user straight in but never issued a CSRF
/// token, so the brand-new session could browse yet was rejected with 403 on every
/// state-changing request until the user logged out and back in.
///
/// `/register` gates on a live MX/A lookup of the email domain, so this test needs
/// outbound DNS. When DNS is unavailable it reports and returns rather than failing
/// for an unrelated reason.
#[tokio::test]
async fn test_registration_issues_csrf_token_and_allows_state_changing_requests() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage = tempfile::tempdir().unwrap();
        let app = common::build_test_app(&db, temp_storage.path().to_path_buf()).await;

        let email = format!("reg-csrf-{}@gmail.com", Uuid::new_v4());
        let register_req = Request::builder()
            .method("POST")
            .uri("/register")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .extension(connect_info())
            .body(Body::from(format!(
                "email={}&password=super-secure-password",
                email
            )))
            .unwrap();

        let register_res = app.clone().oneshot(register_req).await.unwrap();
        assert_eq!(register_res.status(), StatusCode::OK);

        let register_cookies = common::collect_cookies(&register_res);
        if !register_cookies.contains("id=") {
            eprintln!(
                "SKIP: registration did not create a session (DNS deliverability check \
                 unavailable?); cookies={}",
                register_cookies
            );
            return;
        }

        // The regression assertion: the session born from /register must carry a token.
        assert!(
            register_cookies.contains("csrf_token="),
            "registration must set a csrf_token cookie, got: {}",
            register_cookies
        );

        let csrf_token = common::extract_csrf_token(&register_cookies);
        assert!(
            !csrf_token.is_empty(),
            "csrf_token cookie must not be empty"
        );

        // Registration logs the user in, so it must be recorded as their first login.
        let registered = maileroo::db::get_user_by_email(&db, &email)
            .await
            .unwrap()
            .expect("registered user should exist");
        assert!(
            registered.last_login_at.is_some(),
            "registration must record last_login_at"
        );
        assert_eq!(registered.last_login_ip.as_deref(), Some("127.0.0.1"));

        // And that session must actually be able to POST, without re-logging in.
        common::create_test_alias(
            &db,
            maileroo::db::get_user_by_email(&db, &email)
                .await
                .unwrap()
                .expect("registered user should exist")
                .id,
            "reg-csrf-test.com",
            "seed",
            &email,
        )
        .await;

        let domain_id = get_domains(&db)
            .await
            .unwrap()
            .into_iter()
            .find(|d| d.name == "reg-csrf-test.com")
            .expect("test domain should exist")
            .id;

        let create_req = Request::builder()
            .method("POST")
            .uri("/aliases")
            .header(header::COOKIE, &register_cookies)
            .header("X-CSRF-Token", &csrf_token)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .extension(connect_info())
            .body(Body::from(format!(
                "domain_id={}&custom_subdomain=afterregister&auto_forward=true",
                domain_id
            )))
            .unwrap();

        let create_res = app.oneshot(create_req).await.unwrap();
        assert_ne!(
            create_res.status(),
            StatusCode::FORBIDDEN,
            "a freshly registered session must be able to create an alias"
        );
    })
    .await;
}

fn extract_session_id(cookie_header: &str) -> Option<String> {
    cookie_header
        .split(';')
        .map(str::trim)
        .find(|part| part.starts_with("id="))
        .and_then(|part| part.strip_prefix("id="))
        .map(str::to_string)
}

#[tokio::test]
async fn test_session_id_cycled_on_login_prevents_session_fixation() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage = tempfile::tempdir().unwrap();
        let app = common::build_test_app(&db, temp_storage.path().to_path_buf()).await;

        let email1 = format!("fixation-a-{}@example.com", Uuid::new_v4());
        let email2 = format!("fixation-b-{}@example.com", Uuid::new_v4());
        common::create_test_user(&db, &email1, "password-one-123").await;
        common::create_test_user(&db, &email2, "password-two-123").await;

        // 1. Obtain an initial valid session cookie (or planted pre-login session ID).
        let first_cookie = common::get_auth_cookie(app.clone(), &email1, "password-one-123").await;
        let old_session_id =
            extract_session_id(&first_cookie).expect("first login must set an id= cookie");

        // 2. Log in again while sending the existing session cookie (`id=<old_session_id>`).
        let relogin_req = Request::builder()
            .method("POST")
            .uri("/login")
            .header(header::COOKIE, format!("id={}", old_session_id))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .extension(connect_info())
            .body(Body::from(format!(
                "email={}&password=password-two-123",
                email2
            )))
            .unwrap();

        let relogin_res = app.clone().oneshot(relogin_req).await.unwrap();
        assert_eq!(relogin_res.status(), StatusCode::OK);

        let second_cookie = common::collect_cookies(&relogin_res);
        let new_session_id = extract_session_id(&second_cookie)
            .expect("second login must issue a cycled id= Set-Cookie header");

        assert_ne!(
            old_session_id, new_session_id,
            "establish_session must cycle the session ID on login"
        );

        // 3. The old session ID must now be invalid in the backing store.
        let old_dash_req = Request::builder()
            .method("GET")
            .uri("/dashboard")
            .header(header::COOKIE, format!("id={}", old_session_id))
            .extension(connect_info())
            .body(Body::empty())
            .unwrap();
        let old_dash_res = app.clone().oneshot(old_dash_req).await.unwrap();
        assert_eq!(
            old_dash_res.status(),
            StatusCode::SEE_OTHER,
            "old session ID must be invalidated after cycle_id()"
        );

        // 4. The newly cycled session ID must be active and authenticated.
        let new_dash_req = Request::builder()
            .method("GET")
            .uri("/dashboard")
            .header(header::COOKIE, &second_cookie)
            .extension(connect_info())
            .body(Body::empty())
            .unwrap();
        let new_dash_res = app.oneshot(new_dash_req).await.unwrap();
        assert_eq!(
            new_dash_res.status(),
            StatusCode::OK,
            "new cycled session ID must authenticate the user"
        );
    })
    .await;
}

#[tokio::test]
async fn test_logout_flushes_session_and_invalidates_old_session_id() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage = tempfile::tempdir().unwrap();
        let app = common::build_test_app(&db, temp_storage.path().to_path_buf()).await;

        let email = format!("logout-flush-{}@example.com", Uuid::new_v4());
        common::create_test_user(&db, &email, "super-secure-password").await;

        let auth_cookie =
            common::get_auth_cookie(app.clone(), &email, "super-secure-password").await;
        let csrf_token = common::extract_csrf_token(&auth_cookie);

        // Verify session works before logout
        let before_req = Request::builder()
            .method("GET")
            .uri("/dashboard")
            .header(header::COOKIE, &auth_cookie)
            .extension(connect_info())
            .body(Body::empty())
            .unwrap();
        let before_res = app.clone().oneshot(before_req).await.unwrap();
        assert_eq!(before_res.status(), StatusCode::OK);

        // Logout without CSRF token must be rejected with 403
        let logout_no_csrf_req = Request::builder()
            .method("POST")
            .uri("/logout")
            .header(header::COOKIE, &auth_cookie)
            .extension(connect_info())
            .body(Body::empty())
            .unwrap();
        let logout_no_csrf_res = app.clone().oneshot(logout_no_csrf_req).await.unwrap();
        assert_eq!(
            logout_no_csrf_res.status(),
            StatusCode::FORBIDDEN,
            "POST /logout without X-CSRF-Token must be rejected with 403"
        );

        // Perform logout with valid CSRF token
        let logout_req = Request::builder()
            .method("POST")
            .uri("/logout")
            .header(header::COOKIE, &auth_cookie)
            .header("X-CSRF-Token", &csrf_token)
            .extension(connect_info())
            .body(Body::empty())
            .unwrap();
        let logout_res = app.clone().oneshot(logout_req).await.unwrap();
        assert_eq!(logout_res.status(), StatusCode::OK);

        // Replaying the old session cookie must be rejected
        let after_req = Request::builder()
            .method("GET")
            .uri("/dashboard")
            .header(header::COOKIE, &auth_cookie)
            .extension(connect_info())
            .body(Body::empty())
            .unwrap();
        let after_res = app.oneshot(after_req).await.unwrap();
        assert_eq!(
            after_res.status(),
            StatusCode::SEE_OTHER,
            "replaying a flushed session cookie after logout must redirect to /login"
        );
    })
    .await;
}

#[tokio::test]
async fn test_login_nonexistent_user_and_wrong_password_return_identical_response() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage = tempfile::tempdir().unwrap();
        let app = common::build_test_app(&db, temp_storage.path().to_path_buf()).await;

        let existing_email = format!("timing-exists-{}@example.com", Uuid::new_v4());
        let missing_email = format!("timing-missing-{}@example.com", Uuid::new_v4());
        common::create_test_user(&db, &existing_email, "correct-password-123").await;

        // Warm up the dummy Argon2 hash so initialization is not measured
        let _ = maileroo::web::auth::dummy_password_hash();

        // 1. Login attempt for non-existent user
        let start_missing = std::time::Instant::now();
        let missing_req = Request::builder()
            .method("POST")
            .uri("/login")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .extension(connect_info())
            .body(Body::from(format!(
                "email={}&password=wrong-password-123",
                missing_email
            )))
            .unwrap();
        let missing_res = app.clone().oneshot(missing_req).await.unwrap();
        let missing_elapsed = start_missing.elapsed();

        // 2. Login attempt for existing user with wrong password
        let wrong_req = Request::builder()
            .method("POST")
            .uri("/login")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .extension(connect_info())
            .body(Body::from(format!(
                "email={}&password=wrong-password-123",
                existing_email
            )))
            .unwrap();
        let wrong_res = app.oneshot(wrong_req).await.unwrap();

        assert_eq!(missing_res.status(), wrong_res.status());
        let missing_body = axum::body::to_bytes(missing_res.into_body(), usize::MAX)
            .await
            .unwrap();
        let wrong_body = axum::body::to_bytes(wrong_res.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(
            missing_body, wrong_body,
            "response bodies for non-existent user and wrong password must be identical"
        );
        assert!(
            missing_elapsed >= std::time::Duration::from_millis(5),
            "non-existent user login should perform Argon2 verification (took {:?})",
            missing_elapsed
        );
    })
    .await;
}

#[tokio::test]
async fn test_permission_changes_apply_immediately_without_relogin() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage = tempfile::tempdir().unwrap();
        let app = common::build_test_app(&db, temp_storage.path().to_path_buf()).await;

        let email = format!("perms-live-{}@example.com", Uuid::new_v4());
        let user = common::create_test_user(&db, &email, "super-secure-password").await;

        let auth_cookie =
            common::get_auth_cookie(app.clone(), &email, "super-secure-password").await;

        // 1. Initially can_send_firsthand is false -> /api/v1/emails/compose returns 403
        let req1 = Request::builder()
            .method("GET")
            .uri("/api/v1/emails/compose")
            .header(header::COOKIE, &auth_cookie)
            .extension(connect_info())
            .body(Body::empty())
            .unwrap();
        let res1 = app.clone().oneshot(req1).await.unwrap();
        assert_eq!(res1.status(), StatusCode::FORBIDDEN);

        // 2. Grant can_send_firsthand in DB while user's session is already established
        let enabled = maileroo::db::users::toggle_can_send_firsthand(&db, user.id)
            .await
            .unwrap();
        assert!(enabled);

        // 3. Same session cookie immediately sees the granted permission without re-logging in
        let req2 = Request::builder()
            .method("GET")
            .uri("/api/v1/emails/compose")
            .header(header::COOKIE, &auth_cookie)
            .extension(connect_info())
            .body(Body::empty())
            .unwrap();
        let res2 = app.clone().oneshot(req2).await.unwrap();
        assert_eq!(
            res2.status(),
            StatusCode::OK,
            "permission grant must take effect immediately without re-login"
        );

        // 4. Revoke can_send_firsthand in DB -> immediately forbidden on next request
        let enabled_after = maileroo::db::users::toggle_can_send_firsthand(&db, user.id)
            .await
            .unwrap();
        assert!(!enabled_after);

        let req3 = Request::builder()
            .method("GET")
            .uri("/api/v1/emails/compose")
            .header(header::COOKIE, &auth_cookie)
            .extension(connect_info())
            .body(Body::empty())
            .unwrap();
        let res3 = app.oneshot(req3).await.unwrap();
        assert_eq!(
            res3.status(),
            StatusCode::FORBIDDEN,
            "permission revocation must take effect immediately without re-login"
        );
    })
    .await;
}

#[tokio::test]
async fn test_rate_limit_cannot_be_bypassed_with_spoofed_x_forwarded_for() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage = tempfile::tempdir().unwrap();
        let app = common::build_test_app(&db, temp_storage.path().to_path_buf()).await;

        let attacker_socket = ConnectInfo(SocketAddr::from(([198, 51, 100, 77], 54321)));
        let mut saw_rate_limit = false;

        // Burst size is 5. Send 8 requests from the same peer IP with rotating fake X-Forwarded-For headers.
        for i in 1..=8 {
            let req = Request::builder()
                .method("GET")
                .uri("/login")
                .header("X-Forwarded-For", format!("203.0.113.{}", i))
                .extension(attacker_socket)
                .body(Body::empty())
                .unwrap();
            let res = app.clone().oneshot(req).await.unwrap();
            if res.status() == StatusCode::TOO_MANY_REQUESTS {
                saw_rate_limit = true;
                break;
            }
        }

        assert!(
            saw_rate_limit,
            "rotating X-Forwarded-For from an untrusted peer must not bypass the rate limiter"
        );
    })
    .await;
}
