# Contributing to peat-gateway

Thank you for your interest in contributing to peat-gateway. This document covers development setup, testing, and the pull request process.

## Getting Started

1. Fork the repository and clone your fork
2. Create a feature branch from `main`
3. Make your changes
4. Run pre-commit checks
5. Submit a pull request

## Development Setup

### Prerequisites

- Rust stable toolchain (install via [rustup](https://rustup.rs))
- Node.js and pnpm (for the admin UI)
- Optional runtime dependencies for feature-gated sinks:
  - **NATS**: A running NATS server for CDC sink testing
  - **Kafka**: A Kafka broker (rdkafka requires cmake)
  - **PostgreSQL**: A Postgres instance for storage backend testing
  - **Vault**: HashiCorp Vault for KMS integration testing

### Feature Flags

peat-gateway uses feature-gated sinks to keep the binary lean. Each CDC sink, identity provider, and KMS backend is behind its own feature flag so deployments only compile what they need:

| Feature | Description |
|---------|-------------|
| `nats` | NATS CDC event sink (default) |
| `kafka` | Kafka CDC event sink |
| `webhook` | HTTP webhook CDC event sink (default) |
| `oidc` | OpenID Connect identity federation (default) |
| `postgres` | PostgreSQL storage backend |
| `aws-kms` | AWS KMS envelope encryption |
| `vault` | HashiCorp Vault envelope encryption |
| `full` | All features above combined |
| `loadtest` | Load testing utilities |

Default features: `nats`, `webhook`, `oidc`.

### Building

```bash
# Rust — default features
cargo build

# Rust — all features
cargo build --features full

# Rust — specific sink combination
cargo build --features nats,kafka,postgres

# Admin UI
cd ui
pnpm install
pnpm build
```

The admin UI is a SvelteKit application served as static assets by the gateway. Run `pnpm dev` inside `ui/` for development with hot reload.

## Testing

```bash
# Unit and integration tests with all features
cargo test --all-features

# Tests for a specific sink
cargo test --features nats --test nats_sink_tests
cargo test --features webhook --test webhook_sink_tests
cargo test --features postgres --test postgres_tests

# Identity and KMS tests
cargo test --features oidc --test oidc_mock_tests
cargo test --features aws-kms --test kms_tests
cargo test --features vault --test vault_tests

# Load tests
cargo test --features loadtest --test loadtest_tests

# Admin UI lint
cd ui
pnpm lint
```

Integration tests live in the `tests/` directory and cover API behavior, CDC sinks, error recovery, identity federation, key management, storage isolation, and load scenarios.

## Pre-Commit Checks

Before submitting a PR, ensure all of the following pass locally:

```bash
cargo fmt --check
cargo clippy --all-features -- -D warnings
cargo test --all-features
cd ui && pnpm lint
```

The CI pipeline runs these same checks on every PR.

## Feature-Gated Sinks Pattern

When adding a new CDC sink or integration backend, follow the existing pattern:

1. Add the dependency as optional in `Cargo.toml`
2. Create a feature flag that enables `dep:your-crate`
3. Gate all related modules and code behind `#[cfg(feature = "your-feature")]`
4. Add the feature to the `full` feature list
5. Write integration tests in `tests/` gated on the same feature

This keeps compile times fast and binary size small for deployments that only need a subset of sinks.

## Branching Strategy

We use **trunk-based development** on `main` with short-lived feature branches:

- Branch from `main` for all changes
- Keep branches small and focused (prefer multiple small PRs over one large one)
- Squash-and-merge to `main`

## Commit Requirements

- **GPG-signed commits are required.** Configure commit signing per [GitHub's documentation](https://docs.github.com/en/authentication/managing-commit-signature-verification).
- Write clear, descriptive commit messages

## Pull Request Access

Submitting pull requests requires contributor access to the repository. If you're interested in contributing, please open an issue to introduce yourself and discuss the change you'd like to make. A maintainer will grant PR access to active contributors.

## Pull Request Process

1. Open a PR against `main` with a clear description of the change
2. Focus each PR on a single concern
3. Ensure CI passes (fmt, clippy, tests across all features)
4. PRs require at least one approving review from a CODEOWNERS member
5. PRs are squash-merged to maintain a clean history

## Architectural Changes

For significant architectural changes, open an issue first to discuss the approach. Reference the relevant ADR (Architecture Decision Record) if one exists, or propose a new one.

## Reporting Issues

Use GitHub Issues to report bugs or request features. Include steps to reproduce, expected vs. actual behavior, and relevant log output.

## Code of Conduct

All contributors are expected to follow our [Code of Conduct](CODE_OF_CONDUCT.md).

## License

By contributing, you agree that your contributions will be licensed under the [Apache License 2.0](LICENSE).
