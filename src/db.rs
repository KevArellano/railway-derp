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

/// A registered application (Keycloak calls these "clients"). Shown as a
/// clickable thumbnail on the dashboard.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Client {
    pub id: Uuid,
    pub name: String,
    pub url: String,
}

/// Default role names seeded at startup.
pub const ROLE_ADMIN: &str = "admin";
pub const ROLE_VIEWER: &str = "viewer";

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

    // --- RBAC schema ---
    // Standard RBAC shape:
    //   users >-- user_roles --< roles >-- role_permissions --< permissions
    // We enforce *permissions* (resource:action), never role names, in code.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS roles (
            id   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            name TEXT NOT NULL UNIQUE
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS permissions (
            id   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            name TEXT NOT NULL UNIQUE
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS role_permissions (
            role_id       UUID NOT NULL REFERENCES roles(id)       ON DELETE CASCADE,
            permission_id UUID NOT NULL REFERENCES permissions(id) ON DELETE CASCADE,
            PRIMARY KEY (role_id, permission_id)
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS user_roles (
            user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            role_id UUID NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
            PRIMARY KEY (user_id, role_id)
        )
        "#,
    )
    .execute(pool)
    .await?;

    // --- Clients (the "apps" shown on the dashboard) ---
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS clients (
            id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            name       TEXT NOT NULL,
            url        TEXT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT now()
        )
        "#,
    )
    .execute(pool)
    .await?;

    seed_rbac(pool).await?;

    Ok(())
}

/// Seed the default roles, permissions, and their mappings. Idempotent:
/// `ON CONFLICT DO NOTHING` means it is safe to run on every startup.
///
/// - `admin`  -> client:read, client:create, client:delete
/// - `viewer` -> client:read
async fn seed_rbac(pool: &PgPool) -> anyhow::Result<()> {
    for role in [ROLE_ADMIN, ROLE_VIEWER] {
        sqlx::query("INSERT INTO roles (name) VALUES ($1) ON CONFLICT (name) DO NOTHING")
            .bind(role)
            .execute(pool)
            .await?;
    }

    for perm in [PERM_CLIENT_READ, PERM_CLIENT_CREATE, PERM_CLIENT_DELETE] {
        sqlx::query("INSERT INTO permissions (name) VALUES ($1) ON CONFLICT (name) DO NOTHING")
            .bind(perm)
            .execute(pool)
            .await?;
    }

    // Map role -> permission by name, resolving ids via subqueries.
    let mappings: &[(&str, &str)] = &[
        (ROLE_ADMIN, PERM_CLIENT_READ),
        (ROLE_ADMIN, PERM_CLIENT_CREATE),
        (ROLE_ADMIN, PERM_CLIENT_DELETE),
        (ROLE_VIEWER, PERM_CLIENT_READ),
    ];
    for (role, perm) in mappings {
        sqlx::query(
            r#"
            INSERT INTO role_permissions (role_id, permission_id)
            SELECT r.id, p.id FROM roles r, permissions p
            WHERE r.name = $1 AND p.name = $2
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(role)
        .bind(perm)
        .execute(pool)
        .await?;
    }

    Ok(())
}

/// Permission names (resource:action). These are the atomic units enforced in
/// code — handlers check permissions, never role names.
pub const PERM_CLIENT_READ: &str = "client:read";
pub const PERM_CLIENT_CREATE: &str = "client:create";
pub const PERM_CLIENT_DELETE: &str = "client:delete";

// --- RBAC queries -----------------------------------------------------------

/// Assign a role (by name) to a user. Idempotent.
pub async fn assign_role(pool: &PgPool, user_id: Uuid, role_name: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO user_roles (user_id, role_id)
        SELECT $1, r.id FROM roles r WHERE r.name = $2
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(user_id)
    .bind(role_name)
    .execute(pool)
    .await?;
    Ok(())
}

/// Resolve the flattened set of permission names a user has, via their roles.
/// Called once at login and cached in the session.
pub async fn permissions_for_user(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT DISTINCT p.name
        FROM user_roles ur
        JOIN role_permissions rp ON rp.role_id = ur.role_id
        JOIN permissions p       ON p.id = rp.permission_id
        WHERE ur.user_id = $1
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(name,)| name).collect())
}

// --- Client queries ---------------------------------------------------------

/// List all clients, newest first. The dashboard is a shared catalog: any user
/// with `client:read` sees the same clients.
pub async fn list_clients(pool: &PgPool) -> Result<Vec<Client>, sqlx::Error> {
    sqlx::query_as::<_, Client>(
        "SELECT id, name, url FROM clients ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await
}

/// Create a client. Requires `client:create` (enforced at the handler).
pub async fn create_client(pool: &PgPool, name: &str, url: &str) -> Result<Client, sqlx::Error> {
    sqlx::query_as::<_, Client>(
        "INSERT INTO clients (name, url) VALUES ($1, $2) RETURNING id, name, url",
    )
    .bind(name)
    .bind(url)
    .fetch_one(pool)
    .await
}

/// Delete a client by id. Requires `client:delete` (enforced at the handler).
pub async fn delete_client(pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM clients WHERE id = $1")
        .bind(id)
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
