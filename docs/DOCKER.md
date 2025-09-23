# Running govee2mqtt in Docker

To deploy in docker:

1. Ensure that you have configured the MQTT integration in Home Assistant.

    * [follow these steps](https://www.home-assistant.io/integrations/mqtt/#configuration)

2. Set up a `.env` file.  Here's a skeleton file; you will need to populate
   the values with things that make sense in your environment.
   See [CONFIG.md](CONFIG.md) for more details.

```bash
# Optional, but strongly recommended
GOVEE_EMAIL=user@example.com
GOVEE_PASSWORD=secret
# Optional, but recommended
GOVEE_API_KEY=UUID

GOVEE_MQTT_HOST=mqtt
GOVEE_MQTT_PORT=1883
# Uncomment if your mqtt broker requires authentication
#GOVEE_MQTT_USER=user
#GOVEE_MQTT_PASSWORD=password

# MQTTS (TLS/SSL) Configuration - uncomment to enable
#GOVEE_MQTT_USE_TLS=true
#GOVEE_MQTT_CA_FILE=/app/certs/ca.crt
#GOVEE_MQTT_CERT_FILE=/app/certs/client.crt  # Optional: client certificate
#GOVEE_MQTT_KEY_FILE=/app/certs/client.key   # Optional: client key
#GOVEE_MQTT_INSECURE=false  # Set to true to skip certificate verification

# Specify the temperature scale to use, either C for Celsius
# or F for Fahrenheit
GOVEE_TEMPERATURE_SCALE=C

# Always use colorized output
RUST_LOG_STYLE=always

# If you are asked to set the debug level, uncomment the next line
#RUST_LOG=govee=trace

# Set the timezone for timestamps in the log
TZ=America/Phoenix
```

3. Set up your `docker-compose.yml`:

```yaml
version: '3.8'
services:
  govee2mqtt:
    image: ghcr.io/wez/govee2mqtt:latest
    container_name: govee2mqtt
    restart: unless-stopped
    env_file:
      - .env
    # Host networking is required
    network_mode: host
    # Uncomment if using MQTTS with certificate files
    # volumes:
    #   - ./certs:/app/certs:ro  # Mount certificate directory
```

### MQTTS Certificate Setup

If you're using MQTTS (TLS/SSL), you'll need to provide certificate files:

1. **Create a certificates directory**:
   ```bash
   mkdir certs
   ```

2. **Copy your certificates**:
   ```bash
   # Copy your CA certificate
   cp /path/to/your/ca.crt certs/

   # Optional: Copy client certificates if using client authentication
   cp /path/to/your/client.crt certs/
   cp /path/to/your/client.key certs/
   ```

3. **Update your `.env` file** with MQTTS settings:
   ```bash
   GOVEE_MQTT_USE_TLS=true
   GOVEE_MQTT_CA_FILE=/app/certs/ca.crt
   # Optional client certificates:
   GOVEE_MQTT_CERT_FILE=/app/certs/client.crt
   GOVEE_MQTT_KEY_FILE=/app/certs/client.key
   ```

4. **Uncomment the volume mount** in your `docker-compose.yml`:
   ```yaml
   volumes:
     - ./certs:/app/certs:ro
   ```

4. Launch it:

```console
$ docker compose up -d
```

5. If you need to review the logs:

```console
$ docker logs govee2mqtt --follow
```

