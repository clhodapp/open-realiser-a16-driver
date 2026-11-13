# SPDX-License-Identifier: MIT
{
  lib,
  stdenv,
  cmake,
  pkg-config,
  python3,
  makeWrapper,
  libusb1,
  libsamplerate,
  pipewire,
}:
stdenv.mkDerivation {
  pname = "open-realiser-a16-driver";
  version = "0.1.0";
  # The repository root, without local CMake build trees and result links.
  src = lib.cleanSourceWith {
    src = ../../../..;
    filter =
      path: type:
      let
        name = baseNameOf path;
      in
      !(lib.hasPrefix "build-userspace" name)
      && !(lib.hasPrefix "result" name)
      && lib.cleanSourceFilter path type;
  };
  nativeBuildInputs = [
    cmake
    pkg-config
    python3
    makeWrapper
  ];
  buildInputs = [
    libusb1
    libsamplerate
    pipewire
  ];
  postInstall = ''
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
  doCheck = true;
  # The shared engine's host tests, beside the CTest suite of the build.
  postCheck = ''
    make -C ../tests -j"$NIX_BUILD_CORES" check
  '';
  meta = {
    description = "Userspace USB audio driver and upmixer for the Smyth Realiser A16";
    platforms = lib.platforms.linux;
    license = lib.licenses.mit;
    mainProgram = "a16-driver";
  };
}
