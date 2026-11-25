# SPDX-License-Identifier: MIT
#
# The driver: the binaries of the Cargo workspace, built with the
# dependencies that `cargo-purifier lock` recorded, and the launchers and
# configuration of the upmixer. Building it runs the tests of the
# workspace, which include the Python checks of tools/, offline and
# without an A16 or a PipeWire server.
{
  bash,
  cargo-purifier,
  lib,
  makeWrapper,
  pkg-config,
  python3,
  rustPlatform,
  libusb1,
  libsamplerate,
  systemdLibs,
  pipewire,
}:
let
  # The flake's source, a path already in the store, so the builder can
  # read the lockfiles while evaluating without anything being built or
  # added to the store. srcFilter picks the files the builds see.
  root = ../../../..;
  sources = [
    "Cargo.toml"
    "Cargo.lock"
    "cargo-purifier.lock.json"
    "crates"
    "tools"
  ];
  project = cargo-purifier.buildCargoProject {
    src = root;
    srcFilter =
      path: type:
      let
        relative = lib.removePrefix "${toString root}/" (toString path);
        name = baseNameOf path;
      in
      lib.elem (builtins.head (lib.splitString "/" relative)) sources
      && !(type == "directory" && name == "target")
      && !(lib.hasPrefix "result" name);
  };
  # pipewire-rs generates its bindings with bindgen; the crate links
  # libsamplerate and libsystemd through pkg-config.
  native = [
    pkg-config
    rustPlatform.bindgenHook
  ];
  libraries = [
    libusb1
    libsamplerate
    systemdLibs
    pipewire
  ];
  tests = project.tests.overrideAttrs (old: {
    nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ native ++ [ python3 ];
    buildInputs = (old.buildInputs or [ ]) ++ libraries;
  });
in
project.packages.open-realiser-a16-driver.overrideAttrs (old: {
  nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ native ++ [ makeWrapper ];
  # bash for the shebang of a16-upmix.
  buildInputs = (old.buildInputs or [ ]) ++ libraries ++ [ bash ];
  # The package builds only once the tests have passed.
  workspaceTests = tests;
  postInstall = (old.postInstall or "") + ''
    install -Dm755 tools/pipewire-upmix/a16-upmix tools/pipewire-upmix/a16-play-static -t $out/bin
    install -Dm644 tools/pipewire-upmix/a16-upmix.conf -t $out/share/a16-userspace-driver
    wrapProgram $out/bin/a16-upmix \
      --prefix PATH : ${lib.makeBinPath [ pipewire ]}
    wrapProgram $out/bin/a16-play-static \
      --prefix PATH : ${
        lib.makeBinPath [
          pipewire
          python3
        ]
      }
  '';
  passthru = (old.passthru or { }) // {
    inherit tests;
  };
  meta = (old.meta or { }) // {
    description = "Userspace USB audio driver and upmixer for the Smyth Realiser A16";
    platforms = lib.platforms.linux;
    license = lib.licenses.mit;
    mainProgram = "a16-driver";
  };
})
