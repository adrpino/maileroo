use crate::web::{AdminUser, AppState};
use crate::web::i18n::{Locale, Messages};
use crate::db::Domain;
use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Template)]
#[template(path = "dkim_modal.html")]
pub struct DkimModalTemplate {
    pub domain: Domain,
    pub locale: Locale,
    pub error: Option<String>,
    pub success_message: Option<String>,
}

impl IntoResponse for DkimModalTemplate {
    fn into_response(self) -> Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to render template: {err}"),
            )
                .into_response(),
        }
    }
}

pub async fn dkim_modal_handler(
    _user: AdminUser,
    locale: Locale,
    State(state): State<Arc<AppState>>,
    Path(domain_id): Path<Uuid>,
) -> impl IntoResponse {
    use crate::db::get_domain_by_id;

    let domain = match get_domain_by_id(&state.db, domain_id).await {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, "Domain not found").into_response(),
        Err(e) => {
            tracing::error!("Failed to fetch domain: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    DkimModalTemplate {
        domain,
        locale,
        error: None,
        success_message: None,
    }
    .into_response()
}

pub async fn rotate_dkim_handler(
    _user: AdminUser,
    locale: Locale,
    State(state): State<Arc<AppState>>,
    Path(domain_id): Path<Uuid>,
) -> impl IntoResponse {
    use crate::db::{get_domain_by_id, update_pending_dkim};

    let domain = match get_domain_by_id(&state.db, domain_id).await {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, "Domain not found").into_response(),
        Err(e) => {
            tracing::error!("Failed to fetch domain: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    // Alternating active selectors: "maileroo" <-> "maileroo2"
    let next_selector = if domain.dkim_selector == "maileroo" {
        "maileroo2".to_string()
    } else {
        "maileroo".to_string()
    };

    let (private_pem, public_dns) = match crate::outbound::generate_dkim_key_pair() {
        Ok(keys) => keys,
        Err(e) => {
            tracing::error!("Failed to generate DKIM key pair: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Failed to generate keys").into_response();
        }
    };

    let encrypted_private_key = match crate::crypto::encrypt(&private_pem) {
        Ok(enc) => enc,
        Err(e) => {
            tracing::error!("Failed to encrypt pending private key: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Encryption error").into_response();
        }
    };

    if let Err(e) = update_pending_dkim(
        &state.db,
        domain_id,
        Some(encrypted_private_key),
        Some(public_dns),
        Some(next_selector),
    )
    .await
    {
        tracing::error!("Failed to save pending DKIM key: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "Database save error").into_response();
    }

    let updated_domain = match get_domain_by_id(&state.db, domain_id).await {
        Ok(Some(d)) => d,
        _ => domain,
    };

    DkimModalTemplate {
        domain: updated_domain,
        locale,
        error: None,
        success_message: None,
    }
    .into_response()
}

pub async fn verify_dkim_handler(
    _user: AdminUser,
    locale: Locale,
    State(state): State<Arc<AppState>>,
    Path(domain_id): Path<Uuid>,
) -> impl IntoResponse {
    use crate::db::{get_domain_by_id, promote_pending_dkim};

    let domain = match get_domain_by_id(&state.db, domain_id).await {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, "Domain not found").into_response(),
        Err(e) => {
            tracing::error!("Failed to fetch domain: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    let Some(pending_selector) = domain.pending_dkim_selector.clone() else {
        return DkimModalTemplate {
            domain,
            locale,
            error: Some("No pending DKIM rotation in progress for this domain.".to_string()),
            success_message: None,
        }
        .into_response();
    };

    let Some(pending_public_key) = domain.pending_dkim_public_key.clone() else {
        return DkimModalTemplate {
            domain,
            locale,
            error: Some("Pending public key is missing.".to_string()),
            success_message: None,
        }
        .into_response();
    };

    let status = state
        .dns_scanner
        .check_dkim_record(&domain.name, &pending_selector, &pending_public_key)
        .await;

    if status.ok {
        if let Err(e) = promote_pending_dkim(&state.db, domain_id).await {
            tracing::error!("Failed to promote pending DKIM key: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database promotion error").into_response();
        }
        let updated_domain = match get_domain_by_id(&state.db, domain_id).await {
            Ok(Some(d)) => d,
            _ => domain,
        };
        DkimModalTemplate {
            domain: updated_domain,
            locale,
            error: None,
            success_message: Some("DKIM key successfully verified and activated!".to_string()),
        }
        .into_response()
    } else {
        DkimModalTemplate {
            domain,
            locale,
            error: Some(format!("DNS Verification failed: {}", status.message)),
            success_message: None,
        }
        .into_response()
    }
}

pub async fn cancel_dkim_rotation_handler(
    _user: AdminUser,
    locale: Locale,
    State(state): State<Arc<AppState>>,
    Path(domain_id): Path<Uuid>,
) -> impl IntoResponse {
    use crate::db::{get_domain_by_id, clear_pending_dkim};

    let domain = match get_domain_by_id(&state.db, domain_id).await {
        Ok(Some(d)) => d,
        Ok(None) => return (StatusCode::NOT_FOUND, "Domain not found").into_response(),
        Err(e) => {
            tracing::error!("Failed to fetch domain: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Database error").into_response();
        }
    };

    if let Err(e) = clear_pending_dkim(&state.db, domain_id).await {
        tracing::error!("Failed to clear pending DKIM key: {}", e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "Database clear error").into_response();
    }

    let updated_domain = match get_domain_by_id(&state.db, domain_id).await {
        Ok(Some(d)) => d,
        _ => domain,
    };

    DkimModalTemplate {
        domain: updated_domain,
        locale,
        error: None,
        success_message: None,
    }
    .into_response()
}
