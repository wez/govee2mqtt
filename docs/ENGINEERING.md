# Engineering Notes

## Design Goals

- Python-first for contributor accessibility and faster iteration.
- Use only the official Govee Platform API v2 endpoints.
- Capability-driven mapping for HA entities; avoid model-specific branching.
- Keep MQTT discovery retained and stable across restarts.

## Architecture Overview

- `govee2mqtt_v2.api` wraps Platform API v2 endpoints with rate-limit handling.
- `govee2mqtt_v2.hass` maps capabilities to HA entities and state schemas.
- `govee2mqtt_v2.discovery` builds MQTT Discovery payloads.
- `govee2mqtt_v2.cli` orchestrates polling, command handling, and publishing.

## Rate Limiting

- Default polling is 120 seconds.
- Requests are staggered across devices to avoid bursts.
- HTTP 429 triggers exponential backoff with a max delay of 60 seconds.
- Prefer increasing `POLL_INTERVAL_SECONDS` if rate limits occur.

## Discovery Rules (MVP)

- Light: on_off + (brightness or color or color temperature)
- Switch: on_off without light capabilities
- Sensor: range/property instances (temperature/humidity/co2/pm2_5/pm10/voc/aqi)
- Binary sensor: motion/leak/contact/door when exposed by capability

## State Publishing

- Light state payload: `state`, `brightness`, `color`, `color_temp`
- Switch state payload: `ON` or `OFF`
- Sensor state payload: JSON with multiple sensor values per device

## Command Handling

- Light commands accept HA JSON schema and translate to Platform API capability calls.
- Switch commands accept `ON`/`OFF` and map to `powerSwitch`.
- Post-command state is refreshed and published.

## Testing

- JSON fixtures live in `test-data/`.
- Unit tests focus on parsing and discovery payload construction.
- Run locally:

```bash
cd python
poetry install
poetry run pytest
poetry run pre-commit run --all-files
```
