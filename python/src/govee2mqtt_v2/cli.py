from __future__ import annotations

import argparse
import json
import logging
import queue
import time
from typing import Any

from .api import GoveeApiClient
from .config import load_config
from .discovery import light_discovery_payload, sensor_discovery_payloads, switch_discovery_payload
from .hass import (
    device_slug,
    is_light,
    is_switch,
    light_command_to_capabilities,
    light_state_from_device_state,
    sensor_entities,
    sensor_state_from_device_state,
    switch_command_to_capabilities,
    switch_state_from_device_state,
)
from .mqtt_client import MqttClient

logger = logging.getLogger(__name__)


def _setup_logging(level: str) -> None:
    logging.basicConfig(
        level=getattr(logging, level.upper(), logging.INFO),
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )


def _print_dry_run(devices: list) -> None:
    for device in devices:
        slug = device_slug(device)
        print(f"{device.name} ({device.sku}) [{slug}]")
        if is_light(device):
            print("  - light")
        elif is_switch(device):
            print("  - switch")
        for entity in sensor_entities(device):
            kind = "binary_sensor" if entity.get("binary") else "sensor"
            print(f"  - {kind}: {entity['instance']}")


def _publish_discovery(mqtt: MqttClient, base_topic: str, devices: list) -> None:
    for device in devices:
        if is_light(device):
            topic, payload = light_discovery_payload(device, base_topic)
            mqtt.publish_discovery(topic, payload)
        elif is_switch(device):
            topic, payload = switch_discovery_payload(device, base_topic)
            mqtt.publish_discovery(topic, payload)

        for topic, payload in sensor_discovery_payloads(device, base_topic):
            mqtt.publish_discovery(topic, payload)


def _publish_state(mqtt: MqttClient, base_topic: str, device, state) -> None:
    slug = device_slug(device)
    if is_light(device):
        payload = light_state_from_device_state(state)
        if payload:
            mqtt.publish(f"{base_topic}/{slug}/light/state", payload, retain=True)
    elif is_switch(device):
        value = switch_state_from_device_state(state)
        if value is not None:
            mqtt.publish(f"{base_topic}/{slug}/switch/state", value, retain=True)

    sensor_payload = sensor_state_from_device_state(state)
    if sensor_payload:
        mqtt.publish(f"{base_topic}/{slug}/sensor/state", sensor_payload, retain=True)


def _handle_command(
    api: GoveeApiClient,
    mqtt: MqttClient,
    base_topic: str,
    device_map: dict[str, Any],
    command: tuple[str, str, str],
) -> None:
    device_id, entity, payload = command
    device = device_map.get(device_id)
    if not device:
        logger.warning("Received command for unknown device %s", device_id)
        return

    if entity == "light":
        try:
            data = json.loads(payload) if payload else {}
        except json.JSONDecodeError:
            logger.warning("Invalid JSON payload for light command: %s", payload)
            return
        for cap_type, instance, value in light_command_to_capabilities(data):
            api.control_device(device, capability_type=cap_type, instance=instance, value=value)
    elif entity == "switch":
        for cap_type, instance, value in switch_command_to_capabilities(payload):
            api.control_device(device, capability_type=cap_type, instance=instance, value=value)
    else:
        logger.debug("Unsupported command entity: %s", entity)
        return

    state = api.get_device_state(device)
    _publish_state(mqtt, base_topic, device, state)


def main() -> int:
    parser = argparse.ArgumentParser(description="Govee Platform API v2 to MQTT bridge")
    parser.add_argument("--dry-run", action="store_true", help="Print discovered devices and exit")
    parser.add_argument("--once", action="store_true", help="Poll once and exit")
    args = parser.parse_args()

    config = load_config(dry_run=args.dry_run)
    _setup_logging(config.log_level)

    api = GoveeApiClient(config.govee_api_key, base_url=config.api_base_url)
    try:
        devices = api.list_devices()
    except Exception:
        api.close()
        raise

    if args.dry_run:
        _print_dry_run(devices)
        api.close()
        return 0

    if not config.mqtt_host:
        raise ValueError("MQTT_HOST is required when not in --dry-run mode")

    mqtt = MqttClient(
        host=config.mqtt_host,
        port=config.mqtt_port,
        username=config.mqtt_username,
        password=config.mqtt_password,
        base_topic=config.mqtt_base_topic,
    )

    command_queue: queue.Queue[tuple[str, str, str]] = queue.Queue()

    def _enqueue_command(device_id: str, entity: str, payload: str) -> None:
        command_queue.put((device_id, entity, payload))

    mqtt.set_command_handler(_enqueue_command)
    mqtt.connect()

    device_map = {device_slug(device): device for device in devices}
    _publish_discovery(mqtt, config.mqtt_base_topic, devices)

    try:
        while True:
            cycle_start = time.monotonic()
            for device in devices:
                while not command_queue.empty():
                    _handle_command(
                        api, mqtt, config.mqtt_base_topic, device_map, command_queue.get()
                    )

                state = api.get_device_state(device)
                _publish_state(mqtt, config.mqtt_base_topic, device, state)

                time.sleep(max(1.0, config.poll_interval_seconds / max(1, len(devices))))

            if args.once:
                break

            elapsed = time.monotonic() - cycle_start
            sleep_time = max(0.0, config.poll_interval_seconds - elapsed)
            if sleep_time:
                time.sleep(sleep_time)
    except KeyboardInterrupt:
        logger.info("Shutting down")
    finally:
        mqtt.disconnect()
        api.close()

    return 0
