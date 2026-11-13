# SPDX-License-Identifier: MIT
#
# The default package overlay: the driver under
# `pkgs.open-realiser-a16-driver` (packages/default.nix lists it). A
# consumer that lists this flake in `projects` holds it as
# `open-realiser-a16-driver/default`, which its package sets apply by
# default.
{ closure-lib, ... }:
{
  overlay = closure-lib.caisson.nixpkgs.mkPackagesOverlay (
    { callPackage, ... }: import ./packages { inherit callPackage; }
  ) "open-realiser-a16-driver";
}
