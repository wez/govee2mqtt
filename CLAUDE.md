# govee2mqtt

Rust project that bridges Govee smart home devices to MQTT / Home Assistant.

## Build & Test

```bash
cargo build --all
cargo test --all -- --show-output
cargo fmt --all -- --check
```

## Project Structure

- `src/` — Rust source code
- `addon/` — Home Assistant add-on configuration
- `scripts/` — Build and release scripts
- `docs/` — Documentation
- `test-data/` — Test fixtures

## CI

PRs must pass `cargo build`, `cargo test`, and `cargo fmt --check` (see `.github/workflows/pr.yml`).

The fork also runs Claude Code CI (`.github/workflows/claude.yml`).
