# SPDX-License-Identifier: MIT
{ ... }:
{ ... }:
{

  debug = false;
  caisson = {
    libOverlays.exported = libOverlays: { inherit (libOverlays) default; };
  };

  # The driver's package is the `default` entry of this flake's package
  # overlay registry (pkg-overlays/, registered on mkLib): the package set
  # applies it by default, and the flake exports it as the plain
  # `overlays.default`. The export adds a check that builds each exported
  # package; building the driver runs its CTest suite and the shared
  # engine's host tests, offline and without an A16.
  caisson.nixpkgs = {
    packages.export.enabled = true;
  };

  perSystem =
    { config, pkgs, ... }:
    let
      driver = config.packages.open-realiser-a16-driver;
    in
    {
      # `nix build github:clhodapp/open-realiser-a16-driver` and `nix run`
      # resolve through packages.default.
      packages.default = driver;
      apps.upmix = {
        type = "app";
        program = "${driver}/bin/a16-upmix";
      };
      devShells.default = pkgs.mkShell {
        packages = [
          pkgs.cmake
          pkgs.pkg-config
          pkgs.libusb1
          pkgs.libsamplerate
          pkgs.pipewire
          pkgs.python3
          pkgs.gcc
          pkgs.gnumake
        ];
      };
    };

}
