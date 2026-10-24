# Contributing to A2A Warden

Thank you for your interest in contributing to A2A Warden! This document outlines the guidelines and processes for contributing to this project.

## Table of Contents

1. [Code of Conduct](#code-of-conduct)
2. [Getting Started](#getting-started)
3. [Development Workflow](#development-workflow)
4. [Commit Messages](#commit-messages)
5. [Pull Request Process](#pull-request-process)
6. [Testing](#testing)
7. [Documentation](#documentation)
8. [Security](#security)

## Code of Conduct

This project adheres to the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md). By participating, you are expected to uphold this code. Please report unacceptable behavior to security@a2a-warden.io.

## Getting Started

### Prerequisites

- Rust (managed via `rust-toolchain.toml`)
- Node.js 20+ (for frontend development)
- SQLite (for local development)
- PostgreSQL (for integration testing)
- Docker (for containerized development)

### Setup

```bash
# Clone the repository
git clone https://github.com/a2a-warden/a2a-warden.git
cd a2a-warden

# Install Rust toolchain
rustup override set $(cat rust-toolchain.toml | grep channel | cut -d'"' -f2)

# Build the project
cargo build

# Run tests
cargo test

# Run with default configuration
cargo run --bin a2a-warden
```

### Frontend Development

```bash
cd frontend
npm install
npm run dev  # Development server with hot reload
```

## Development Workflow

### Branch Strategy

We use **trunk-based development** with short-lived feature branches:

- `main` - Protected branch, always deployable
- Feature branches - `feature/<short-description>`
- Bug fix branches - `fix/<short-description>`
- Documentation branches - `docs/<short-description>`

### Creating Changes

1. Fork the repository
2. Create a feature branch from `main`
3. Make your changes
4. Ensure all tests pass
5. Submit a pull request

### Quality Gates

All changes must pass:

- `cargo fmt --check` - Code formatting
- `cargo clippy -D warnings` - Linting
- `cargo test` - All tests pass
- `cargo audit` - No known vulnerabilities
- Frontend build succeeds

## Commit Messages

We follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>[optional scope]: <description>

[optional body]

[optional footer(s)]
```

### Types

- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation only changes
- `style`: Formatting, missing semicolons, etc.
- `refactor`: Code change that neither fixes a bug nor adds a feature
- `perf`: Performance improvement
- `test`: Adding missing tests or correcting existing tests
- `chore`: Maintenance tasks, dependency updates, etc.
- `ci`: CI/CD changes

### Examples

```
feat(server): add SendMessage endpoint implementation
fix(store): handle SQLite migration race condition
docs: update configuration reference for PostgreSQL
refactor(core): extract TaskStore trait to separate module
test(e2e): add approval workflow test case
ci: add PostgreSQL matrix to GitHub Actions
```

## Pull Request Process

1. **Title**: Use conventional commit format
2. **Description**: Explain the motivation and context
3. **Testing**: Describe how you tested the changes
4. **Documentation**: Update relevant documentation
5. **Review**: Address reviewer comments
6. **Merge**: Squash and merge after approval

### PR Template

All PRs should include:

- What changed and why
- How it was tested
- Any breaking changes
- Related issues
- Screenshots (for UI changes)

## Testing

### Test Categories

- **Unit Tests**: Per-crate, fast, isolated (`cargo test`)
- **Integration Tests**: Cross-crate, test endpoints (`cargo test --test integration`)
- **E2E Tests**: Full workflow with external agents (`cargo test --test e2e`)

### Running Tests

```bash
# All tests
cargo test

# With SQLite (default)
cargo test --features sqlite

# With PostgreSQL (requires running instance)
cargo test --features postgres

# Specific test
cargo test -p a2a-warden-store test_task_persistence

# Frontend tests
cd frontend && npm test
```

### Test Requirements

- Every new feature must have tests
- Bug fixes must include a regression test
- Integration tests for all API endpoints
- E2E tests for critical user flows

## Documentation

### Requirements

All code, comments, and documentation must be in **English**.

### Mandatory Documentation

- README - Project overview and quick start
- Architecture - System design and decisions
- Configuration - Complete configuration reference
- API Reference - Administration API documentation
- Security - Data privacy and security considerations

### Updating Documentation

- Update docs in the same PR as code changes
- Keep examples runnable and tested
- Document breaking changes prominently

## Security

### Reporting Vulnerabilities

Please report security vulnerabilities privately to **security@a2a-warden.io**.

Do not create public issues for security vulnerabilities.

### Security Practices

- All dependencies audited via `cargo audit`
- Deny-by-default policy decisions
- No sensitive data in logs
- Regular dependency updates via Dependabot

## Release Process

Releases are automated via `cargo-dist`:

1. Version bump in `Cargo.toml`
2. Tag push triggers release workflow
3. Multi-platform binaries built and signed
4. Docker image published to ghcr.io
5. GitHub Release created with artifacts

## Questions?

- Open a [discussion](https://github.com/a2a-warden/a2a-warden/discussions)
- Check existing [issues](https://github.com/a2a-warden/a2a-warden/issues)
- Read the [architecture docs](docs/architecture.md)

---

*This document is a living document and will evolve with the project.*