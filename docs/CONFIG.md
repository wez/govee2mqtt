# Configuration Options

## Govee Credentials

While `govee2mqtt` can run without any govee credentials, it can only discover
and control the devices for which you have already enabled LAN control.

It is recommended that you configure at least your Govee username and password
prior to your first run, as that is the only way for `govee2mqtt` to determine
room names to pre-assign your lights into the appropriate Home Assistant areas.

For scene control, for devices that don't support the LAN API, a Govee API Key
is required.  If you don't already have one, [you can find instructions on
obtaining one
here](https://developer.govee.com/reference/apply-you-govee-api-key).

|CLI|ENV|AddOn|Purpose|
|---|---|-----|-------|
|`--govee-email`|`GOVEE_EMAIL`|`govee_email`|The email address you registered with your govee account|
|`--govee-password`|`GOVEE_PASSWORD`|`govee_password`|The password you registered for your govee account|
|`--api-key`|`GOVEE_API_KEY`|`govee_api_key`|The API key you requested from Govee support|

*Concerned about sharing your credentials? See [Privacy](PRIVACY.md) for
information about how data is used and retained by `govee2mqtt`*

## LAN API Control

A number of Govee's devices support a local control protocol that doesn't require
your primary internet connection to be online.  This offers the lowest latency
for control and is the preferred way for `govee2mqtt` to interact with your
devices.

The [Govee LAN API is described in more detail
here](https://app-h5.govee.com/user-manual/wlan-guide), including a list of
supported devices.

*Note that you must use the Govee Home app to enable the LAN API for each
individual device before it will be possible for `govee2mqtt` to control
it via the LAN API.*

In theory the LAN API is zero-configuration and auto-discovery, but this
relies on your network supporting multicast-UDP, which is challenging
on some networks, especially across wifi access points and routers.

|CLI|ENV|AddOn|Purpose|
|---|---|-----|-------|
|`--no-multicast`|`GOVEE_LAN_NO_MULTICAST=true`|`no_multicast`|Do not multicast discovery packets to the Govee multicast group `239.255.255.250`. It is not recommended to use this option.|
|`--broadcast-all`|`GOVEE_LAN_BROADCAST_ALL=true`|`broadcast_all`|Enumerate all non-loopback network interfaces and send discovery packets to the broadcast address of each one, individually. This may be a good option if multicast-UDP doesn't work well on your network|
|`--global-broadcast`|`GOVEE_LAN_BROADCAST_GLOBAL=true`|`global_broadcast`|Send discovery packets to the global broadcast address `255.255.255.255`. This may be a possible solution if multicast-UDP doesn't work well on your network.|
|`--scan`|`GOVEE_LAN_SCAN=10.0.0.1,10.0.0.2`|`scan`|Specify a list of addresses that should be scanned by sending them discovery packets. Each element in the list can be an individual IP address (eg: the address of a specific device: be sure to assign it a static IP in your DHCP or other network setup!) or a network broadcast address like `10.0.0.255` for networks that are reachable but not directly plumbed on the machine where `govee2mqtt` is running.|

[Read more about LAN API Requirements here](LAN.md)

## MQTT Configuration

In order to make your devices appear in Home Assistant, you will need to have configured Home Assistant with an MQTT broker.

  * [follow these steps](https://www.home-assistant.io/integrations/mqtt/#configuration)

You will also need to configure `govee2mqtt` to use the same broker:

|CLI|ENV|AddOn|Purpose|
|---|---|-----|-------|
|`--mqtt-host`|`GOVEE_MQTT_HOST`|`mqtt_host`|The host name or IP address of your mqtt broker. This should be the same broker that you have configured in Home Assistant.|
|`--mqtt-port`|`GOVEE_MQTT_PORT`|`mqtt_port`|The port number of the mqtt broker. The default is `1883` for unencrypted MQTT or `8883` for MQTTS when TLS is enabled|
|`--mqtt-username`|`GOVEE_MQTT_USER`|`mqtt_username`|If your broker requires authentication, the username to use|
|`--mqtt-password`|`GOVEE_MQTT_PASSWORD`|`mqtt_password`|If your broker requires authentication, the password to use|

## MQTTS (TLS/SSL) Configuration

For secure MQTT connections using TLS/SSL encryption, you can configure the following options:

|CLI|ENV|AddOn|Purpose|
|---|---|-----|-------|
|`--mqtt-use-tls`|`GOVEE_MQTT_USE_TLS`|`mqtt_use_tls`|Enable TLS/SSL for MQTT connections (MQTTS)|
|`--mqtt-ca-file`|`GOVEE_MQTT_CA_FILE`|`mqtt_ca_file`|Path to the PEM encoded CA certificate file|
|`--mqtt-cert-file`|`GOVEE_MQTT_CERT_FILE`|`mqtt_cert_file`|Path to the PEM encoded client certificate file (optional)|
|`--mqtt-key-file`|`GOVEE_MQTT_KEY_FILE`|`mqtt_key_file`|Path to the PEM encoded client private key file (optional)|
|`--mqtt-insecure`|`GOVEE_MQTT_INSECURE`|`mqtt_insecure`|Skip certificate verification (insecure, for testing only)|

### MQTTS Examples

#### Basic MQTTS with CA certificate only:
```bash
govee serve \
  --mqtt-host your-broker.example.com \
  --mqtt-use-tls \
  --mqtt-ca-file /path/to/ca.crt
```

#### MQTTS with client certificate authentication:
```bash
govee serve \
  --mqtt-host your-broker.example.com \
  --mqtt-use-tls \
  --mqtt-ca-file /path/to/ca.crt \
  --mqtt-cert-file /path/to/client.crt \
  --mqtt-key-file /path/to/client.key
```

#### Docker environment variables for MQTTS:
```yaml
environment:
  - GOVEE_MQTT_HOST=your-broker.example.com
  - GOVEE_MQTT_USE_TLS=true
  - GOVEE_MQTT_CA_FILE=/app/certs/ca.crt
  - GOVEE_MQTT_CERT_FILE=/app/certs/client.crt  # Optional
  - GOVEE_MQTT_KEY_FILE=/app/certs/client.key   # Optional
```

### Setting up MQTTS with Mosquitto

If you're using Eclipse Mosquitto as your MQTT broker, here's how to configure it for TLS:

1. **Generate certificates** (for testing):
   ```bash
   # Generate CA private key
   openssl genrsa -out ca.key 2048

   # Generate CA certificate
   openssl req -new -x509 -days 365 -key ca.key -out ca.crt \
     -subj "/C=US/ST=Test/L=Test/O=Test/CN=Test CA"

   # Generate server private key
   openssl genrsa -out server.key 2048

   # Generate server certificate
   openssl req -new -key server.key -out server.csr \
     -subj "/C=US/ST=Test/L=Test/O=Test/CN=your-broker-hostname"
   openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key \
     -CAcreateserial -out server.crt -days 365
   ```

2. **Configure Mosquitto** (`mosquitto.conf`):
   ```
   # Standard MQTT
   listener 1883

   # MQTTS
   listener 8883
   cafile /path/to/ca.crt
   certfile /path/to/server.crt
   keyfile /path/to/server.key
   require_certificate false  # Set to true for client cert auth
   ```

3. **Configure govee2mqtt**:
   ```bash
   export GOVEE_MQTT_HOST=your-broker-hostname
   export GOVEE_MQTT_USE_TLS=true
   export GOVEE_MQTT_CA_FILE=/path/to/ca.crt
   govee serve
   ```

