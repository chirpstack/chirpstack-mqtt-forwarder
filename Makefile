.PHONY: dist

# Compile the binaries for all targets.
build: build-aarch64-unknown-linux-musl \
	build-armv5te-unknown-linux-musleabi \
	build-armv7-unknown-linux-musleabihf \
	build-mips-unknown-linux-musl \
	build-mipsel-unknown-linux-musl

build-aarch64-unknown-linux-musl:
	cross build --target aarch64-unknown-linux-musl --release

build-armv5te-unknown-linux-musleabi:
	cross build --target armv5te-unknown-linux-musleabi --release

build-armv7-unknown-linux-musleabihf:
	cross build --target armv7-unknown-linux-musleabihf --release

build-mips-unknown-linux-musl:
	cross build --target mips-unknown-linux-musl --release --no-default-features --features semtech_udp

build-mipsel-unknown-linux-musl:
	cross build --target mipsel-unknown-linux-musl --release --no-default-features --features semtech_udp

# Build distributable binaries for all targets.
dist: dist-aarch64-unknown-linux-musl \
	dist-armv5te-unknown-linux-musleabi \
	dist-armv7-unknown-linux-musleabihf \
	dist-mips-unknown-linux-musl \
	dist-mipsel-unknown-linux-musl

dist-aarch64-unknown-linux-musl: build-aarch64-unknown-linux-musl package-aarch64-unknown-linux-musl

dist-armv5te-unknown-linux-musleabi: build-armv5te-unknown-linux-musleabi package-armv5te-unknown-linux-musleabi

dist-armv7-unknown-linux-musleabihf: build-armv7-unknown-linux-musleabihf package-armv7-unknown-linux-musleabihf

dist-mips-unknown-linux-musl: build-mips-unknown-linux-musl package-mips-unknown-linux-musl

dist-mipsel-unknown-linux-musl: build-mipsel-unknown-linux-musl package-mipsel-unknown-linux-musl

# Package the compiled binaries
package-aarch64-unknown-linux-musl:
	$(eval PKG_VERSION := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'))
	mkdir -p dist

	# .tar.gz
	tar -czvf dist/chirpstack-mqtt-forwarder_$(PKG_VERSION)_arm64.tar.gz -C target/aarch64-unknown-linux-musl/release chirpstack-mqtt-forwarder

	# .deb
	cargo deb --target aarch64-unknown-linux-musl --no-build --no-strip
	cp target/aarch64-unknown-linux-musl/debian/*.deb ./dist

package-armv7-unknown-linux-musleabihf: package-tektelic-kona package-kerlink-klkgw package-multitech-conduit-ap3
	$(eval PKG_VERSION := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version'))
	mkdir -p dist

	# .tar.gz
	tar -czvf dist/chirpstack-mqtt-forwarder_$(PKG_VERSION)_armv7hf.tar.gz -C target/armv7-unknown-linux-musleabihf/release chirpstack-mqtt-forwarder

	# .deb
	cargo deb --target armv7-unknown-linux-musleabihf --no-build --no-strip
	cp target/armv7-unknown-linux-musleabihf/debian/*.deb ./dist

package-armv5te-unknown-linux-musleabi: package-multitech-conduit package-multitech-conduit-ap

package-mips-unknown-linux-musl: package-dragino

package-mipsel-unknown-linux-musl: package-rak-ramips-24kec

# Gateway specific packaging.
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

package-multitech-conduit-ap3:
	cd packaging/vendor/multitech/conduit_ap3 && ./package.sh
	mkdir -p dist/vendor/multitech/conduit_ap3
	cp packaging/vendor/multitech/conduit_ap3/*.ipk dist/vendor/multitech/conduit_ap3

package-rak-ramips-24kec:
	cd packaging/vendor/rak/ramips_24kec && ./package.sh
	mkdir -p dist/vendor/rak/ramips_24kec
	cp packaging/vendor/rak/ramips_24kec/*.ipk dist/vendor/rak/ramips_24kec

package-tektelic-kona:
	cd packaging/vendor/tektelic/kona && ./package.sh
	mkdir -p dist/vendor/tektelic/kona
	cp packaging/vendor/tektelic/kona/*.ipk dist/vendor/tektelic/kona

package-kerlink-klkgw:
	cd packaging/vendor/kerlink/klkgw && ./package.sh
	mkdir -p dist/vendor/kerlink/klkgw
	cp packaging/vendor/kerlink/klkgw/*.ipk dist/vendor/kerlink/klkgw

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
