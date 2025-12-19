from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass
class Capability:
    type: str
    instance: str
    parameters: dict[str, Any] = field(default_factory=dict)
    state: dict[str, Any] = field(default_factory=dict)

    @property
    def state_value(self) -> Any:
        return self.state.get("value")


@dataclass
class Device:
    sku: str
    device: str
    name: str
    device_type: str | None
    capabilities: list[Capability]


@dataclass
class DeviceState:
    sku: str
    device: str
    capabilities: list[Capability]


def _parse_capabilities(raw_caps: list[dict[str, Any]]) -> list[Capability]:
    capabilities: list[Capability] = []
    for item in raw_caps or []:
        capabilities.append(
            Capability(
                type=item.get("type", ""),
                instance=item.get("instance", ""),
                parameters=item.get("parameters") or {},
                state=item.get("state") or {},
            )
        )
    return capabilities


def parse_device_list(payload: dict[str, Any]) -> list[Device]:
    data = payload.get("data")
    if data is None:
        data = payload.get("payload")
    if data is None:
        data = []

    devices: list[Device] = []
    for entry in data:
        name = entry.get("deviceName") or entry.get("name") or entry.get("device")
        devices.append(
            Device(
                sku=entry.get("sku", ""),
                device=entry.get("device", ""),
                name=name or "Unknown",
                device_type=entry.get("type"),
                capabilities=_parse_capabilities(entry.get("capabilities") or []),
            )
        )
    return devices


def parse_device_state(payload: dict[str, Any]) -> DeviceState:
    data = payload.get("payload")
    if data is None:
        data = payload.get("data")
    if data is None:
        data = payload
    return DeviceState(
        sku=data.get("sku", ""),
        device=data.get("device", ""),
        capabilities=_parse_capabilities(data.get("capabilities") or []),
    )
