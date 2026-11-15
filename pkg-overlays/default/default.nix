# SPDX-License-Identifier: MIT
#
# The default package overlay: the driver under
# `pkgs.open-realiser-a16-driver` (packages/default.nix lists it). A
# consumer that lists this flake in `projects` holds it as
# `open-realiser-a16-driver/default`, which its package sets apply by
# default.
#
# The overlay imports `cargo-purifier/default`, the package overlay of
# cargo-purifier, so every package set that applies this overlay also has
# `pkgs.cargo-purifier`, whose builder the driver's package is built with.
{ closure-lib, ... }:
{
  imports = [ closure-lib.caisson.libManifest.pkgOverlays."cargo-purifier/default" ];
  overlay = closure-lib.caisson.nixpkgs.mkPackagesOverlay (
    { callPackage, ... }: import ./packages { inherit callPackage; }
  ) "open-realiser-a16-driver";
}
