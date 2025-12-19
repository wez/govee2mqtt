# Govee2MQTT v2 (Python) Add-on

Engineering notes for the optional Home Assistant add-on wrapper.

## Purpose

- Package the Python implementation for HAOS/Supervised installs.
- Map add-on options to environment variables consumed by `govee2mqtt-v2`.

## Options Mapping

- `govee_api_key` -> `GOVEE_API_KEY`
- `mqtt_host` -> `MQTT_HOST`
- `mqtt_port` -> `MQTT_PORT`
- `mqtt_username` -> `MQTT_USERNAME`
- `mqtt_password` -> `MQTT_PASSWORD`
- `mqtt_base_topic` -> `MQTT_BASE_TOPIC`
- `poll_interval_seconds` -> `POLL_INTERVAL_SECONDS`
- `log_level` -> `LOG_LEVEL`

## Runtime

- `addon-v2/run.sh` reads config via `bashio` and execs `govee2mqtt-v2`.
- The container builds from the Python `Dockerfile` contents for parity with standalone usage.
