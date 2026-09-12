//! Authorization (RBAC enforcement).
//!
//! Design choices (the "how a senior does it" version):
//! - We enforce **permissions** (`resource:action`), never role names, so
//!   adding/retuning roles is a data change, not a code change.
//! - Permissions are resolved **once at login** and cached in the session, so
//!   request-time checks hit an in-memory set, not the database.
//!   Trade-off: a role revoked mid-session is not reflected until the session
//!   expires or the user logs in again. Acceptable here given the 7-day
//!   inactivity expiry; a "permissions version" bump could force refresh later.
//! - **Deny by default**: missing permission -> 403, enforced server-side
//!   regardless of what the UI shows.

use std::collections::HashSet;

use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{Html, IntoResponse, Response},
};
use tower_sessions::Session;

use crate::db;

/// Session key holding the flattened set of the user's permission names.
pub const PERMISSIONS_KEY: &str = "permissions";

/// Resolve a user's permissions and store them in the session. Call this at
/// login/signup, after the user id is established.
pub async fn cache_permissions(
    session: &Session,
    pool: &sqlx::PgPool,
    user_id: uuid::Uuid,
) -> anyhow::Result<()> {
    let perms = db::permissions_for_user(pool, user_id).await?;
    session
        .insert(PERMISSIONS_KEY, perms)
        .await
        .map_err(|e| anyhow::anyhow!("failed to cache permissions: {e}"))?;
    Ok(())
}

/// Read the cached permission set from the session (empty if none/not logged in).
pub async fn permissions(session: &Session) -> HashSet<String> {
    session
        .get::<Vec<String>>(PERMISSIONS_KEY)
        .await
        .ok()
        .flatten()
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

/// True if the session's cached permissions include `perm`.
pub async fn has_permission(session: &Session, perm: &str) -> bool {
    permissions(session).await.contains(perm)
}

/// Error type for authorization failures, mapped to HTTP responses.
pub enum AuthzError {
    /// Not logged in — no session / no permissions cached.
    Unauthenticated,
    /// Logged in but lacking the required permission.
    Forbidden,
    /// Session backend error.
    Internal,
}

impl IntoResponse for AuthzError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            AuthzError::Unauthenticated => (StatusCode::UNAUTHORIZED, "Please log in."),
            AuthzError::Forbidden => {
                (StatusCode::FORBIDDEN, "You do not have permission to do that.")
            }
            AuthzError::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "Authorization error."),
        };
        (status, Html(format!("<p>{msg}</p>"))).into_response()
    }
}

/// Extractor that requires a specific permission before a handler runs.
///
/// Usage: declare a typed wrapper via the [`require_permission!`] macro and add
/// it as a handler argument. If the permission is absent, extraction fails with
/// 403 (or 401 if not logged in) and the handler body never executes.
///
/// This is a generic-free design: each required permission is a distinct
/// zero-sized type carrying its permission string as an associated constant.
pub trait Permission {
    const NAME: &'static str;
}

/// A guard that succeeds only if the session holds `P::NAME`.
pub struct Require<P: Permission>(pub std::marker::PhantomData<P>);

impl<S, P> FromRequestParts<S> for Require<P>
where
    S: Send + Sync,
    P: Permission,
{
    type Rejection = AuthzError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // Pull the Session that tower-sessions inserted into request extensions.
        let session = Session::from_request_parts(parts, state)
            .await
            .map_err(|_| AuthzError::Internal)?;

        let perms = permissions(&session).await;
        if perms.is_empty() {
            return Err(AuthzError::Unauthenticated);
        }
        if perms.contains(P::NAME) {
            Ok(Require(std::marker::PhantomData))
        } else {
            Err(AuthzError::Forbidden)
        }
    }
}

/// Define a zero-sized permission marker type implementing [`Permission`].
#[macro_export]
macro_rules! require_permission {
    ($ty:ident, $name:expr) => {
        pub struct $ty;
        impl $crate::authz::Permission for $ty {
            const NAME: &'static str = $name;
        }
    };
}
