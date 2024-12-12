{ pkgs ? import (fetchTarball "https://github.com/NixOS/nixpkgs/archive/nixos-24.11.tar.gz") {} }:

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
    pkgs.cargo-cross
    pkgs.cargo-deb
  ];
  LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
  BINDGEN_EXTRA_CLANG_ARGS = "-I${pkgs.llvmPackages.libclang.lib}/lib/clang/${pkgs.llvmPackages.libclang.version}/include";
  DOCKER_BUILDKIT = "1";
  NIX_STORE = "/nix/store";
}
