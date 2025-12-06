% Config Migration Plan (YAML → TOML)

## Goal

Switch configuration format from YAML to TOML across the workspace and bump crate version to 0.6 (breaking change accepted).

## Proposed Steps
2) Implement TOML parsing
   - Replace YAML dependencies with `toml`/`toml_edit` + serde integrations.
  ^^^ I added already, we'll use "toml"
   - Update config loader API to read `.toml` files; keep error messages clear for misformatted TOML.
   - Adjust schema structs if TOML typing requires tweaks (e.g., arrays vs tables).
3) Update assets and docs
   - Convert sample configs to `.toml` (tests fixtures, examples, README, AGENTS.md references).
   - Rename paths/extensions and update any hardcoded `.yml` references.
4) Tests + tooling
   - Update integration tests to load TOML fixtures; ensure tokio runtimes still create configs correctly.
   - Run `cargo fmt`, `cargo clippy --workspace --all-targets --all-features`, `cargo test --workspace --all-features`.
5) Versioning + release notes
   - Bump workspace/package version to `0.6.0` in `Cargo.toml` and references.
   - Note breaking change (config format switch) in changelog/README.

## Files to Update for TOML Migration
- `capp-config/src/config.rs`: core loader uses `serde_yaml::Value`, YAML error type, and tests reference `tests/simple_config.yml`.
- `capp-config/src/http.rs`: YAML-focused docs/examples and parsing tests built on `serde_yaml::Value`.
- `capp-config/src/proxy.rs`: YAML parsing helpers and inline YAML fixtures for proxy lists.
- `capp/src/lib.rs`: re-exports `serde_yaml`; remove/swap to TOML value type.
- `capp/src/manager/worker.rs`: worker config fields use `serde_yaml::Value`.
- `capp/Cargo.toml`, `capp-config/Cargo.toml`: `serde_yaml` dependency and ignore list entry for `.tmuxp.yaml`.
- Examples: `examples/basic.rs`, `examples/complex.rs.not_working`, `examples/hackernews/main.rs` all load `tests/simple_config.yml` and rely on `serde_yaml::Value`.
- Tests: `tests/manager_tests.rs` uses `serde_yaml::Value` and loads `tests/simple_config.yml`; fixture `tests/simple_config.yml` must become TOML.
- Docs: `README.md` configuration snippet in YAML; `AGENTS.md` mentions `tests/*.yml` and should be updated to TOML wording.
