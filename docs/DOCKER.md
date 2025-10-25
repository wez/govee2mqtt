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
name: govee2mqtt
services:
  govee2mqtt:
    image: ghcr.io/wez/govee2mqtt:latest
    container_name: govee2mqtt
    restart: unless-stopped
    env_file:
      - .env
    # Host networking is required
    network_mode: host
# By default, a Docker volume will be used to persist data. If you prefer to mount this on your host, you can do so as follows:
#    volumes:
#      - '/path/to/data:/data'
```

4. Launch it:

```console
$ docker compose up -d
```

5. If you need to review the logs:

```console
$ docker logs govee2mqtt --follow
```

