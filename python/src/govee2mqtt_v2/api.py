from __future__ import annotations

import logging
import time
from typing import Any

import httpx

from .models import Device, DeviceState, parse_device_list, parse_device_state

logger = logging.getLogger(__name__)


class GoveeApiClient:
    def __init__(
        self,
        api_key: str,
        *,
        base_url: str,
        timeout: float = 10.0,
        max_retries: int = 5,
    ) -> None:
        self._api_key = api_key
        self._timeout = timeout
        self._max_retries = max_retries
        self._client = httpx.Client(
            base_url=base_url,
            headers={
                "Govee-API-Key": api_key,
                "Content-Type": "application/json",
            },
            timeout=timeout,
        )

    def close(self) -> None:
        self._client.close()

    def _request(
        self, method: str, url: str, *, json: dict[str, Any] | None = None
    ) -> dict[str, Any]:
        backoff = 1.0
        for attempt in range(1, self._max_retries + 1):
            response = self._client.request(method, url, json=json)
            if response.status_code == 429:
                retry_after = response.headers.get("Retry-After")
                if retry_after and retry_after.isdigit():
                    sleep_seconds = float(retry_after)
                else:
                    sleep_seconds = backoff
                logger.warning(
                    "Rate limited by Govee API (429). Backing off for %.1fs (attempt %d/%d).",
                    sleep_seconds,
                    attempt,
                    self._max_retries,
                )
                time.sleep(sleep_seconds)
                backoff = min(backoff * 2.0, 60.0)
                continue
            response.raise_for_status()
            return response.json()
        raise RuntimeError("Exceeded maximum retries due to rate limiting")

    def list_devices(self) -> list[Device]:
        payload = self._request("GET", "/user/devices")
        return parse_device_list(payload)

    def get_device_state(self, device: Device) -> DeviceState:
        payload = self._request(
            "POST",
            "/device/state",
            json={"device": device.device, "sku": device.sku},
        )
        return parse_device_state(payload)

    def control_device(
        self, device: Device, *, capability_type: str, instance: str, value: Any
    ) -> None:
        self._request(
            "POST",
            "/device/control",
            json={
                "device": device.device,
                "sku": device.sku,
                "capability": {
                    "type": capability_type,
                    "instance": instance,
                    "value": value,
                },
            },
        )
