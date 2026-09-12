use std::net::SocketAddr;

use axum::{
    http::StatusCode,
    response::Html,
    routing::get,
    Router,
};
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;

/// Landing page HTML, rendered at compile time from the templates directory.
const INDEX_HTML: &str = include_str!("../templates/index.html");

#[tokio::main]
async fn main() {
    // Structured logging. Set RUST_LOG to control verbosity (defaults to info).
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower_http=info".into()),
        )
        .init();

    let app = build_router();

    // Railway injects PORT. Fall back to 3000 for local development.
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);

    // Bind to 0.0.0.0 so the container is reachable from outside.
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr)
        .await
        .expect("failed to bind to address");

    tracing::info!("listening on http://{addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");
}

/// Build the application router.
///
/// New routes (e.g. `/login`, `/signup`) get added here as the app grows.
fn build_router() -> Router {
    Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .layer(TraceLayer::new_for_http())
}

/// Serve the landing page.
async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
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
