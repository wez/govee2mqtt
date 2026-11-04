#!/usr/bin/with-contenv bashio

export RUST_BACKTRACE=full
export RUST_LOG_STYLE=always
export XDG_CACHE_HOME=/data

wait_for_mqtt() {
  local max_attempts=30
  local attempt=1

  bashio::log.info "mqtt_host was not explicitly configured, waiting for the Mosquitto broker Add-on to become available"

  while [ $attempt -le $max_attempts ]; do
    if bashio::services.available mqtt ; then
      if timeout 2 bash -c "cat < /dev/null > /dev/tcp/$(bashio::services mqtt host)/$(bashio::services mqtt port)" 2>/dev/null; then
        bashio::log.info "MQTT broker is ready!"
        return 0
      fi
    fi

    bashio::log.info "MQTT broker not ready yet (attempt ${attempt}/${max_attempts}), waiting 2 seconds..."
    sleep 2
    attempt=$((attempt + 1))
  done

  bashio::log.error "MQTT broker did not become available after ${max_attempts} attempts"
  return 1
}

if bashio::config.has_value mqtt_host ; then
  export GOVEE_MQTT_HOST="$(bashio::config mqtt_host)"
else
  if ! wait_for_mqtt ; then
    bashio::exit.nok "Mosquitto MQTT broker is not available"
  fi
  export GOVEE_MQTT_HOST="$(bashio::services mqtt 'host')"
  export GOVEE_MQTT_PORT="$(bashio::services mqtt 'port')"
  export GOVEE_MQTT_USER="$(bashio::services mqtt 'username')"
  export GOVEE_MQTT_PASSWORD="$(bashio::services mqtt 'password')"
fi

if bashio::config.has_value mqtt_port ; then
  export GOVEE_MQTT_PORT="$(bashio::config mqtt_port)"
fi

if bashio::config.has_value mqtt_username ; then
  export GOVEE_MQTT_USER="$(bashio::config mqtt_username)"
fi

if bashio::config.has_value mqtt_password ; then
  export GOVEE_MQTT_PASSWORD="$(bashio::config mqtt_password)"
fi

if bashio::config.has_value debug_level ; then
  export RUST_LOG="$(bashio::config debug_level)"
fi

if bashio::config.has_value govee_email ; then
  export GOVEE_EMAIL="$(bashio::config govee_email)"
fi

if bashio::config.has_value govee_password ; then
  export GOVEE_PASSWORD="$(bashio::config govee_password)"
fi

if bashio::config.has_value govee_api_key ; then
  export GOVEE_API_KEY="$(bashio::config govee_api_key)"
fi

if bashio::config.has_value no_multicast ; then
  export GOVEE_LAN_NO_MULTICAST="$(bashio::config no_multicast)"
fi

if bashio::config.has_value broadcast_all ; then
  export GOVEE_LAN_BROADCAST_ALL="$(bashio::config broadcast_all)"
fi

if bashio::config.has_value global_broadcast ; then
  export GOVEE_LAN_BROADCAST_GLOBAL="$(bashio::config global_broadcast)"
fi

if bashio::config.has_value scan ; then
  export GOVEE_LAN_SCAN="$(bashio::config scan)"
fi

if bashio::config.has_value temperature_scale ; then
  export GOVEE_TEMPERATURE_SCALE="$(bashio::config temperature_scale)"
fi

env | grep GOVEE_ | sed -r 's/_(EMAIL|KEY|PASSWORD)=.*/_\1=REDACTED/'
set -x

cd /app
exec /app/govee serve --device-db=/data/devices.json
