mod common;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, header};
use maileroo::db::{get_alias_count, get_domains};
use maileroo::web::alias_api::TAKEN_ANSWERS_PER_ACCOUNT_PER_HOUR;
use std::net::SocketAddr;
use tower::ServiceExt;
use uuid::Uuid;

const TAKEN: &str = "this alias was just taken";
const TOO_MANY: &str = "Too many of the names you tried are taken";

/// Submits the alias form for `name` and returns whether it redirected (the alias was
/// created) and the page body.
async fn submit_alias(
    app: &Router,
    cookie: &str,
    csrf_token: &str,
    domain_id: Uuid,
    name: &str,
) -> (bool, String) {
    let req = Request::builder()
        .method("POST")
        .uri("/aliases")
        .header(header::COOKIE, cookie)
        .header("X-CSRF-Token", csrf_token)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .extension(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))))
        .body(Body::from(format!(
            "domain_id={}&custom_subdomain={}&auto_forward=true",
            domain_id, name
        )))
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let created = res.headers().contains_key("HX-Redirect");
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (created, String::from_utf8_lossy(&body).into_owned())
}

/// The alias form says "taken" only so many times an hour; after that it refuses every
/// name, free ones included, so it can't be used to map which addresses exist.
#[tokio::test]
async fn test_alias_form_stops_answering_after_too_many_taken_names() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage = tempfile::tempdir().unwrap();
        let app = common::build_test_app(&db, temp_storage.path().to_path_buf()).await;

        let owner_email = format!("owner-{}@example.com", Uuid::new_v4());
        let owner = common::create_test_user(&db, &owner_email, "super-secure-password").await;
        common::create_test_alias(&db, owner.id, "guess-test.com", "taken", &owner_email).await;
        let domain_id = get_domains(&db)
            .await
            .unwrap()
            .into_iter()
            .find(|d| d.name == "guess-test.com")
            .expect("test domain should exist")
            .id;

        let email = format!("guesser-{}@example.com", Uuid::new_v4());
        let user = common::create_test_user(&db, &email, "super-secure-password").await;
        let cookie = common::get_auth_cookie(app.clone(), &email, "super-secure-password").await;
        let csrf_token = common::extract_csrf_token(&cookie);

        let (created, _) = submit_alias(&app, &cookie, &csrf_token, domain_id, "mine").await;
        assert!(created, "a free name is created before any limit applies");

        for _ in 0..TAKEN_ANSWERS_PER_ACCOUNT_PER_HOUR {
            let (created, body) =
                submit_alias(&app, &cookie, &csrf_token, domain_id, "taken").await;
            assert!(!created);
            assert!(body.contains(TAKEN), "a taken name is reported as taken");
        }

        let (created, body) = submit_alias(&app, &cookie, &csrf_token, domain_id, "free").await;
        assert!(!created, "past the limit even a free name is refused");
        assert!(body.contains(TOO_MANY));
        assert!(!body.contains(TAKEN));
        assert_eq!(get_alias_count(&db, user.id).await.unwrap(), 1);
    })
    .await;
}

/// Administrators are not limited by the alias form's guess counter.
#[tokio::test]
async fn test_admins_are_not_limited_by_taken_names() {
    common::run_on_all_dbs(|db| async move {
        let temp_storage = tempfile::tempdir().unwrap();
        let app = common::build_test_app(&db, temp_storage.path().to_path_buf()).await;

        let email = format!("admin-{}@example.com", Uuid::new_v4());
        let admin = common::create_test_user(&db, &email, "super-secure-password").await;
        common::grant_user_sender_permissions(&db, admin.id).await;
        common::create_test_alias(&db, admin.id, "admin-guess.com", "taken", &email).await;
        let domain_id = get_domains(&db)
            .await
            .unwrap()
            .into_iter()
            .find(|d| d.name == "admin-guess.com")
            .expect("test domain should exist")
            .id;

        let cookie = common::get_auth_cookie(app.clone(), &email, "super-secure-password").await;
        let csrf_token = common::extract_csrf_token(&cookie);

        for _ in 0..=TAKEN_ANSWERS_PER_ACCOUNT_PER_HOUR {
            let (_, body) = submit_alias(&app, &cookie, &csrf_token, domain_id, "taken").await;
            assert!(body.contains(TAKEN));
        }
        let (created, _) = submit_alias(&app, &cookie, &csrf_token, domain_id, "free").await;
        assert!(created);
    })
    .await;
}
