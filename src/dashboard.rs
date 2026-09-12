//! Dashboard: a catalog of clients (apps) shown as clickable thumbnails.
//!
//! - `GET  /dashboard`                  requires `client:read`
//! - `POST /dashboard/apps`             requires `client:create`
//! - `POST /dashboard/apps/{id}/delete` requires `client:delete`
//!
//! Add/delete controls are only rendered when the user holds the relevant
//! permission, AND the POST routes are guarded server-side by the `Require`
//! extractor — UI hiding is UX, the extractor is the security boundary.

use axum::{
    extract::{Path, State},
    response::{Html, IntoResponse, Redirect, Response},
    Form,
};
use serde::Deserialize;
use tower_sessions::Session;
use uuid::Uuid;

use crate::authz::{self, AuthzError, Require};
use crate::{auth::html_escape, db, require_permission, AppState};

// Permission marker types used by the `Require<...>` extractor.
require_permission!(CanRead, db::PERM_CLIENT_READ);
require_permission!(CanCreate, db::PERM_CLIENT_CREATE);
require_permission!(CanDelete, db::PERM_CLIENT_DELETE);

const DASHBOARD_HTML: &str = include_str!("../templates/dashboard.html");

/// Form payload for adding a client.
#[derive(Debug, Deserialize)]
pub struct NewClient {
    pub name: String,
    pub url: String,
}

/// GET /dashboard — list clients as thumbnails. Requires `client:read`.
pub async fn dashboard(
    _guard: Require<CanRead>,
    State(state): State<AppState>,
    session: Session,
) -> Result<Html<String>, AuthzError> {
    let clients = db::list_clients(&state.pool)
        .await
        .map_err(|_| AuthzError::Internal)?;

    let can_create = authz::has_permission(&session, db::PERM_CLIENT_CREATE).await;
    let can_delete = authz::has_permission(&session, db::PERM_CLIENT_DELETE).await;

    let page = DASHBOARD_HTML
        .replacen("<!--THUMBNAILS-->", &render_thumbnails(&clients, can_delete), 1)
        .replacen("<!--ADDFORM-->", &render_add_form(can_create), 1);

    Ok(Html(page))
}

/// POST /dashboard/apps — create a client. Requires `client:create`.
pub async fn add_client(
    _guard: Require<CanCreate>,
    State(state): State<AppState>,
    Form(input): Form<NewClient>,
) -> Response {
    let name = input.name.trim();
    let url = input.url.trim();

    if name.is_empty() || !is_valid_http_url(url) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Html("<p>Name is required and URL must be a valid http(s) link.</p>".to_string()),
        )
            .into_response();
    }

    match db::create_client(&state.pool, name, url).await {
        Ok(_) => Redirect::to("/dashboard").into_response(),
        Err(e) => {
            tracing::error!("create_client failed: {e}");
            AuthzError::Internal.into_response()
        }
    }
}

/// POST /dashboard/apps/{id}/delete — delete a client. Requires `client:delete`.
pub async fn delete_client(
    _guard: Require<CanDelete>,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Response {
    match db::delete_client(&state.pool, id).await {
        Ok(()) => Redirect::to("/dashboard").into_response(),
        Err(e) => {
            tracing::error!("delete_client failed: {e}");
            AuthzError::Internal.into_response()
        }
    }
}

// --- Rendering --------------------------------------------------------------

/// Render the client thumbnails. Each is a card linking to the client URL in a
/// new tab (with `rel="noopener noreferrer"`). A colored tile shows the first
/// letter of the name — minimal, no image storage.
fn render_thumbnails(clients: &[db::Client], can_delete: bool) -> String {
    if clients.is_empty() {
        return "<p class=\"empty\">No apps yet.</p>".to_string();
    }

    clients
        .iter()
        .map(|c| {
            let initial = c
                .name
                .chars()
                .next()
                .map(|ch| ch.to_uppercase().to_string())
                .unwrap_or_else(|| "?".to_string());
            let color = color_for(&c.name);
            let name = html_escape(&c.name);
            // href is validated http(s) at insert time; escape it for the attribute.
            let url = html_escape(&c.url);

            let delete_btn = if can_delete {
                format!(
                    "<form class=\"del\" method=\"post\" action=\"/dashboard/apps/{}/delete\">\
                       <button title=\"Delete\" aria-label=\"Delete {}\">×</button>\
                     </form>",
                    c.id, name
                )
            } else {
                String::new()
            };

            format!(
                "<div class=\"card\">\
                   {delete}\
                   <a class=\"thumb\" href=\"{url}\" target=\"_blank\" rel=\"noopener noreferrer\">\
                     <span class=\"tile\" style=\"background:{color}\">{initial}</span>\
                     <span class=\"name\">{name}</span>\
                   </a>\
                 </div>",
                delete = delete_btn,
                url = url,
                color = color,
                initial = html_escape(&initial),
                name = name,
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render the "add app" form, only when the user can create.
fn render_add_form(can_create: bool) -> String {
    if !can_create {
        return String::new();
    }
    "<form class=\"add\" method=\"post\" action=\"/dashboard/apps\">\
       <input name=\"name\" placeholder=\"App name\" required />\
       <input name=\"url\" type=\"url\" placeholder=\"https://example.com\" required />\
       <button type=\"submit\">Add app</button>\
     </form>"
        .to_string()
}

/// Only permit http/https URLs. This blocks `javascript:` and other schemes
/// that would be dangerous when rendered into an href.
fn is_valid_http_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://")) && url.len() <= 2048
}

/// Deterministic pleasant-ish color from a name, for the thumbnail tile.
fn color_for(name: &str) -> String {
    // Simple FNV-ish hash -> hue. Fixed saturation/lightness for consistency.
    let mut hash: u32 = 2166136261;
    for b in name.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(16777619);
    }
    let hue = hash % 360;
    format!("hsl({hue}, 55%, 45%)")
}
