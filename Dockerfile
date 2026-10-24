# A2A Warden: from-scratch image.
#
# Stage 1 builds the SPA and the binary, stage 2 produces a minimal runtime
# image with no shell and no package manager.

# --- Stage 1: frontend ------------------------------------------------------
FROM docker.io/library/node:22-alpine AS frontend

WORKDIR /src/frontend
COPY frontend/package.json frontend/package-lock.json* ./
RUN npm ci --no-audit --no-fund || npm install --no-audit --no-fund
COPY frontend/ ./
RUN npm run build

# --- Stage 2: binary --------------------------------------------------------
FROM docker.io/library/rust:1-alpine AS builder

RUN apk add --no-cache musl-dev openssl-dev pkgconfig sqlite-dev

WORKDIR /src
COPY Cargo.toml Cargo.lock* ./
COPY crates/ crates/
COPY tests/ tests/
COPY config/ config/
COPY --from=frontend /src/frontend/dist/ frontend/dist/

RUN cargo build --release --locked --bin a2a-warden

# --- Stage 3: runtime -------------------------------------------------------
FROM docker.io/library/debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --no-create-home warden

COPY --from=builder /src/target/release/a2a-warden /usr/local/bin/a2a-warden
COPY config/default.yaml /etc/a2a-warden/config.yaml

USER 10001
WORKDIR /var/lib/a2a-warden
VOLUME ["/var/lib/a2a-warden"]
EXPOSE 8080

ENV A2AWARDEN_DATABASE__SQLITE__PATH=/var/lib/a2a-warden/a2a-warden.db

ENTRYPOINT ["/usr/local/bin/a2a-warden"]
CMD ["--config", "/etc/a2a-warden/config.yaml"]
