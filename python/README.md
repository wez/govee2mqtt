# Govee2MQTT v2 (Python)

Engineering-focused documentation for the Python implementation that uses the official Govee Platform API v2 and publishes Home Assistant MQTT Discovery entities.

## Objectives

- Python-first for broader community contribution.
- Platform API v2 only (no undocumented IoT or LAN).
- Capability-driven entity mapping and stable MQTT discovery topics.

## Layout

- `src/govee2mqtt_v2/` : Core implementation.
- `tests/` : Unit tests.
- `Dockerfile` : Container build for HAOS and standalone use.

## Configuration (Env Vars)

- GOVEE_API_KEY (required)
- MQTT_HOST (required unless --dry-run)
- MQTT_PORT (default: 1883)
- MQTT_USERNAME (optional)
- MQTT_PASSWORD (optional)
- MQTT_BASE_TOPIC (default: govee2mqtt)
- POLL_INTERVAL_SECONDS (default: 120)
- LOG_LEVEL (default: info)
- GOVEE_API_BASE_URL (default: https://openapi.api.govee.com/router/api/v1)

## Local Development

```bash
cd python
poetry install
poetry run govee2mqtt-v2 --dry-run
```

## Testing and Auto-Fix

```bash
cd python
poetry run pytest
poetry run pre-commit run --all-files
```

## Docker Build

```bash
docker build -f python/Dockerfile -t govee2mqtt-v2 .
docker run --rm --env-file .env --network host govee2mqtt-v2
```

## Home Assistant Discovery

- Discovery topics are retained under `homeassistant/...`.
- State/command topics are under the configured base topic (default `govee2mqtt`).
- Light schema uses JSON payloads compatible with HA MQTT Light.

## Limitations (MVP)

- Polling only (no event subscription).
- Capability mapping is best-effort; unknown capabilities are ignored.

## Design Notes

See `docs/ENGINEERING.md` for mapping rules, rate limiting strategy, and module-level responsibilities.
