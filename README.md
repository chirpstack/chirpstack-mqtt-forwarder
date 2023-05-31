# ChirpStack MQTT Forwarder

![CI](https://github.com/chirpstack/chirpstack-mqtt-forwarder/actions/workflows/main.yml/badge.svg?branch=master)

ChirpStack MQTT Forwarder is a Semtech UDP and ChirpStack Concentratord to
MQTT forwarder. It is intended to run on the gateway and is a more lightweight
alternative to the ChirpStack Gateway Bridge such that it can run on gateways
with a limited amount of flash memory.

## Documentation and binaries

Please refer to the [ChirpStack](https://www.chirpstack.io/) website for
documentation and pre-compiled binaries.

## Building from source

### Requirements

Building ChirpStack MQTT Forwarder requires:

* [Nix](https://nixos.org/download.html) (recommended) and
* [Docker](https://www.docker.com/)

#### Nix

Nix is used for setting up the development environment which is used for local
development and for creating the binaries.

If you do not have Nix installed and do not wish to install it, then you can
use the provided Docker Compose based Nix environment. To start this environment
execute the following command:

```bash
make docker-devshell
```

**Note:** You will be able to run the test commands and run `cargo build`, but
cross-compiling will not work within this environment (because it would try start
Docker within Docker).

#### Docker

Docker is used by [cross-rs](https://github.com/cross-rs/cross) for cross-compiling,
as well as some of the `make` commands.

### Starting the development shell

Run the following command to start the development shell:

```bash
nix-shell
```

Or if you do not have Nix installed, execute the following command:

```bash
make docker-devshell
```

### Running tests

#### Start required services

ChirpStack MQTT Forwarder depends on a MQTT broker for running the tests.
You need to start this service manually if you started the development shell
using `nix-shell`:

```bash
docker-compose up -d
```

#### Run tests

Execute the following command to run the tests:

```bash
make test
```

### Building binaries

Execute the following commands to build the ChirpStack MQTT Forwarder binaries
and packages:

```bash
# Only build binaries
make build

# Build binaries + distributable packages.
make dist
```

## License

ChirpStack MQTT Forwarder is distributed under the MIT license. See also
[LICENSE](https://github.com/brocaar/chirpstack/blob/master/LICENSE).
