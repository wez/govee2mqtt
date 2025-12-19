from __future__ import annotations

from typing import Any

from .hass import device_slug, sensor_entities
from .models import Device

DISCOVERY_PREFIX = "homeassistant"


def _device_info(device: Device) -> dict[str, Any]:
    return {
        "identifiers": [device.device],
        "name": device.name,
        "manufacturer": "Govee",
        "model": device.sku,
    }


def light_discovery_payload(device: Device, base_topic: str) -> tuple[str, dict[str, Any]]:
    slug = device_slug(device)
    object_id = f"{slug}_light"
    topic = f"{DISCOVERY_PREFIX}/light/{object_id}/config"
    state_topic = f"{base_topic}/{slug}/light/state"
    command_topic = f"{base_topic}/{slug}/light/set"
    payload: dict[str, Any] = {
        "name": device.name,
        "unique_id": f"govee2mqtt_v2_{slug}_light",
        "schema": "json",
        "command_topic": command_topic,
        "state_topic": state_topic,
        "brightness": True,
        "rgb": True,
        "color_temp": True,
        "device": _device_info(device),
    }
    return topic, payload


def switch_discovery_payload(device: Device, base_topic: str) -> tuple[str, dict[str, Any]]:
    slug = device_slug(device)
    object_id = f"{slug}_switch"
    topic = f"{DISCOVERY_PREFIX}/switch/{object_id}/config"
    state_topic = f"{base_topic}/{slug}/switch/state"
    command_topic = f"{base_topic}/{slug}/switch/set"
    payload: dict[str, Any] = {
        "name": device.name,
        "unique_id": f"govee2mqtt_v2_{slug}_switch",
        "state_topic": state_topic,
        "command_topic": command_topic,
        "device": _device_info(device),
    }
    return topic, payload


def sensor_discovery_payloads(device: Device, base_topic: str) -> list[tuple[str, dict[str, Any]]]:
    slug = device_slug(device)
    base_state_topic = f"{base_topic}/{slug}/sensor/state"
    payloads: list[tuple[str, dict[str, Any]]] = []
    for entity in sensor_entities(device):
        instance = entity["instance"]
        object_id = f"{slug}_{instance}"
        if entity.get("binary"):
            topic = f"{DISCOVERY_PREFIX}/binary_sensor/{object_id}/config"
            payload = {
                "name": f"{device.name} {instance}",
                "unique_id": f"govee2mqtt_v2_{slug}_{instance}",
                "state_topic": base_state_topic,
                "value_template": f"{{{{ value_json.{instance} }}}}",
                "device_class": entity["device_class"],
                "device": _device_info(device),
            }
        else:
            topic = f"{DISCOVERY_PREFIX}/sensor/{object_id}/config"
            payload = {
                "name": f"{device.name} {instance}",
                "unique_id": f"govee2mqtt_v2_{slug}_{instance}",
                "state_topic": base_state_topic,
                "value_template": f"{{{{ value_json.{instance} }}}}",
                "device_class": entity["device_class"],
                "unit_of_measurement": entity["unit"],
                "device": _device_info(device),
            }
        payloads.append((topic, payload))
    return payloads
