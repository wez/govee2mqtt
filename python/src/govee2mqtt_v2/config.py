from __future__ import annotations

import os
from dataclasses import dataclass

DEFAULT_API_BASE_URL = "https://openapi.api.govee.com/router/api/v1"


@dataclass(frozen=True)
class Config:
    govee_api_key: str
    mqtt_host: str | None
    mqtt_port: int
    mqtt_username: str | None
    mqtt_password: str | None
    mqtt_base_topic: str
    poll_interval_seconds: int
    log_level: str
    api_base_url: str


def _get_env(name: str, default: str | None = None) -> str | None:
    value = os.getenv(name)
    if value is None or value == "":
        return default
    return value


def load_config(*, dry_run: bool) -> Config:
    api_key = _get_env("GOVEE_API_KEY")
    if not api_key:
        raise ValueError("GOVEE_API_KEY is required")

    mqtt_host = _get_env("MQTT_HOST")
    if not mqtt_host and not dry_run:
        raise ValueError("MQTT_HOST is required unless --dry-run is set")

    mqtt_port = int(_get_env("MQTT_PORT", "1883"))
    mqtt_username = _get_env("MQTT_USERNAME")
    mqtt_password = _get_env("MQTT_PASSWORD")
    mqtt_base_topic = _get_env("MQTT_BASE_TOPIC", "govee2mqtt")
    poll_interval_seconds = int(_get_env("POLL_INTERVAL_SECONDS", "120"))
    log_level = _get_env("LOG_LEVEL", "info")
    api_base_url = _get_env("GOVEE_API_BASE_URL", DEFAULT_API_BASE_URL)

    return Config(
        govee_api_key=api_key,
        mqtt_host=mqtt_host,
        mqtt_port=mqtt_port,
        mqtt_username=mqtt_username,
        mqtt_password=mqtt_password,
        mqtt_base_topic=mqtt_base_topic,
        poll_interval_seconds=poll_interval_seconds,
        log_level=log_level,
        api_base_url=api_base_url,
    )
