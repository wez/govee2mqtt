from __future__ import annotations

import json
import logging
from collections.abc import Callable

import paho.mqtt.client as mqtt

logger = logging.getLogger(__name__)

CommandHandler = Callable[[str, str, str], None]


class MqttClient:
    def __init__(
        self,
        *,
        host: str,
        port: int,
        username: str | None,
        password: str | None,
        base_topic: str,
    ) -> None:
        self._host = host
        self._port = port
        self._username = username
        self._password = password
        self._base_topic = base_topic
        self._client = mqtt.Client()
        if username:
            self._client.username_pw_set(username, password)
        self._client.on_connect = self._on_connect
        self._client.on_message = self._on_message
        self._command_handler: CommandHandler | None = None

    def set_command_handler(self, handler: CommandHandler) -> None:
        self._command_handler = handler

    def connect(self) -> None:
        logger.info("Connecting to MQTT broker %s:%d", self._host, self._port)
        self._client.connect(self._host, self._port, keepalive=60)
        self._client.loop_start()

    def disconnect(self) -> None:
        self._client.loop_stop()
        self._client.disconnect()

    def publish(self, topic: str, payload: str | dict, *, retain: bool = False) -> None:
        if isinstance(payload, dict):
            payload_str = json.dumps(payload)
        else:
            payload_str = payload
        self._client.publish(topic, payload_str, retain=retain)

    def publish_discovery(self, topic: str, payload: dict) -> None:
        self.publish(topic, payload, retain=True)

    def _on_connect(self, client: mqtt.Client, userdata: object, flags: dict, rc: int) -> None:
        if rc != 0:
            logger.error("MQTT connection failed with rc=%s", rc)
            return
        logger.info("Connected to MQTT broker")
        command_topic = f"{self._base_topic}/+/+/set"
        client.subscribe(command_topic)

    def _on_message(self, client: mqtt.Client, userdata: object, msg: mqtt.MQTTMessage) -> None:
        if not self._command_handler:
            return
        topic = msg.topic
        payload = msg.payload.decode("utf-8") if msg.payload else ""
        parts = topic.split("/")
        if len(parts) < 3:
            return
        base, device_id, entity = parts[0], parts[1], parts[2]
        if base != self._base_topic:
            return
        self._command_handler(device_id, entity, payload)
