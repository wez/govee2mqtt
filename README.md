# Govee2MQTT (Engineering-Focused)

This repo is transitioning from a Rust-first bridge to a Python implementation that targets the official Govee Platform API v2. The goal is to improve community contribution velocity while keeping a clear, capability-driven architecture for Home Assistant MQTT Discovery.

## Objective

- Migrate core integration work to Python for broader contributor adoption.
- Use only the official Govee Platform API v2 (no undocumented IoT or LAN paths).
- Keep MQTT Discovery and device mapping capability-driven, not model-driven.

## Repository Layout

- `python/` : Python implementation (`govee2mqtt_v2`) with MQTT discovery and polling.
- `addon-v2/` : Optional Home Assistant add-on scaffolding for the Python app.
- `src/` : Legacy Rust implementation (retained, no refactors planned).
- `test-data/` : JSON fixtures used by tests and for capability examples.

## Engineering Docs

- `python/README.md` : Developer setup, architecture notes, testing, Docker.
- `addon-v2/README.md` : Add-on options mapping and runtime notes.
- `docs/ENGINEERING.md` : Design principles, rate limiting strategy, discovery rules.

## Build and Test (Python)

```bash
cd python
poetry install
poetry run pytest
poetry run pre-commit run --all-files
```

## Notes

- Secrets are never logged; configuration is env-var only.
- Rate limits are enforced with exponential backoff on HTTP 429.
- Unknown capabilities are ignored with debug logging.
