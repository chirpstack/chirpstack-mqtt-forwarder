{ pkgs ? import (fetchTarball "https://github.com/NixOS/nixpkgs/archive/nixos-25.05.tar.gz") {} }:

pkgs.mkShell {
  nativeBuildInputs = [
    pkgs.pkg-config
  ];
  buildInputs = [
    pkgs.cacert
    pkgs.rustup
    pkgs.protobuf
    pkgs.perl
    pkgs.cmake
    pkgs.clang
    pkgs.openssl
    pkgs.opkg-utils
    pkgs.jq
    # cargo-cross can be used once version > 0.2.5, as 0.2.5 does not work well
    # with nightly toolchain. It is for now installed through make dev-dependencies.
    # pkgs.cargo-cross
    pkgs.cargo-deb
  ];
  shellHook = ''
    export PATH=$PATH:~/.cargo/bin
  '';
  LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
  BINDGEN_EXTRA_CLANG_ARGS = "-I${pkgs.llvmPackages.libclang.lib}/lib/clang/${pkgs.llvmPackages.libclang.version}/include";
  DOCKER_BUILDKIT = "1";
  NIX_STORE = "/nix/store";
}
