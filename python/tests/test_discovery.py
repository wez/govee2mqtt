from govee2mqtt_v2.discovery import light_discovery_payload, sensor_discovery_payloads
from govee2mqtt_v2.models import Capability, Device


def _cap(cap_type: str, instance: str) -> Capability:
    return Capability(type=cap_type, instance=instance)


def test_light_discovery_payload_rgb() -> None:
    device = Device(
        sku="H6072",
        device="AA:BB:CC:DD:AA:BB:CC:DD",
        name="Floor Lamp",
        device_type="devices.types.light",
        capabilities=[
            _cap("devices.capabilities.on_off", "powerSwitch"),
            _cap("devices.capabilities.range", "brightness"),
            _cap("devices.capabilities.color_setting", "colorRgb"),
            _cap("devices.capabilities.color_setting", "colorTemperatureK"),
        ],
    )
    topic, payload = light_discovery_payload(device, "govee2mqtt")
    assert topic.endswith("/config")
    assert payload["schema"] == "json"
    assert payload["brightness"] is True
    assert payload["rgb"] is True
    assert payload["color_temp"] is True


def test_sensor_discovery_payload_temp_humidity() -> None:
    device = Device(
        sku="H7143",
        device="11:22:33:44:55:66:77:88",
        name="Air Monitor",
        device_type="devices.types.sensor",
        capabilities=[
            _cap("devices.capabilities.range", "temperature"),
            _cap("devices.capabilities.range", "humidity"),
        ],
    )
    payloads = sensor_discovery_payloads(device, "govee2mqtt")
    assert len(payloads) == 2
    topics = [topic for topic, _ in payloads]
    assert any("sensor" in topic for topic in topics)
