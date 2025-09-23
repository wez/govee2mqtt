# Testing MQTTS Support

This document describes how to test the MQTTS (MQTT over TLS/SSL) functionality in govee2mqtt.

## Quick Test

Run the automated test script:

```bash
./scripts/test-mqtts.sh
```

This will:
1. Generate test certificates if needed
2. Build the project
3. Start a test mosquitto broker
4. Run basic connectivity tests
5. Provide manual testing instructions

## Manual Testing

### Prerequisites

1. Install mosquitto client tools:
   ```bash
   # macOS
   brew install mosquitto

   # Ubuntu/Debian
   sudo apt-get install mosquitto-clients

   # Other systems - see mosquitto documentation
   ```

2. Generate test certificates:
   ```bash
   ./scripts/generate-test-certs.sh
   ```

### Start Test Environment

Start the test MQTTS broker:

```bash
docker-compose -f docker-compose.test.yml up -d mosquitto
```

Wait a few seconds for the broker to start.

### Test MQTT Connectivity

#### Test 1: Basic MQTT (unencrypted)
```bash
mosquitto_pub -h localhost -p 1883 -t govee2mqtt/test -m "Test message - unencrypted" -d
```

#### Test 2: Subscribe to unencrypted messages
```bash
mosquitto_sub -h localhost -p 1883 -t 'govee2mqtt/#' -v
```

#### Test 3: Basic MQTTS (encrypted)
```bash
mosquitto_pub -h localhost -p 8883 --cafile test/certs/ca.crt --insecure -t govee2mqtt/test -m "Test message - encrypted" -d
```

#### Test 4: Subscribe to encrypted messages
```bash
mosquitto_sub -h localhost -p 8883 --cafile test/certs/ca.crt --insecure -t 'gv2mqtt/#' -v
```

#### Test 5: MQTTS with client certificates
```bash
mosquitto_pub -h localhost -p 8883 \
    --cafile test/certs/ca.crt \
    --cert test/certs/client.crt \
    --key test/certs/client.key \
    --insecure \
    -t govee2mqtt/test -m "Test message - with client cert" -d
```

#### Test 6: Run govee2mqtt with MQTTS
```bash
GOVEE_MQTT_HOST=localhost \
GOVEE_MQTT_USE_TLS=true \
GOVEE_MQTT_CA_FILE=test/certs/ca.crt \
./target/release/govee serve
```

## Production Testing

### With Let's Encrypt Certificates

1. Configure your environment variables:
   ```bash
   GOVEE_MQTT_HOST=your-mqtt-broker.com
   GOVEE_MQTT_PORT=8883
   GOVEE_MQTT_USE_TLS=true
   GOVEE_MQTT_CA_FILE=/etc/ssl/certs/ca-certificates.crt
   GOVEE_MQTT_USERNAME=your-username
   GOVEE_MQTT_PASSWORD=your-password
   ```

2. Subscribe to messages on your broker:
   ```bash
   mosquitto_sub -h your-mqtt-broker.com -p 8883 \
       --cafile /etc/ssl/certs/ca-certificates.crt \
       -u your-username -P your-password \
       -t 'gv2mqtt/#' -v
   ```

3. Run govee2mqtt and verify encrypted messages appear.

### Docker Testing

Use the provided docker-compose configuration:

```yaml
version: '3.8'
services:
  govee2mqtt:
    image: ghcr.io/vfilby/govee2mqtt:latest
    environment:
      - GOVEE_MQTT_HOST=your-mqtt-broker.com
      - GOVEE_MQTT_PORT=8883
      - GOVEE_MQTT_USE_TLS=true
      - GOVEE_MQTT_CA_FILE=/etc/ssl/certs/ca-certificates.crt
      - GOVEE_MQTT_USERNAME=your-username
      - GOVEE_MQTT_PASSWORD=your-password
      - GOVEE_API_KEY=your-govee-api-key
      - GOVEE_EMAIL=your-govee-email
      - GOVEE_PASSWORD=your-govee-password
    volumes:
      - /etc/ssl/certs:/etc/ssl/certs:ro
```

## Troubleshooting

### Connection Refused Errors

If tests 4, 5, or 6 show "connection refused":

1. Check if mosquitto broker is running:
   ```bash
   docker-compose -f docker-compose.test.yml logs mosquitto
   ```

2. Verify test certificates exist:
   ```bash
   ls -la test/certs/
   ```

3. Regenerate certificates if needed:
   ```bash
   rm -rf test/certs/
   ./scripts/generate-test-certs.sh
   ```

### Certificate Verification Errors

For self-signed certificates in testing, this is expected behavior. In production with Let's Encrypt certificates, ensure:

1. System CA bundle is available at `/etc/ssl/certs/ca-certificates.crt`
2. Your MQTT broker's certificate is valid and not expired
3. Clock synchronization is correct

### Authentication Errors

If using username/password authentication:

1. Verify credentials are correct
2. Check broker logs for authentication failures
3. Ensure broker is configured to accept the credentials

## Cleanup

Stop the test environment:

```bash
docker-compose -f docker-compose.test.yml down
```

Remove test certificates (optional):

```bash
rm -rf test/
```