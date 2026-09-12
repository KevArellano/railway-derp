# Postgres service for Railway.
#
# Uses the Alpine variant of the latest major (18) for the smallest footprint
# (~110 MB vs ~400+ MB for the Debian-based default).
#
# IMPORTANT: This is a self-managed database. Persistence depends on a Railway
# volume mounted at /var/lib/postgresql (see postgres/railway.json and the
# README). Without that volume, data is lost on every redeploy. There are no
# automatic backups — that is the tradeoff versus Railway's managed Postgres.
#
# Postgres 18+ expects a SINGLE mount at /var/lib/postgresql and manages a
# version-specific data subdirectory itself (e.g. .../18/docker). Do NOT
# override PGDATA or mount at /var/lib/postgresql/data — that is the old
# (<=17) layout and breaks the pg_upgrade path on 18+.
FROM postgres:18-alpine

# Postgres listens on 5432. Railway wires this over the private network; the
# app reaches it by the service's private hostname, not a public port.
EXPOSE 5432
