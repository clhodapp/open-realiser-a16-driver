# SPDX-License-Identifier: MIT
{

  description = "Open Realiser A16 userspace USB audio driver and surround upmixer";

  inputs = {
    caisson.url = "github:nix-caisson/caisson";
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # The library is built from the nixpkgs.lib mirror, and the
    # package sets from nixpkgs.
    nixpkgs-lib.url = "github:nix-community/nixpkgs.lib";
    flake-parts.url = "github:hercules-ci/flake-parts";
  };

  outputs =
    inputs@{ caisson, ... }:
    let
      lib = caisson.lib.caisson.mkLib {
        inherit (caisson.lib.caisson.pins.flake inputs) dependencies root;
        name = "open-realiser-a16-driver";
        systems = [
          "x86_64-linux"
          "aarch64-linux"
        ];
        projects = {
          inherit caisson;
        };

        configs = lib: lib.caisson.mkModules ./configs;

        pkgSets = lib: lib.caisson.nixpkgs.mkConfigurations { };

        libOverlays = lib: lib.caisson.mkLibOverlays ./lib-overlays;
        pkgOverlays = lib: lib.caisson.mkPkgOverlays ./pkg-overlays;
      };
    in
    lib.caisson.flake-parts.mkTopConfiguration { };

}
