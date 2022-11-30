.PHONY: dist


# Update the version.
version:
	test -n "$(VERSION)"
	sed -i 's/^version.*/version = "$(VERSION)"/g' ./Cargo.toml
	make test
	git add .
	git commit -v -m "Bump version to $(VERSION)"
	git tag -a v$(VERSION) -m "v$(VERSION)"

# Run tests
test:
	docker-compose run --rm chirpstack-mqtt-forwarder cargo clippy --no-deps
	docker-compose run --rm chirpstack-mqtt-forwarder cargo test

# Enter the devshell.
devshell:
	docker-compose run --rm --service-ports chirpstack-mqtt-forwarder bash

# Build distributable binaries.
dist:
	docker-compose run --rm chirpstack-mqtt-forwarder make docker-package-dragino

docker-release-mips-semtech-udp:
	PATH=$$PATH:/opt/mips-linux-muslsf/bin \
	BINDGEN_EXTRA_CLANG_ARGS="--sysroot=/opt/mips-linux-muslsf/mips-linux-muslsf" \
	CC_mips_unknown_linux_musl=mips-linux-muslsf-gcc \
		cargo build --target mips-unknown-linux-musl --release --no-default-features --features semtech_udp

docker-release-armv7hf:
	BINDGEN_EXTRA_CLANG_ARGS="--sysroot=/usr/arm-linux-gnueabihf" \
		cargo build --target armv7-unknown-linux-gnueabihf --release

docker-package-dragino: docker-release-mips-semtech-udp
	cd packaging/vendor/dragino/mips_24kc && ./package.sh

build-dev-image:
	docker build -t chirpstack/chirpstack-mqtt-forwarder-dev-cache -f Dockerfile-devel .
