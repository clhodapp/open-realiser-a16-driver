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
  # package; building the driver runs the tests of the Cargo workspace and
  # the Python checks of tools/, offline and without an A16.
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
      # cargo-purifier's cargo wrapper builds with the locked dependencies;
      # bindgen generates the PipeWire bindings.
      devShells.default = pkgs.mkShell {
        packages = [
          pkgs.cargo-purifier.cargo-purifier
          pkgs.cargo
          pkgs.rustc
          pkgs.clippy
          pkgs.rustfmt
          pkgs.pkg-config
          pkgs.rustPlatform.bindgenHook
          pkgs.python3
        ];
        buildInputs = [
          pkgs.libusb1
          pkgs.libsamplerate
          pkgs.systemdLibs
          pkgs.pipewire
        ];
      };
    };

}
