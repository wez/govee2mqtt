from __future__ import annotations

import logging
from typing import Any

from .models import Capability, Device, DeviceState

logger = logging.getLogger(__name__)

SENSOR_CAPABILITIES: dict[str, dict[str, str]] = {
    "temperature": {"device_class": "temperature", "unit": "C"},
    "humidity": {"device_class": "humidity", "unit": "%"},
    "co2": {"device_class": "carbon_dioxide", "unit": "ppm"},
    "pm2_5": {"device_class": "pm25", "unit": "ug/m3"},
    "pm10": {"device_class": "pm10", "unit": "ug/m3"},
    "voc": {"device_class": "volatile_organic_compounds", "unit": "ppb"},
    "aqi": {"device_class": "aqi", "unit": "AQI"},
}

BINARY_SENSOR_CAPABILITIES: dict[str, dict[str, str]] = {
    "motion": {"device_class": "motion"},
    "leak": {"device_class": "moisture"},
    "contact": {"device_class": "door"},
    "door": {"device_class": "door"},
}


def _capability_matches(cap: Capability, *, cap_type: str, instance: str | None = None) -> bool:
    if cap.type != cap_type:
        return False
    if instance is None:
        return True
    return cap.instance == instance


def _find_capability(
    capabilities: list[Capability], cap_type: str, instance: str | None = None
) -> Capability | None:
    for cap in capabilities:
        if _capability_matches(cap, cap_type=cap_type, instance=instance):
            return cap
    return None


def is_light(device: Device) -> bool:
    has_power = _find_capability(device.capabilities, "devices.capabilities.on_off") is not None
    has_brightness = (
        _find_capability(device.capabilities, "devices.capabilities.range", "brightness")
        is not None
    )
    has_color = (
        _find_capability(device.capabilities, "devices.capabilities.color_setting", "colorRgb")
        is not None
    )
    has_temp = (
        _find_capability(
            device.capabilities, "devices.capabilities.color_setting", "colorTemperatureK"
        )
        is not None
    )
    return has_power and (has_brightness or has_color or has_temp)


def is_switch(device: Device) -> bool:
    has_power = _find_capability(device.capabilities, "devices.capabilities.on_off") is not None
    if not has_power:
        return False
    return not is_light(device)


def sensor_entities(device: Device) -> list[dict[str, Any]]:
    entities: list[dict[str, Any]] = []
    for cap in device.capabilities:
        if cap.type not in ("devices.capabilities.range", "devices.capabilities.property"):
            continue
        instance = cap.instance
        if instance in SENSOR_CAPABILITIES:
            meta = SENSOR_CAPABILITIES[instance]
            entities.append(
                {
                    "instance": instance,
                    "device_class": meta["device_class"],
                    "unit": meta["unit"],
                }
            )
        elif instance in BINARY_SENSOR_CAPABILITIES:
            meta = BINARY_SENSOR_CAPABILITIES[instance]
            entities.append(
                {"instance": instance, "device_class": meta["device_class"], "binary": True}
            )
    return entities


def device_slug(device: Device) -> str:
    return device.device.replace(":", "").lower()


def light_state_from_device_state(state: DeviceState) -> dict[str, Any]:
    payload: dict[str, Any] = {}
    power = _find_capability(state.capabilities, "devices.capabilities.on_off", "powerSwitch")
    if power is not None:
        payload["state"] = "ON" if power.state_value else "OFF"

    brightness = _find_capability(state.capabilities, "devices.capabilities.range", "brightness")
    if brightness is not None and isinstance(brightness.state_value, int | float):
        payload["brightness"] = int(round((float(brightness.state_value) / 100.0) * 255.0))

    color = _find_capability(state.capabilities, "devices.capabilities.color_setting", "colorRgb")
    if color is not None and isinstance(color.state_value, dict):
        rgb = {
            "r": color.state_value.get("r"),
            "g": color.state_value.get("g"),
            "b": color.state_value.get("b"),
        }
        if all(isinstance(v, int) for v in rgb.values()):
            payload["color"] = rgb

    color_temp = _find_capability(
        state.capabilities, "devices.capabilities.color_setting", "colorTemperatureK"
    )
    if color_temp is not None and isinstance(color_temp.state_value, int | float):
        kelvin = float(color_temp.state_value)
        if kelvin > 0:
            payload["color_temp"] = int(round(1000000.0 / kelvin))

    return payload


def switch_state_from_device_state(state: DeviceState) -> str | None:
    power = _find_capability(state.capabilities, "devices.capabilities.on_off", "powerSwitch")
    if power is None:
        return None
    return "ON" if power.state_value else "OFF"


def sensor_state_from_device_state(state: DeviceState) -> dict[str, Any]:
    payload: dict[str, Any] = {}
    for cap in state.capabilities:
        if cap.type not in ("devices.capabilities.range", "devices.capabilities.property"):
            continue
        instance = cap.instance
        value = cap.state_value
        if instance in SENSOR_CAPABILITIES:
            payload[instance] = value
        elif instance in BINARY_SENSOR_CAPABILITIES:
            if value is None or value == "":
                continue
            payload[instance] = 1 if value else 0
        else:
            logger.debug("Ignoring unsupported sensor capability: %s", instance)
    return payload


def light_command_to_capabilities(payload: dict[str, Any]) -> list[tuple[str, str, Any]]:
    commands: list[tuple[str, str, Any]] = []
    if "state" in payload:
        value = 1 if str(payload["state"]).upper() == "ON" else 0
        commands.append(("devices.capabilities.on_off", "powerSwitch", value))
    if "brightness" in payload:
        brightness = payload["brightness"]
        if isinstance(brightness, int | float):
            scaled = max(1, min(100, int(round((float(brightness) / 255.0) * 100.0))))
            commands.append(("devices.capabilities.range", "brightness", scaled))
    if "color" in payload and isinstance(payload["color"], dict):
        rgb = payload["color"]
        commands.append(("devices.capabilities.color_setting", "colorRgb", rgb))
    if "color_temp" in payload:
        mired = payload["color_temp"]
        if isinstance(mired, int | float) and mired > 0:
            kelvin = int(round(1000000.0 / float(mired)))
            commands.append(("devices.capabilities.color_setting", "colorTemperatureK", kelvin))
    return commands


def switch_command_to_capabilities(payload: str) -> list[tuple[str, str, Any]]:
    value = 1 if payload.strip().upper() == "ON" else 0
    return [("devices.capabilities.on_off", "powerSwitch", value)]
