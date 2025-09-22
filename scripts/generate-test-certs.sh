#!/bin/bash

# Script to generate self-signed certificates for MQTTS testing

set -e

CERT_DIR="test/certs"
DAYS=3650

echo "Creating certificate directory: $CERT_DIR"
mkdir -p "$CERT_DIR"
cd "$CERT_DIR"

# Generate CA private key
echo "Generating CA private key..."
openssl genrsa -out ca.key 2048

# Generate CA certificate
echo "Generating CA certificate..."
openssl req -new -x509 -days $DAYS -key ca.key -out ca.crt -subj "/C=US/ST=Test/L=Test/O=Govee2MQTT Test/CN=Test CA"

# Generate server private key
echo "Generating server private key..."
openssl genrsa -out server.key 2048

# Generate server certificate signing request
echo "Generating server certificate signing request..."
openssl req -new -key server.key -out server.csr -subj "/C=US/ST=Test/L=Test/O=Govee2MQTT Test/CN=mosquitto"

# Generate server certificate
echo "Generating server certificate..."
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial -out server.crt -days $DAYS

# Generate client private key
echo "Generating client private key..."
openssl genrsa -out client.key 2048

# Generate client certificate signing request
echo "Generating client certificate signing request..."
openssl req -new -key client.key -out client.csr -subj "/C=US/ST=Test/L=Test/O=Govee2MQTT Test/CN=govee2mqtt-client"

# Generate client certificate
echo "Generating client certificate..."
openssl x509 -req -in client.csr -CA ca.crt -CAkey ca.key -CAcreateserial -out client.crt -days $DAYS

# Clean up CSR files
rm server.csr client.csr

# Set appropriate permissions
chmod 644 *.crt
chmod 600 *.key

echo "Certificates generated successfully in $CERT_DIR:"
ls -la

echo ""
echo "To test MQTTS connection, use:"
echo "  docker-compose -f docker-compose.test.yml up"
echo ""
echo "Or test manually with mosquitto_pub:"
echo "  mosquitto_pub -h localhost -p 8883 --cafile $CERT_DIR/ca.crt -t test/topic -m 'Hello MQTTS!'"