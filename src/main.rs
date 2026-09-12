use std::net::SocketAddr;

use axum::{
    extract::State,
    http::StatusCode,
    response::Html,
    routing::{get, post},
    Router,
};
use sqlx::PgPool;
use tower_sessions::Session;
use uuid::Uuid;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tower_sessions::{cookie::SameSite, Expiry, SessionManagerLayer};
use tower_sessions_sqlx_store::PostgresStore;

mod auth;
mod db;

/// Shared application state handed to every handler.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
}

/// Landing page HTML, embedded at compile time.
const INDEX_HTML: &str = include_str!("../templates/index.html");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Structured logging. Set RUST_LOG to control verbosity (defaults to info).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    // Railway provides DATABASE_URL when a Postgres service is attached.
    let database_url = std::env::var("DATABASE_URL")
        .map_err(|_| anyhow::anyhow!("DATABASE_URL is not set"))?;

    // Connect and provision the schema at runtime (no compile-time DB needed).
    let pool = db::connect(&database_url).await?;
    db::init_schema(&pool).await?;

    // Session store, backed by Postgres. `migrate()` creates its table on
    // startup — same "dynamic" approach as the app schema.
    let session_store = PostgresStore::new(pool.clone());
    session_store.migrate().await?;

    // Cookies are Secure (HTTPS-only) by default, which is correct on Railway.
    // Set COOKIE_SECURE=false for local development over plain HTTP, otherwise
    // the browser will refuse to store the session cookie.
    let cookie_secure = std::env::var("COOKIE_SECURE")
        .map(|v| v != "false" && v != "0")
        .unwrap_or(true);

    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(cookie_secure)
        .with_http_only(true)
        .with_same_site(SameSite::Lax)
        .with_expiry(Expiry::OnInactivity(time::Duration::days(7)));

    let state = AppState { pool };
    let app = build_router(state, session_layer);

    // Railway injects PORT. Fall back to 3000 for local development.
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;

    tracing::info!("listening on http://{addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

/// Build the application router.
fn build_router(
    state: AppState,
    session_layer: SessionManagerLayer<PostgresStore>,
) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        // Auth
        .route("/signup", get(auth::signup_form).post(auth::signup))
        .route("/login", get(auth::login_form).post(auth::login))
        .route("/logout", post(auth::logout))
        .route("/me", get(auth::me)) // protected: 401 unless logged in
        .layer(session_layer)
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Serve the landing page, rendering a header that reflects auth state.
///
/// When logged in, the top-right shows the user's email and a logout button;
/// otherwise it shows Log in / Sign up links. This is plain server-side
/// rendering — the header fragment is spliced into the page at `<!--HEADER-->`.
async fn index(State(state): State<AppState>, session: Session) -> Html<String> {
    let header = render_header(&state, &session).await;
    Html(INDEX_HTML.replacen("<!--HEADER-->", &header, 1))
}

/// Build the top-right header fragment based on the current session.
async fn render_header(state: &AppState, session: &Session) -> String {
    // Read the logged-in user id from the session, if any.
    let user_id: Option<Uuid> = session.get(auth::USER_ID_KEY).await.ok().flatten();

    // Resolve the email only when a session id is present. Any failure (DB
    // error, or a stale id whose user no longer exists) degrades gracefully to
    // the logged-out view rather than breaking the landing page.
    let email = match user_id {
        Some(id) => db::find_user_by_id(&state.pool, id)
            .await
            .ok()
            .flatten()
            .map(|u| u.email),
        None => None,
    };

    match email {
        Some(email) => format!(
            "<span class=\"email\">{}</span>\
             <form method=\"post\" action=\"/logout\"><button>Log out</button></form>",
            auth::html_escape(&email)
        ),
        None => {
            "<a href=\"/login\">Log in</a><a href=\"/signup\">Sign up</a>".to_string()
        }
    }
}

/// Health check endpoint — handy for Railway and uptime monitors.
async fn health() -> StatusCode {
    StatusCode::OK
}

/// Wait for a shutdown signal (Ctrl+C, or SIGTERM on Unix) so the server
/// can drain connections cleanly when Railway redeploys or scales down.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received, draining connections");
}
