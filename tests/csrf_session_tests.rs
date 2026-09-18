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
