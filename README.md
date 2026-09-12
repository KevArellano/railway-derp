# railway-derp

A minimal, low-footprint web service built with **Rust + [Axum](https://github.com/tokio-rs/axum)**, deployed on **[Railway](https://railway.com)**. Includes backend-handled auth (signup / login / logout) with Postgres.

## Why this stack

- **Tiny memory footprint** — idles at ~10 MB RAM, ideal for constrained compute.
- **Single static binary** — a static musl build shipped on a `scratch` image (~10-15 MB), fast cold starts.
- **Size-optimized release build** — `opt-level = "z"`, LTO, stripped symbols, `panic = "abort"`.
- **No compile-time database dependency** — all SQL uses sqlx's runtime query API, so the Docker image builds without a reachable database and the schema self-provisions at startup.

## Project layout

```
railway-derp/
├── src/
│   ├── main.rs         # server, router, DB + session wiring, graceful shutdown
│   ├── db.rs           # pool, startup schema init, runtime user queries
│   └── auth.rs         # signup/login/logout/me handlers, argon2 hashing
├── templates/
│   ├── index.html      # landing page
│   ├── login.html      # login form
│   └── signup.html     # signup form
├── Cargo.toml          # dependencies + size-optimized release profile
├── Dockerfile          # multi-stage: static musl build → scratch runtime
├── .dockerignore
├── railway.json        # Railway build/deploy config (Dockerfile builder)
├── rust-toolchain.toml # pinned Rust toolchain
├── .env.example        # local env var template
└── .gitignore
```

## Routes

| Method | Path       | Purpose                                    |
|--------|------------|--------------------------------------------|
| GET    | `/`        | landing page                               |
| GET    | `/health`  | health check (`200 OK`)                    |
| GET    | `/signup`  | signup form                                |
| POST   | `/signup`  | create account, start session, redirect    |
| GET    | `/login`   | login form                                 |
| POST   | `/login`   | verify credentials, start session, redirect |
| POST   | `/logout`  | destroy session                            |
| GET    | `/me`      | protected — shows current user or `401`    |

## Run locally

Requires a [Rust toolchain](https://rustup.rs) (the pinned version is in `rust-toolchain.toml`) and a Postgres instance.

Start a local Postgres (Docker):

```bash
docker run --rm -e POSTGRES_PASSWORD=dev -p 5432:5432 postgres:17
```

Then run the app (copy `.env.example` to `.env` first, or export the vars):

```bash
DATABASE_URL=postgres://postgres:dev@localhost:5432/postgres COOKIE_SECURE=false cargo run
```

`COOKIE_SECURE=false` is needed for local HTTP so the browser stores the session cookie; in production (HTTPS on Railway) leave it at the default `true`.

Open http://localhost:3000. The server creates its tables on first startup.

## How auth works

- **Passwords** are hashed with **Argon2id** ([`argon2`](https://crates.io/crates/argon2)) using a fresh random salt. Only the PHC hash string is stored.
- **Sessions** use [`tower-sessions`](https://crates.io/crates/tower-sessions) with a Postgres-backed store: the cookie carries only an opaque session id (HttpOnly, Secure, SameSite=Lax); session data lives server-side.
- **Login timing** runs a hash verification even when the email is unknown, to avoid leaking which emails are registered.
- **Session fixation** is mitigated by cycling the session id on login.

### No compile-time DB dependency (by design)

All queries use `sqlx::query` / `query_as` (runtime API), **not** the `sqlx::query!` macros. Consequences:

- The Docker build needs no database and no `.sqlx` offline cache.
- The schema is created at startup via `CREATE TABLE IF NOT EXISTS`, so pointing the app at any empty Postgres provisions it automatically.
- Tradeoff: SQL is validated at runtime, not compile time. All SQL is centralized in `src/db.rs` to keep it reviewable.

## Deploy to Railway

This repo deploys **two services** from a single GitHub repo, each with its own Dockerfile:

| Service   | Config              | Dockerfile           | Purpose                    |
|-----------|---------------------|----------------------|----------------------------|
| app       | `railway.json`      | `Dockerfile`         | Rust app (static musl → scratch) |
| postgres  | `postgres/railway.json` | `postgres/Dockerfile` | Self-managed Postgres 18-alpine |

### 1. Postgres service (self-managed)

1. Push this repo to GitHub.
2. In Railway, create the project and add a service → **Deploy from GitHub repo** → select this repo.
3. In that service's **Settings**, set the **config path** to `postgres/railway.json` (so it builds `postgres/Dockerfile`).
4. Add a **Volume** to the service, mounted at `/var/lib/postgresql`. This is required — without it the database is wiped on every redeploy. (Postgres 18+ manages a version-specific data subdirectory inside this mount; do not mount at `/var/lib/postgresql/data`, which is the old ≤17 layout.)
5. Set variables on the Postgres service:
   - `POSTGRES_PASSWORD` — a strong password
   - `POSTGRES_USER` — e.g. `postgres`
   - `POSTGRES_DB` — e.g. `railway_derp`

> **Tradeoff:** this is a self-managed database — no automatic backups or managed upgrades. Railway's managed Postgres plugin provides those; this Dockerfile approach trades them for full in-repo version control.

### 2. App service

1. Add a second service from the same repo; leave its config path as the root `railway.json` (builds the root `Dockerfile`).
2. Set the `DATABASE_URL` variable to reference the Postgres service over the private network, e.g.:
   ```
   DATABASE_URL=postgresql://${{postgres.POSTGRES_USER}}:${{postgres.POSTGRES_PASSWORD}}@${{postgres.RAILWAY_PRIVATE_DOMAIN}}:5432/${{postgres.POSTGRES_DB}}
   ```
   (Use the actual name of your Postgres service in place of `postgres`.)
3. Railway injects `PORT` automatically; the server reads it on startup and creates its tables on first boot.
4. Under the app service **Settings → Networking**, generate a domain for a public URL.

Every push to the connected branch triggers a new build and deploy for both services.

### Local development

For a one-command local stack (app + Postgres 18-alpine), use the included `docker-compose.yml`:

```bash
docker compose up --build
```

This is for local dev only — Railway uses the two-service setup above, not compose.

## Configuration

| Variable       | Purpose                              | Default                          |
|----------------|--------------------------------------|----------------------------------|
| `DATABASE_URL`  | Postgres connection string (required — app exits if unset) | — |
| `PORT`          | Port the server binds to             | `3000` (set by Railway in prod)  |
| `COOKIE_SECURE` | Session cookie `Secure` flag (HTTPS-only). Set `false` for local HTTP dev | `true` |
| `RUST_LOG`      | Log verbosity                        | `info,tower_http=info`           |

Never commit secrets — `DATABASE_URL` and any session keys belong in Railway's environment variables, not the repo.

## Notes on dependency pinning

`tower-sessions-sqlx-store` lags the main `tower-sessions` crate by one internal `tower-sessions-core` version. To keep a single `core` in the tree, `tower-sessions` is pinned to `=0.14.0` and the store to `=0.15.0` (both use `core 0.14`). Revisit these pins when the store crate catches up to `core 0.15`.
