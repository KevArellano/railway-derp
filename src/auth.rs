//! Authentication: signup, login, logout, and a protected route.
//!
//! Passwords are hashed with Argon2 (current recommended default). Sessions
//! are managed by `tower-sessions`: the cookie carries only an opaque session
//! id, while the session data (here, the user's id) lives server-side in
//! Postgres. All SQL goes through the runtime query layer in [`crate::db`].

use std::sync::LazyLock;

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect},
    Form,
};
use serde::Deserialize;
use tower_sessions::Session;
use uuid::Uuid;

use crate::{db, AppState};

/// Session key under which we store the logged-in user's id.
const USER_ID_KEY: &str = "user_id";

/// Login/signup form payload.
#[derive(Debug, Deserialize)]
pub struct Credentials {
    pub email: String,
    pub password: String,
}

const SIGNUP_HTML: &str = include_str!("../templates/signup.html");
const LOGIN_HTML: &str = include_str!("../templates/login.html");

// --- Form pages -------------------------------------------------------------

pub async fn signup_form() -> Html<&'static str> {
    Html(SIGNUP_HTML)
}

pub async fn login_form() -> Html<&'static str> {
    Html(LOGIN_HTML)
}

// --- Handlers ---------------------------------------------------------------

/// Create an account, then log the new user in and redirect home.
pub async fn signup(
    State(state): State<AppState>,
    session: Session,
    Form(creds): Form<Credentials>,
) -> Result<Redirect, AuthError> {
    let email = creds.email.trim().to_lowercase();
    if email.is_empty() || creds.password.len() < 8 {
        return Err(AuthError::BadInput(
            "Email is required and password must be at least 8 characters.",
        ));
    }

    let password_hash = hash_password(&creds.password)?;

    let user = match db::create_user(&state.pool, &email, &password_hash).await {
        Ok(user) => user,
        Err(e) if db::is_unique_violation(&e) => {
            return Err(AuthError::BadInput("That email is already registered."));
        }
        Err(e) => return Err(AuthError::from(e)),
    };

    start_session(&session, user.id).await?;
    Ok(Redirect::to("/"))
}

/// Verify credentials, then log in and redirect home.
pub async fn login(
    State(state): State<AppState>,
    session: Session,
    Form(creds): Form<Credentials>,
) -> Result<Redirect, AuthError> {
    let email = creds.email.trim().to_lowercase();

    let user = db::find_user_by_email(&state.pool, &email).await?;

    // Verify against the stored hash. When the user does not exist we still
    // run a verification against a dummy hash so the response time does not
    // reveal whether the email is registered (mitigates user enumeration).
    let verified = match &user {
        Some(u) => verify_password(&creds.password, &u.password_hash),
        None => {
            let _ = verify_password(&creds.password, &DUMMY_HASH);
            false
        }
    };

    match (user, verified) {
        (Some(u), true) => {
            start_session(&session, u.id).await?;
            Ok(Redirect::to("/"))
        }
        _ => Err(AuthError::InvalidCredentials),
    }
}

/// Destroy the current session and return to the landing page.
pub async fn logout(session: Session) -> Result<Redirect, AuthError> {
    session.delete().await.map_err(AuthError::session)?;
    Ok(Redirect::to("/"))
}

/// Protected route: shows the current user, or 401 if not logged in.
pub async fn me(
    State(state): State<AppState>,
    session: Session,
) -> Result<Html<String>, AuthError> {
    let user_id: Option<Uuid> = session.get(USER_ID_KEY).await.map_err(AuthError::session)?;

    let Some(user_id) = user_id else {
        return Err(AuthError::Unauthorized);
    };

    match db::find_user_by_id(&state.pool, user_id).await? {
        Some(user) => Ok(Html(format!(
            "<!doctype html><meta charset=utf-8><title>me</title>\
             <p>Logged in as <strong>{}</strong> (id {}).</p>\
             <form method=post action=/logout><button>Log out</button></form>",
            html_escape(&user.email),
            user.id
        ))),
        // Session referenced a user that no longer exists; treat as logged out.
        None => {
            let _ = session.delete().await;
            Err(AuthError::Unauthorized)
        }
    }
}

// --- Helpers ----------------------------------------------------------------

/// A valid Argon2 hash of an arbitrary password, computed once on first use.
/// Used to keep login timing constant when the email is not found, so response
/// time does not reveal whether an email is registered. Generated at runtime
/// (rather than a hardcoded literal) so it is guaranteed to be a parseable PHC
/// string that matches our Argon2 parameters.
static DUMMY_HASH: LazyLock<String> = LazyLock::new(|| {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(b"dummy-password-for-constant-time", &salt)
        .expect("failed to build dummy hash")
        .to_string()
});

/// Hash a plaintext password with Argon2id and a fresh random salt.
fn hash_password(password: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| AuthError::Internal("failed to hash password"))?;
    Ok(hash.to_string())
}

/// Verify a plaintext password against a stored PHC hash string.
fn verify_password(password: &str, stored_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Persist the logged-in user's id into the session. Cycling the id on login
/// prevents session fixation.
async fn start_session(session: &Session, user_id: Uuid) -> Result<(), AuthError> {
    session.cycle_id().await.map_err(AuthError::session)?;
    session
        .insert(USER_ID_KEY, user_id)
        .await
        .map_err(AuthError::session)?;
    Ok(())
}

/// Minimal HTML escaping for user-controlled text rendered into a page.
fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// --- Errors -----------------------------------------------------------------

/// Errors returned by auth handlers, mapped to sensible HTTP responses.
#[derive(Debug)]
pub enum AuthError {
    BadInput(&'static str),
    InvalidCredentials,
    Unauthorized,
    Internal(&'static str),
}

impl AuthError {
    fn session<E: std::fmt::Display>(e: E) -> Self {
        tracing::error!("session error: {e}");
        AuthError::Internal("session error")
    }
}

impl From<sqlx::Error> for AuthError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!("database error: {e}");
        AuthError::Internal("database error")
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> axum::response::Response {
        let (status, msg) = match self {
            AuthError::BadInput(m) => (StatusCode::BAD_REQUEST, m),
            AuthError::InvalidCredentials => {
                (StatusCode::UNAUTHORIZED, "Invalid email or password.")
            }
            AuthError::Unauthorized => (StatusCode::UNAUTHORIZED, "Not logged in."),
            AuthError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Html(format!("<p>{}</p>", html_escape(msg)))).into_response()
    }
}
