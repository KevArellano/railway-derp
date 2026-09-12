# TODOs / Roadmap

Deferred features to implement later. Captured from design discussions.

## Email validation

Current signup only checks non-empty email + password length ≥ 8. Harden it:

- **Level 1 — format validation** (recommended first): add the [`email_address`](https://crates.io/crates/email_address) crate and validate in `signup` (and optionally `login`). Keep existing trim + lowercase.
  ```rust
  use email_address::EmailAddress;
  let email = creds.email.trim().to_lowercase();
  if email.len() > 254 || !EmailAddress::is_valid(&email) {
      return Err(AuthError::BadInput("Please enter a valid email address."));
  }
  ```
  - Alternative: the [`validator`](https://crates.io/crates/validator) crate for declarative rules on the form struct (`#[validate(email)]`) if we expect more fields/rules.
  - Avoid hand-rolled email regexes.
- **Level 2 — normalization:** cap length at 254 (RFC max); already lowercase + trim.
- **Level 3 — deliverability / ownership:** email confirmation flow (unverified user + one-time token + verification link). Requires an email-sending integration. See "Magic links" below — shares infrastructure.

## Multi-factor and passwordless auth

All of these turn login into a multi-step, stateful flow and want: encrypted secrets at rest + hashed recovery codes. Build that scaffolding once.

### 1. TOTP 2FA (authenticator apps) — recommended first
- Second factor on top of existing password login.
- Crate: [`totp-rs`](https://crates.io/crates/totp-rs) (secret generation, QR, verification).
- Schema: `users.totp_secret` (nullable, encrypt at rest), `users.totp_enabled`; plus hashed one-time **recovery codes**.
- Flow: password → if TOTP enabled, enter-code page → verify → start session.
- No external service, no client JS. Best security-per-effort.

### 2. Passwordless — magic links
- Email a one-time signed login link; click starts a session.
- Requires an **email-sending integration** (SMTP / Postmark / SES) — main cost.
- Schema: `login_tokens` (token hash, user, expiry, used flag).
- Security tied to the email account + short token expiry. Makes email part of the critical login path.

### 3. Passkeys / WebAuthn (covers YubiKey, Touch ID, Face ID, Windows Hello) — strongest
- Phishing-resistant public-key auth; private key never leaves the device/security key.
- **YubiKey is just a WebAuthn authenticator — implement WebAuthn, not YubiKey-specific code.**
- Crate: [`webauthn-rs`](https://crates.io/crates/webauthn-rs) (handles the crypto/ceremony correctly — do not hand-roll).
- Requires a **small amount of client JS** (`navigator.credentials.create/get`) — the only place we'd introduce JS. Breaks the current zero-JS property.
- Schema: `credentials` table (public key, credential id, signature counter; many-to-one with users). Store per-ceremony challenge state server-side.
- Highest effort of the three.

### Recommended sequencing
1. TOTP 2FA (no external deps, no JS, reuses password flow; forces building recovery codes + multi-step login).
2. Passkeys / WebAuthn (strongest; accept the small JS requirement).
3. Magic links (only if passwordless-by-email is a product goal; adds email infra to the login path).

### Open questions to resolve before implementing
- Personal account security (few users) vs. product for many users?
- Passwordless: replace passwords, or add alongside them?
- Is introducing a small amount of client JS acceptable (required for WebAuthn)?


## Architecture / tech debt (flagged during design review)

### Database migrations
- Current approach: `CREATE TABLE IF NOT EXISTS` at startup (deliberate "no compile-time DB" choice). Fine for now, but tech debt as the schema evolves (RBAC adds several tables).
- Move to versioned migrations soon: ordered `.sql` files applied at startup, or adopt `sqlx migrate`. Keeps schema changes reviewable and repeatable.

### API boundary (REST / GraphQL) — NOT needed yet
- The server renders its own HTML and talks to Postgres in-process. No external/uncontrolled consumers → no API layer needed now. Adding one now = over-engineering.
- Add an API contract boundary only when there are consumers we don't ship with: a separate SPA/mobile frontend, third-party integrators, or multiple internal services.
- When that day comes: **REST first** (data is resource-shaped: users, roles, clients). Reach for **GraphQL** only if consumers genuinely need flexible/varied queries (accepting its caching + query-complexity + N+1 costs).
- **Avoid** auto-generated API-on-tables (Hasura/PostgREST) for anything long-lived — it couples the public API to the physical schema and pushes authz into the DB. Keep the API contract decoupled from storage.

## Learning / craft notes (toward senior-level)
- Practice: state trade-offs of your own decisions explicitly (e.g. cached-perms vs staleness).
- Write a one-page design doc (problem / options / trade-offs / decision) before non-trivial work.
- Keep the modular monolith; split into services only under real pressure (independent scaling, team boundaries, deploy isolation).
- Add an **audit log** for authorization-relevant changes (who granted whom what) — painful to backfill.
- Reading: *Designing Data-Intensive Applications* (Kleppmann); Google Zanzibar paper; Raft paper; AWS Builder's Library.
