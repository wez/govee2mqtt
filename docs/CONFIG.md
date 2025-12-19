# Configuration (Python v2)

Configuration is via environment variables only. Do not commit secrets.

## Required

- GOVEE_API_KEY
- MQTT_HOST (required unless running `--dry-run`)

## Optional

- MQTT_PORT (default 1883)
- MQTT_USERNAME
- MQTT_PASSWORD
- MQTT_BASE_TOPIC (default govee2mqtt)
- POLL_INTERVAL_SECONDS (default 120)
- LOG_LEVEL (default info)
- GOVEE_API_BASE_URL (default https://openapi.api.govee.com/router/api/v1)

## Engineering Notes

- `GOVEE_API_KEY` is never logged.
- All MQTT discovery topics are retained for stable HA entity registration.
- Rate limiting uses exponential backoff on HTTP 429.
