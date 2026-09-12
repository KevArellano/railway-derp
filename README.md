# railway-derp

A minimal, low-footprint web service built with **Rust + [Axum](https://github.com/tokio-rs/axum)**, ready to deploy on **[Railway](https://railway.com)**. Structured so backend auth (login/signup) drops in cleanly later.

## Why this stack

- **Tiny memory footprint** — idles at ~10 MB RAM, ideal for constrained compute.
- **Single static binary** — fast cold starts, small deploy image, no runtime to ship.
- **Size-optimized release build** — `opt-level = "z"`, LTO, stripped symbols, `panic = "abort"`.
- **Room to grow** — server-rendered HTML via the standard library, ready for sessions and password hashing without heavy frameworks.

## Project layout

```
railway-derp/
├── src/
│   └── main.rs         # Axum server, routes, graceful shutdown
├── templates/
│   └── index.html      # landing page (embedded at compile time)
├── Cargo.toml          # dependencies + release profile
├── railway.json        # Railway build/deploy config
├── .env.example        # local env var template
└── .gitignore
```

## Run locally

Requires a [Rust toolchain](https://rustup.rs) (1.80+).

```bash
cargo run
```

Then open http://localhost:3000. The port defaults to `3000` and can be overridden with the `PORT` env var.

Routes:
- `GET /` — landing page
- `GET /health` — health check (returns `200 OK`)

## Deploy to Railway

Railway auto-detects Rust via `Cargo.toml` and builds it with Nixpacks — no Dockerfile needed.

1. Push this repo to GitHub.
2. In Railway, create a new project → **Deploy from GitHub repo** → select this repo.
3. Railway builds and deploys automatically. It injects a `PORT` env var, which the server reads on startup.
4. Under the service **Settings → Networking**, generate a domain to get a public URL.

Every push to the connected branch triggers a new build and deploy.

### Configuration

| Variable   | Purpose                                | Default              |
|------------|----------------------------------------|----------------------|
| `PORT`     | Port the server binds to               | `3000` (set by Railway in prod) |
| `RUST_LOG` | Log verbosity                          | `info,tower_http=info` |

## Adding auth later

The router in `src/main.rs` (`build_router`) is the single place to register new routes. When you add login/signup:

- Add `GET`/`POST` `/login` and `/signup` handlers.
- Use the [`argon2`](https://crates.io/crates/argon2) crate for password hashing (current recommended default).
- Use [`tower-sessions`](https://crates.io/crates/tower-sessions) for cookie-based sessions.
- Add a Postgres service in Railway and read its `DATABASE_URL` env var (keep DB traffic on Railway's private network).

Never commit secrets — session keys and database URLs belong in Railway's environment variables, not the repo.
