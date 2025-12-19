# Home Assistant Add-on (Python v2)

This doc describes the optional add-on wrapper for the Python implementation.

## Purpose

- Package `govee2mqtt-v2` for HAOS/Supervised installs.
- Use Platform API v2 only.

## Files

- `addon-v2/config.yaml` : Add-on metadata and options schema.
- `addon-v2/Dockerfile` : Add-on container build.
- `addon-v2/run.sh` : Option-to-env mapping and process entrypoint.

## Options Mapping

| Add-on Option | Env Var |
| --- | --- |
| govee_api_key | GOVEE_API_KEY |
| mqtt_host | MQTT_HOST |
| mqtt_port | MQTT_PORT |
| mqtt_username | MQTT_USERNAME |
| mqtt_password | MQTT_PASSWORD |
| mqtt_base_topic | MQTT_BASE_TOPIC |
| poll_interval_seconds | POLL_INTERVAL_SECONDS |
| log_level | LOG_LEVEL |

## Engineering Notes

- The add-on is a thin wrapper; feature work belongs in `python/`.
- Keep secrets in add-on options and never log them.
