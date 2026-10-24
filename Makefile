# A2A Warden build helpers.
#
# The frontend is not part of the Cargo workspace: its build is invoked from
# this makefile, and its output (frontend/dist) is embedded in the binary with
# rust-embed.

FRONTEND_DIR := frontend
DIST_DIR := $(FRONTEND_DIR)/dist
BINARY := target/release/a2a-warden

.PHONY: all frontend build build-release test test-postgres lint fmt check clean docker run

all: build

## Build the SPA into frontend/dist.
frontend:
	cd $(FRONTEND_DIR) && npm ci --no-audit --no-fund
	cd $(FRONTEND_DIR) && npm run build

## Build the binary in debug mode.
build: frontend
	cargo build --bin a2a-warden

## Build the optimized binary.
build-release: frontend
	cargo build --release --bin a2a-warden

## Run the whole test suite (SQLite).
test:
	cargo test --workspace

## Run the whole test suite against PostgreSQL.
## Usage: make test-postgres DATABASE_URL=postgresql://user:pass@host:5432/db
test-postgres:
	cargo test --workspace

## Format and lint.
fmt:
	cargo fmt --all

lint:
	cargo clippy --workspace --all-targets -- -D warnings

## The quality gates of CI.
check: fmt lint test

## Build the container image.
docker:
	podman build -t a2a-warden:latest .

## Run the warden with the sample configuration.
run: build-release
	$(BINARY) --config examples/config/sqlite.yaml

clean:
	cargo clean
	rm -rf $(DIST_DIR)/assets
