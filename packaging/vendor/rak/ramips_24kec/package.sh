#!/usr/bin/env bash

set -e

REV="r1"

PACKAGE_NAME=`cargo metadata --no-deps --format-version 1 | jq -r ".packages[0].name"`
PACKAGE_VERSION=`cargo metadata --no-deps --format-version 1| jq -r ".packages[0].version"`
PACKAGE_DESCRIPTION=`cargo metadata --no-deps --format-version 1| jq -r ".packages[0].description"`
BIN_PATH="../../../../target/mipsel-unknown-linux-musl/release/${PACKAGE_NAME}"
DIR=`dirname $0`
PACKAGE_DIR="${DIR}/package"

# Cleanup
rm -rf $PACKAGE_DIR

# CONTROL
mkdir -p $PACKAGE_DIR/CONTROL
cat > $PACKAGE_DIR/CONTROL/control << EOF
Package: $PACKAGE_NAME
Version: $PACKAGE_VERSION-$REV
Architecture: ramips_24kec
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

cp files/$PACKAGE_NAME.init $PACKAGE_DIR/etc/init.d/$PACKAGE_NAME
cp files/$PACKAGE_NAME.toml $PACKAGE_DIR/etc/$PACKAGE_NAME/$PACKAGE_NAME.toml

# Binary
cp $BIN_PATH $PACKAGE_DIR/usr/bin/$PACKAGE_NAME

# Package
opkg-build -c -o root -g root $PACKAGE_DIR

# Cleanup
rm -rf $PACKAGE_DIR
