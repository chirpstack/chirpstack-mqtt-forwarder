#!/usr/bin/env bash

set -e

REV="r1"

PACKAGE_NAME=`cargo metadata --no-deps --format-version 1 | jq -r ".packages[0].name"`
PACKAGE_VERSION=`cargo metadata --no-deps --format-version 1| jq -r ".packages[0].version"`
PACKAGE_DESCRIPTION=`cargo metadata --no-deps --format-version 1| jq -r ".packages[0].description"`
DIR=`dirname $0`
PACKAGE_DIR="${DIR}/package"

# Cleanup
rm -rf $PACKAGE_DIR

# CONTROL
mkdir -p $PACKAGE_DIR/CONTROL

cat > $PACKAGE_DIR/CONTROL/control << EOF
Package: $PACKAGE_NAME
Version: $PACKAGE_VERSION-$REV
Architecture: mips_24kc
Maintainer: Orne Brocaar <info@brocaar.com>
Priority: optional
Section: network
Source: N/A
Description: $PACKAGE_DESCRIPTION
EOF

cat > $PACKAGE_DIR/CONTROL/postinst << EOF
#!/bin/sh
/etc/init.d/$PACKAGE_NAME enable
EOF
chmod 0755 $PACKAGE_DIR/CONTROL/postinst

cat > $PACKAGE_DIR/CONTROL/conffiles << EOF
/etc/$PACKAGE_NAME/$PACKAGE_NAME.toml
EOF

# Files
mkdir -p $PACKAGE_DIR/usr/bin
mkdir -p $PACKAGE_DIR/etc/$PACKAGE_NAME
mkdir -p $PACKAGE_DIR/etc/init.d

# Config file
cat > $PACKAGE_DIR/etc/$PACKAGE_NAME/$PACKAGE_NAME.toml << EOF
# See for a full configuration example:
# https://www.chirpstack.io/docs/chirpstack-gateway-bridge/configuration.html

[logging]
  level="info"
  log_to_syslog=true

[backend]
  enabled="semtech_udp" 

  [backend.semtech_udp]
    udp_bind="0.0.0.0:1700"

[mqtt]
  event_topic="eu868/gateway/{{ gateway_id }}/event/{{ event }}"
  command_topic="eu868/gateway/{{ gateway_id }}/command/{{ command }}"
  state_topic="eu868/gateway/{{ gateway_id }}/state/{{ state }}"
  server="tcp://127.0.0.1:1883"
  json=false
  username=""
  password=""
  ca_cert=""
  tls_cert=""
  tls_key=""
EOF
chmod 0644 $PACKAGE_DIR/etc/$PACKAGE_NAME/$PACKAGE_NAME.toml

# Start / stop script
cat > $PACKAGE_DIR/etc/init.d/$PACKAGE_NAME << EOF
#!/bin/sh /etc/rc.common

START=100
STOP=100

start() {
    echo "Starting $PACKAGE_NAME"
	start-stop-daemon \
		-S \
		-b \
		-m \
		-p /var/run/$PACKAGE_NAME.pid \
		-x /usr/bin/$PACKAGE_NAME -- --config /etc/$PACKAGE_NAME/$PACKAGE_NAME.toml
}

stop() {
    echo "Stopping $PACKAGE_NAME"
    start-stop-daemon \
		-K \
		-p /var/run/$PACKAGE_NAME.pid
}
EOF
chmod 0755 $PACKAGE_DIR/etc/init.d/$PACKAGE_NAME

# Binary
cp ../../../../.rust/target/mips-unknown-linux-musl/release/$PACKAGE_NAME $PACKAGE_DIR/usr/bin/$PACKAGE_NAME

# Package
opkg-build -c -o root -g root $PACKAGE_DIR

# Cleanup
rm -rf $PACKAGE_DIR
