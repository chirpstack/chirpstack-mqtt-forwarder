.PHONY: dist


# Update the version.
version:
	test -n "$(VERSION)"
	sed -i 's/^version.*/version = "$(VERSION)"/g' ./Cargo.toml
	make test
	git add .
	git commit -v -m "Bump version to $(VERSION)"
	git tag -a v$(VERSION) -m "v$(VERSION)"

# Cleanup dist.
clean:
	rm -rf dist

# Run tests
test:
	docker-compose run --rm chirpstack-mqtt-forwarder cargo clippy --no-deps
	docker-compose run --rm chirpstack-mqtt-forwarder cargo test

# Enter the devshell.
devshell:
	docker-compose run --rm --service-ports chirpstack-mqtt-forwarder bash

# Build distributable binaries.
dist:
	docker-compose run --rm chirpstack-mqtt-forwarder make \
		docker-package-targz-armv7hf \
		docker-package-targz-arm64 \
		docker-package-dragino \
		docker-package-multitech-conduit \
		docker-package-tektelic-kona \
		docker-package-kerlink-klkgw

build-dev-image:
	docker build -t chirpstack/chirpstack-mqtt-forwarder-dev-cache -f Dockerfile-devel .

###
# All docker-... commands must be executed within the Docker Compose environment.
###

docker-release-mips-semtech-udp:
	PATH=$$PATH:/opt/mips-linux-muslsf/bin \
	BINDGEN_EXTRA_CLANG_ARGS="--sysroot=/opt/mips-linux-muslsf/mips-linux-muslsf" \
	CC_mips_unknown_linux_musl=mips-linux-muslsf-gcc \
		cargo build --target mips-unknown-linux-musl --release --no-default-features --features semtech_udp

docker-release-armv7hf:
	BINDGEN_EXTRA_CLANG_ARGS="--sysroot=/usr/arm-linux-gnueabihf" \
		cargo build --target armv7-unknown-linux-gnueabihf --release

docker-release-armv5:
	BINDGEN_EXTRA_CLANG_ARGS="--sysroot=/usr/arm-linux-gnueabi" \
		cargo build --target armv5te-unknown-linux-gnueabi --release

docker-release-arm64:
	BINDGEN_EXTRA_CLANG_ARGS="--sysroot=/usr/aarch64-linux-gnu" \
		cargo build --target aarch64-unknown-linux-gnu --release

docker-package-dragino: docker-release-mips-semtech-udp
	cd packaging/vendor/dragino/mips_24kc && ./package.sh
	mkdir -p dist/vendor/dragino/mips_24kc
	cp packaging/vendor/dragino/mips_24kc/*.ipk dist/vendor/dragino/mips_24kc

docker-package-multitech-conduit: docker-release-armv5
	cd packaging/vendor/multitech/conduit && ./package.sh
	mkdir -p dist/vendor/multitech/conduit
	cp packaging/vendor/multitech/conduit/*.ipk dist/vendor/multitech/conduit

docker-package-tektelic-kona: docker-release-armv7hf
	cd packaging/vendor/tektelic/kona && ./package.sh
	mkdir -p dist/vendor/tektelic/kona
	cp packaging/vendor/tektelic/kona/*.ipk dist/vendor/tektelic/kona

docker-package-kerlink-klkgw: docker-release-armv7hf
	cd packaging/vendor/kerlink/klkgw && ./package.sh
	mkdir -p dist/vendor/kerlink/klkgw
	cp packaging/vendor/kerlink/klkgw/*.ipk dist/vendor/kerlink/klkgw

docker-package-targz-armv7hf: docker-release-armv7hf
	$(eval PKG_VERSION := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'))
	mkdir -p dist
	tar -czvf dist/chirpstack-mqtt-forwarder_$(PKG_VERSION)_armv7hf.tar.gz -C target/armv7-unknown-linux-gnueabihf/release chirpstack-mqtt-forwarder

docker-package-targz-arm64: docker-release-arm64
	$(eval PKG_VERSION := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'))
	mkdir -p dist
	tar -czvf dist/chirpstack-mqtt-forwarder_$(PKG_VERSION)_arm64.tar.gz -C target/aarch64-unknown-linux-gnu/release chirpstack-mqtt-forwarder

