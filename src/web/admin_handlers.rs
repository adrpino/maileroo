use crate::web::i18n::{Locale, Messages};
use crate::web::{AdminUser, AppState};
use askama::Template;
use axum::http::StatusCode;
use axum::{
    extract::State,
    response::{Html, IntoResponse},
};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Template)]
#[template(path = "admin_users.html")]
pub struct AdminUsersTemplate {
    pub users: Vec<crate::db::UserWithStats>,
    pub locale: Locale,
    pub is_admin: bool,
}

impl IntoResponse for AdminUsersTemplate {
    fn into_response(self) -> axum::response::Response {
        match self.render() {
            Ok(html) => Html(html).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Template rendering error: {}", err),
            )
                .into_response(),
        }
    }
}

pub async fn admin_users_handler(
    locale: Locale,
    user: AdminUser,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let users = match crate::db::get_all_users_with_stats(&state.db).await {
        Ok(u) => u,
        Err(_) => vec![],
    };

    AdminUsersTemplate {
        users,
        locale,
        is_admin: user.0.is_admin,
    }
}

pub fn render_toggle_td(
    is_enabled: bool,
    tooltip: &str,
    post_url: &str,
    status_yes: &str,
    status_no: &str,
) -> String {
    let status_class = if is_enabled {
        "maileroo-status-enabled"
    } else {
        "maileroo-status-disabled"
    };
    let status_text = if is_enabled { status_yes } else { status_no };
    let checked = if is_enabled { "checked" } else { "" };

    format!(
        r#"<td class="desktop-only text-center">
               <div class="maileroo-toggle-wrapper">
                   <label class="maileroo-toggle-switch" title="{tooltip}">
                       <input type="checkbox"
                              {checked}
                              hx-post="{post_url}"
                              hx-swap="outerHTML"
                              hx-target="closest td">
                       <span class="maileroo-toggle-slider"></span>
                   </label>
                   <span class="{status_class}">
                       {status_text}
                   </span>
               </div>
           </td>"#
    )
}

pub async fn toggle_bypass_limit_handler(
    locale: Locale,
    _user: AdminUser,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(target_user_id): axum::extract::Path<Uuid>,
) -> impl IntoResponse {
    let new_val = match crate::db::users::toggle_bypass_alias_limit(&state.db, target_user_id).await
    {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to toggle limit bypass",
            )
                .into_response();
        }
    };

    Html(render_toggle_td(
        new_val,
        locale.admin_bypass_limit_tooltip(),
        &format!("/admin/users/{}/bypass_limit", target_user_id),
        locale.status_yes(),
        locale.status_no(),
    ))
    .into_response()
}

pub async fn toggle_disable_autoclean_handler(
    locale: Locale,
    _user: AdminUser,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(target_user_id): axum::extract::Path<Uuid>,
) -> impl IntoResponse {
    let new_val = match crate::db::users::toggle_disable_autoclean(&state.db, target_user_id).await
    {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to toggle disable autoclean",
            )
                .into_response();
        }
    };

    Html(render_toggle_td(
        new_val,
        locale.admin_disable_autoclean_tooltip(),
        &format!("/admin/users/{}/disable_autoclean", target_user_id),
        locale.status_yes(),
        locale.status_no(),
    ))
    .into_response()
}

pub async fn toggle_can_send_firsthand_handler(
    locale: Locale,
    _user: AdminUser,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(target_user_id): axum::extract::Path<Uuid>,
) -> impl IntoResponse {
    let new_val = match crate::db::users::toggle_can_send_firsthand(&state.db, target_user_id).await
    {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to toggle outbound email",
            )
                .into_response();
        }
    };

    Html(render_toggle_td(
        new_val,
        locale.admin_outbound_email_tooltip(),
        &format!("/admin/users/{}/can_send_firsthand", target_user_id),
        locale.status_yes(),
        locale.status_no(),
    ))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_toggle_td_enabled() {
        let html = render_toggle_td(true, "Tooltip yes", "/test/url", "Yes", "No");
        assert!(html.contains("checked"));
        assert!(html.contains("maileroo-status-enabled"));
        assert!(html.contains("Yes"));
        assert!(html.contains("/test/url"));
    }

    #[test]
    fn test_render_toggle_td_disabled() {
        let html = render_toggle_td(false, "Tooltip no", "/test/url", "Yes", "No");
        assert!(!html.contains("checked"));
        assert!(html.contains("maileroo-status-disabled"));
        assert!(html.contains("No"));
        assert!(html.contains("/test/url"));
    }
}
