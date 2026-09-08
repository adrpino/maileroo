use askama::Template;
use axum::{
    Form,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::db::labels::{
    EmailFilterWithLabel, Label, assign_labels_to_email, delete_filter, delete_label,
    get_filters_by_user, get_labels_by_user, get_labels_for_email, insert_filter, insert_label,
    remove_label_from_email,
};
use crate::filter::BackfillJob;
use crate::web::i18n::{Locale, Messages};
use crate::web::{AppState, AuthenticatedUser};

#[derive(Deserialize)]
pub struct ModalQuery {
    pub tab: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateLabelForm {
    pub name: String,
    pub color: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateFilterForm {
    pub keyword: String,
    pub label_id: Uuid,
    pub apply_to_existing: Option<bool>,
}

#[derive(Deserialize)]
pub struct AddEmailLabelForm {
    pub label_id: Uuid,
}

#[derive(Template)]
#[template(path = "labels_modal.html")]
pub struct LabelsModalTemplate {
    pub locale: Locale,
    pub labels: Vec<Label>,
    pub filters: Vec<EmailFilterWithLabel>,
    pub active_tab: String,
    pub error: Option<String>,
}

impl IntoResponse for LabelsModalTemplate {
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

#[derive(Template)]
#[template(path = "email_labels_partial.html")]
pub struct EmailLabelsPartialTemplate {
    pub locale: Locale,
    pub email_id: Uuid,
    pub labels: Vec<Label>,
    pub all_user_labels: Vec<Label>,
}

impl IntoResponse for EmailLabelsPartialTemplate {
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

pub async fn get_labels_modal_handler(
    locale: Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Query(query): Query<ModalQuery>,
) -> impl IntoResponse {
    let active_tab = query.tab.unwrap_or_else(|| "filters".to_string());
    let labels = get_labels_by_user(&state.db, user.user_id)
        .await
        .unwrap_or_default();
    let filters = get_filters_by_user(&state.db, user.user_id)
        .await
        .unwrap_or_default();

    LabelsModalTemplate {
        locale,
        labels,
        filters,
        active_tab,
        error: None,
    }
}

pub async fn create_label_handler(
    locale: Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<CreateLabelForm>,
) -> impl IntoResponse {
    let name = form.name.trim();
    if name.is_empty() {
        let labels = get_labels_by_user(&state.db, user.user_id)
            .await
            .unwrap_or_default();
        let filters = get_filters_by_user(&state.db, user.user_id)
            .await
            .unwrap_or_default();
        return LabelsModalTemplate {
            locale,
            labels,
            filters,
            active_tab: "labels".to_string(),
            error: Some(locale.error_label_name_required().to_string()),
        }
        .into_response();
    }

    let color = form
        .color
        .filter(|c| !c.trim().is_empty())
        .unwrap_or_else(|| "#3182ce".to_string());

    let _ = insert_label(&state.db, user.user_id, name, &color).await;

    let labels = get_labels_by_user(&state.db, user.user_id)
        .await
        .unwrap_or_default();
    let filters = get_filters_by_user(&state.db, user.user_id)
        .await
        .unwrap_or_default();

    LabelsModalTemplate {
        locale,
        labels,
        filters,
        active_tab: "labels".to_string(),
        error: None,
    }
    .into_response()
}

pub async fn delete_label_handler(
    locale: Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Path(label_id): Path<Uuid>,
) -> impl IntoResponse {
    let _ = delete_label(&state.db, label_id, user.user_id).await;
    state.filter_engine.invalidate(&user.user_id);

    let labels = get_labels_by_user(&state.db, user.user_id)
        .await
        .unwrap_or_default();
    let filters = get_filters_by_user(&state.db, user.user_id)
        .await
        .unwrap_or_default();

    LabelsModalTemplate {
        locale,
        labels,
        filters,
        active_tab: "labels".to_string(),
        error: None,
    }
}

pub async fn create_filter_handler(
    locale: Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Form(form): Form<CreateFilterForm>,
) -> impl IntoResponse {
    let keyword = form.keyword.trim();
    if keyword.is_empty() {
        let labels = get_labels_by_user(&state.db, user.user_id)
            .await
            .unwrap_or_default();
        let filters = get_filters_by_user(&state.db, user.user_id)
            .await
            .unwrap_or_default();
        return LabelsModalTemplate {
            locale,
            labels,
            filters,
            active_tab: "filters".to_string(),
            error: Some(locale.error_filter_keyword_required().to_string()),
        }
        .into_response();
    }

    // Support comma-separated keywords (e.g. "invoice, receipt, payment")
    let keywords: Vec<&str> = keyword
        .split(',')
        .map(|k| k.trim())
        .filter(|k| !k.is_empty())
        .collect();
    for kw in keywords {
        let _ = insert_filter(&state.db, user.user_id, kw, form.label_id).await;
        if form.apply_to_existing == Some(true) {
            state.backfill_engine.submit(BackfillJob {
                user_id: user.user_id,
                keyword: kw.to_string(),
                label_id: form.label_id,
            });
        }
    }

    state.filter_engine.invalidate(&user.user_id);

    let labels = get_labels_by_user(&state.db, user.user_id)
        .await
        .unwrap_or_default();
    let filters = get_filters_by_user(&state.db, user.user_id)
        .await
        .unwrap_or_default();

    LabelsModalTemplate {
        locale,
        labels,
        filters,
        active_tab: "filters".to_string(),
        error: None,
    }
    .into_response()
}

pub async fn delete_filter_handler(
    locale: Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Path(filter_id): Path<Uuid>,
) -> impl IntoResponse {
    let _ = delete_filter(&state.db, filter_id, user.user_id).await;
    state.filter_engine.invalidate(&user.user_id);

    let labels = get_labels_by_user(&state.db, user.user_id)
        .await
        .unwrap_or_default();
    let filters = get_filters_by_user(&state.db, user.user_id)
        .await
        .unwrap_or_default();

    LabelsModalTemplate {
        locale,
        labels,
        filters,
        active_tab: "filters".to_string(),
        error: None,
    }
}

pub async fn add_email_label_handler(
    locale: Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Path(email_id): Path<Uuid>,
    Form(form): Form<AddEmailLabelForm>,
) -> impl IntoResponse {
    let _ = assign_labels_to_email(&state.db, email_id, &[form.label_id]).await;

    let labels = get_labels_for_email(&state.db, email_id)
        .await
        .unwrap_or_default();
    let all_user_labels = get_labels_by_user(&state.db, user.user_id)
        .await
        .unwrap_or_default();

    EmailLabelsPartialTemplate {
        locale,
        email_id,
        labels,
        all_user_labels,
    }
}

pub async fn remove_email_label_handler(
    locale: Locale,
    user: AuthenticatedUser,
    State(state): State<Arc<AppState>>,
    Path((email_id, label_id)): Path<(Uuid, Uuid)>,
) -> impl IntoResponse {
    let _ = remove_label_from_email(&state.db, email_id, label_id).await;

    let labels = get_labels_for_email(&state.db, email_id)
        .await
        .unwrap_or_default();
    let all_user_labels = get_labels_by_user(&state.db, user.user_id)
        .await
        .unwrap_or_default();

    EmailLabelsPartialTemplate {
        locale,
        email_id,
        labels,
        all_user_labels,
    }
}
