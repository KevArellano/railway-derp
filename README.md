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

Railway builds from the `Dockerfile` (multi-stage static musl → `scratch`).

1. Push this repo to GitHub.
2. In Railway, create a new project → **Deploy from GitHub repo** → select this repo.
3. Add a **Postgres** service to the project (New → Database → PostgreSQL).
4. On the app service, set the `DATABASE_URL` variable to the reference `${{Postgres.DATABASE_URL}}` so it resolves over Railway's private network.
5. Railway injects `PORT` automatically; the server reads it on startup.
6. Under the app service **Settings → Networking**, generate a domain for a public URL.

Every push to the connected branch triggers a new build and deploy.

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
