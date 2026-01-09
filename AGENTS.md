# Repository Guidelines

## Project Structure & Modules
- `capp/`: core public API (manager, prelude) re-exporting config/queue/router/backoff under feature flags.
- `capp-queue/`: task queue traits and backends (in-memory, Fjall, MongoDB, PostgreSQL); DB schema files live in `migrations/`.
- `capp-config/`: TOML-driven configuration, HTTP/proxy/backoff helpers.
- `capp-router/`: URL routing/classification utilities; `capp-cache/`: optional cache helpers; `capp-urls/`: URL parsing helpers.
- `examples/`: runnable samples (`cargo run --example basic`, `--example urls`); `tests/`: integration tests with shared harness in `tests/common/`.

## Build, Test, and Development Commands
- `cargo fmt --all`: format using repo rustfmt (edition 2024, max width 84).
- `cargo clippy --workspace --all-targets --all-features -D warnings`: lint all crates and examples.
- `cargo test --workspace --all-features`: run unit + integration tests (tokio runtimes are created inside tests).
- `cargo doc --workspace --no-deps --open`: build API docs locally.
- `just tags`: refresh ctags; `just lines`: quick LOC summary.

## Coding Style & Naming Conventions
- Rust 2024 edition; prefer explicit imports and `tracing` macros for logging.
- Snake_case for modules/files, PascalCase for types/traits, SCREAMING_SNAKE_CASE for constants.
- Avoid `unwrap`/`expect` in library code; bubble errors with `anyhow::Result` or typed errors via `thiserror`.
- Keep feature-gated code tidy (`http`, `router`, `cache`, backend features) and maintain re-exports in `capp/src/lib.rs`.

## Testing Guidelines
- Integration tests live in `tests/` (manager flow, healthcheck, HTTP harness); use fixtures in `tests/common/` and configs under `tests/*.toml`.
- Prefer deterministic async tests with `#[tokio::test]` or explicit runtimes; avoid external network calls.
- For backend changes that need services (Mongo/Postgres), guard with feature flags and document required setup.
- Aim to keep coverage stable (Codecov badge enforced in CI); add tests alongside new behaviors.

## Commit & Pull Request Guidelines
- Follow existing short imperative commit style (`update queue backend`, `make clippy happy`); keep changes scoped.
- Before a PR: run fmt + clippy + tests with relevant features, and update README/examples/config snippets when behavior changes.
- PR description should state what/why, test commands run, feature flags toggled, and any migration impacts (`migrations/`).
- Add screenshots only if output/UX changes; link issues when applicable.

## Security & Configuration Tips
- Do not commit real connection strings or secrets; keep example hosts local.
- Prefer YAML-driven settings via `capp-config`; document defaults when introducing new keys.
- When adding external calls in examples/tests, provide a mocked path or feature guard to keep CI predictable.
- File handling safety: never use `rm` commands; move files into `tmp/` instead (create the directory first if needed).
