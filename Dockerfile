# ---- Build stage ------------------------------------------------------------
# Compile a fully static binary against musl so the runtime image needs no libc.
FROM rust:1.82-alpine AS builder

# musl-dev provides the musl libc headers needed to link statically.
RUN apk add --no-cache musl-dev

# Build explicitly for the musl target so the output is a static binary.
ENV TARGET=x86_64-unknown-linux-musl
RUN rustup target add ${TARGET}

WORKDIR /app

# Cache dependencies: copy manifests first and build a dummy target so the
# dependency layer is reused when only source changes.
COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --target ${TARGET} \
    && rm -rf src

# Now copy the real sources (main.rs + embedded templates) and build.
COPY src ./src
COPY templates ./templates
# Touch main.rs so cargo rebuilds it (the dummy build cached the deps).
RUN touch src/main.rs && cargo build --release --target ${TARGET}

# Move the binary to a fixed path so the runtime stage copy is unambiguous.
RUN cp target/${TARGET}/release/railway-derp /railway-derp

# ---- Runtime stage ----------------------------------------------------------
# scratch = empty image. The static musl binary has no dependencies, and the
# HTML is baked into the binary via include_str!, so nothing else is needed.
FROM scratch AS runtime

# Copy just the compiled static binary.
COPY --from=builder /railway-derp /railway-derp

# Railway injects PORT; the app reads it at runtime.
EXPOSE 3000

ENTRYPOINT ["/railway-derp"]
