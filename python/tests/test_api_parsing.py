import json
from pathlib import Path

from govee2mqtt_v2.models import parse_device_list, parse_device_state


def _load_json(relative_path: str) -> dict:
    repo_root = Path(__file__).resolve().parents[2]
    path = repo_root / relative_path
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def test_parse_device_list() -> None:
    payload = _load_json("test-data/list_devices_2.json")
    devices = parse_device_list(payload)
    assert devices
    assert devices[0].name == "Floor Lamp"
    assert devices[0].sku
    assert devices[0].device


def test_parse_device_state() -> None:
    payload = _load_json("test-data/get_device_state.json")
    state = parse_device_state(payload)
    assert state.device
    assert state.sku
    assert any(cap.instance == "brightness" for cap in state.capabilities)
