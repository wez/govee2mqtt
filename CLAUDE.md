# govee2mqtt

Rust project that bridges Govee smart home devices to MQTT / Home Assistant.


## Parallelization

**Maximize concurrent work at all times.** The user runs Claude Max 20 (up to 20 parallel agents). Every agent must actively look for opportunities to parallelize:

- Launch multiple sub-agents in a single message whenever tasks are independent. Aim for the maximum useful concurrency (up to 20).
- Make independent tool calls (Read, Grep, Glob, Bash, etc.) in parallel — never sequentially when they don't depend on each other.
- When planning work, explicitly identify which steps can run concurrently and structure execution to saturate available capacity.
- Prefer splitting large tasks into independent sub-tasks over doing them one-by-one.

The goal: run the subscription hard. Sequential work is wasted capacity.

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
