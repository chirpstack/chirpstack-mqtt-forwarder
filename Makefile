.PHONY: dist

# Compile the binaries for all targets.
build:
	cross build --target aarch64-unknown-linux-musl --release
	cross build --target armv5te-unknown-linux-musleabi --release
	cross build --target armv7-unknown-linux-musleabihf --release
	cross build --target mips-unknown-linux-musl --release --no-default-features --features semtech_udp

# Build distributable binaries.
dist: build package

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
	cargo clean
	rm -rf dist

# Run tests
test:
	cargo clippy --no-deps
	cargo test

# Enter the devshell.
devshell:
	nix-shell

# Enter the Docker based devshell.
docker-devshell:
	docker-compose run --rm chirpstack-mqtt-forwarder

# Package the compiled binaries.
package: package-targz-armv7hf package-targz-arm64 \
	package-dragino \
	package-multitech-conduit \
	package-multitech-conduit-ap \
	package-tektelic-kona \
	package-kerlink-klkgw

package-targz-armv7hf:
	$(eval PKG_VERSION := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'))
	mkdir -p dist
	tar -czvf dist/chirpstack-mqtt-forwarder_$(PKG_VERSION)_armv7hf.tar.gz -C target/armv7-unknown-linux-musleabihf/release chirpstack-mqtt-forwarder

package-targz-arm64:
	$(eval PKG_VERSION := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'))
	mkdir -p dist
	tar -czvf dist/chirpstack-mqtt-forwarder_$(PKG_VERSION)_arm64.tar.gz -C target/aarch64-unknown-linux-musl/release chirpstack-mqtt-forwarder

package-dragino:
	cd packaging/vendor/dragino/mips_24kc && ./package.sh
	mkdir -p dist/vendor/dragino/mips_24kc
	cp packaging/vendor/dragino/mips_24kc/*.ipk dist/vendor/dragino/mips_24kc

package-multitech-conduit:
	cd packaging/vendor/multitech/conduit && ./package.sh
	mkdir -p dist/vendor/multitech/conduit
	cp packaging/vendor/multitech/conduit/*.ipk dist/vendor/multitech/conduit

package-multitech-conduit-ap:
	cd packaging/vendor/multitech/conduit_ap && ./package.sh
	mkdir -p dist/vendor/multitech/conduit_ap
	cp packaging/vendor/multitech/conduit_ap/*.ipk dist/vendor/multitech/conduit_ap

package-tektelic-kona:
	cd packaging/vendor/tektelic/kona && ./package.sh
	mkdir -p dist/vendor/tektelic/kona
	cp packaging/vendor/tektelic/kona/*.ipk dist/vendor/tektelic/kona

package-kerlink-klkgw:
	cd packaging/vendor/kerlink/klkgw && ./package.sh
	mkdir -p dist/vendor/kerlink/klkgw
	cp packaging/vendor/kerlink/klkgw/*.ipk dist/vendor/kerlink/klkgw
