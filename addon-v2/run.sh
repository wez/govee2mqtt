#!/usr/bin/with-contenv bashio

export GOVEE_API_KEY="$(bashio::config 'govee_api_key')"
export MQTT_HOST="$(bashio::config 'mqtt_host')"
export MQTT_PORT="$(bashio::config 'mqtt_port')"
export MQTT_USERNAME="$(bashio::config 'mqtt_username')"
export MQTT_PASSWORD="$(bashio::config 'mqtt_password')"
export MQTT_BASE_TOPIC="$(bashio::config 'mqtt_base_topic')"
export POLL_INTERVAL_SECONDS="$(bashio::config 'poll_interval_seconds')"
export LOG_LEVEL="$(bashio::config 'log_level')"

exec govee2mqtt-v2
