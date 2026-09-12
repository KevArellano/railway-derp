//! Database layer.
//!
//! All SQL lives here and uses the sqlx **runtime** query API (`sqlx::query`,
//! `query_as`) rather than the compile-time-checked macros. That means there
//! is no database dependency at build time: the Docker image compiles without
//! a reachable Postgres, and the connection only happens at runtime once
//! Railway has injected `DATABASE_URL`.

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use uuid::Uuid;

/// A stored user. `FromRow` maps result columns by name at runtime — it is a
/// derive, not a DB-touching macro, so it needs no schema at compile time.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub password_hash: String,
}

/// Build a connection pool from `DATABASE_URL`.
///
/// Connections are kept low (`max_connections = 5`) to stay light on
/// constrained compute; bump this if you scale up.
pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;
    Ok(pool)
}

/// Create the schema if it does not already exist.
///
/// Runs on every startup and is idempotent (`IF NOT EXISTS`), so pointing the
/// app at a fresh, empty Postgres provisions it automatically. The session
/// table is handled separately by `tower-sessions-sqlx-store`'s `migrate()`.
pub async fn init_schema(pool: &PgPool) -> anyhow::Result<()> {
    // pgcrypto provides gen_random_uuid() on older Postgres; modern Postgres
    // has it built in, but the extension is harmless and keeps us portable.
    sqlx::query("CREATE EXTENSION IF NOT EXISTS pgcrypto")
        .execute(pool)
        .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            email         TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
        )
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Insert a new user. Returns the created row.
///
/// A duplicate email surfaces as a Postgres unique-violation (SQLSTATE 23505);
/// callers can distinguish that via [`is_unique_violation`].
pub async fn create_user(
    pool: &PgPool,
    email: &str,
    password_hash: &str,
) -> Result<User, sqlx::Error> {
    sqlx::query_as::<_, User>(
        "INSERT INTO users (email, password_hash) \
         VALUES ($1, $2) \
         RETURNING id, email, password_hash",
    )
    .bind(email)
    .bind(password_hash)
    .fetch_one(pool)
    .await
}

/// Look up a user by email. Returns `None` if no such user exists.
pub async fn find_user_by_email(
    pool: &PgPool,
    email: &str,
) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(
        "SELECT id, email, password_hash FROM users WHERE email = $1",
    )
    .bind(email)
    .fetch_optional(pool)
    .await
}

/// Look up a user by id. Used to rehydrate the logged-in user from a session.
pub async fn find_user_by_id(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as::<_, User>(
        "SELECT id, email, password_hash FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

/// True if the error is a Postgres unique-constraint violation (SQLSTATE 23505),
/// e.g. signing up with an email that already exists.
pub fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(
        err,
        sqlx::Error::Database(db) if db.code().as_deref() == Some("23505")
    )
}
