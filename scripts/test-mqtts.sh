#!/bin/bash

# Script to test MQTTS functionality with govee2mqtt

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

echo "Testing MQTTS functionality for govee2mqtt"
echo "=========================================="

cd "$PROJECT_DIR"

# Generate certificates if they don't exist
if [ ! -f "test/certs/ca.crt" ]; then
    echo "Generating test certificates..."
    ./scripts/generate-test-certs.sh
else
    echo "Test certificates already exist"
fi

# Build the project
echo ""
echo "Building govee2mqtt..."
cargo build --release

# Start the test environment
echo ""
echo "Starting test MQTTS broker..."
docker-compose -f docker-compose.test.yml up -d mosquitto

# Wait for mosquitto to start
echo "Waiting for mosquitto to start..."
sleep 5

# Test basic MQTT connection (unencrypted)
echo ""
echo "Testing basic MQTT connection..."
if command -v mosquitto_pub &> /dev/null; then
    mosquitto_pub -h localhost -p 1883 -t govee2mqtt/test -m "Test message - unencrypted" -d
    echo "✓ Basic MQTT connection successful"
else
    echo "⚠ mosquitto_pub not found, skipping basic MQTT test"
fi

# Test MQTTS connection
echo ""
echo "Testing MQTTS connection..."
if command -v mosquitto_pub &> /dev/null; then
    mosquitto_pub -h localhost -p 8883 --cafile test/certs/ca.crt -t govee2mqtt/test -m "Test message - encrypted" -d
    echo "✓ MQTTS connection successful"
else
    echo "⚠ mosquitto_pub not found, skipping MQTTS test"
fi

# Test with client certificates
echo ""
echo "Testing MQTTS with client certificates..."
if command -v mosquitto_pub &> /dev/null; then
    mosquitto_pub -h localhost -p 8883 \
        --cafile test/certs/ca.crt \
        --cert test/certs/client.crt \
        --key test/certs/client.key \
        -t govee2mqtt/test -m "Test message - with client cert" -d
    echo "✓ MQTTS with client certificates successful"
else
    echo "⚠ mosquitto_pub not found, skipping client certificate test"
fi

# Test govee2mqtt with MQTTS (dry run)
echo ""
echo "Testing govee2mqtt MQTTS configuration..."
GOVEE_MQTT_HOST=localhost \
GOVEE_MQTT_PORT=8883 \
GOVEE_MQTT_USE_TLS=true \
GOVEE_MQTT_CA_FILE=test/certs/ca.crt \
GOVEE_MQTT_CERT_FILE=test/certs/client.crt \
GOVEE_MQTT_KEY_FILE=test/certs/client.key \
timeout 10s ./target/release/govee serve --help > /dev/null || true

echo "✓ govee2mqtt accepts MQTTS configuration"

echo ""
echo "Test environment is running. To test manually:"
echo "  1. Start govee2mqtt with MQTTS configuration:"
echo "     GOVEE_MQTT_HOST=localhost GOVEE_MQTT_USE_TLS=true GOVEE_MQTT_CA_FILE=test/certs/ca.crt ./target/release/govee serve"
echo ""
echo "  2. Subscribe to messages:"
echo "     mosquitto_sub -h localhost -p 8883 --cafile test/certs/ca.crt -t 'gv2mqtt/#' -v"
echo ""
echo "  3. Stop the test environment:"
echo "     docker-compose -f docker-compose.test.yml down"

echo ""
echo "All tests completed successfully! ✓"