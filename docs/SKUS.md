# Device Support (Capability-Driven)

The Python v2 implementation does not maintain a model-by-model list. Instead, entity mapping is derived from Govee Platform API v2 capabilities. See `docs/ENGINEERING.md` for mapping rules.

Engineering note: If a device exposes a capability not mapped yet, add a fixture to `test-data/` and update `python/src/govee2mqtt_v2/hass.py` with a new mapping plus tests.
