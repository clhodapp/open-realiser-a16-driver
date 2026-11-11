{
  description = "Open Realiser A16 userspace USB audio driver and surround upmixer";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      each = nixpkgs.lib.genAttrs systems;
      package = system:
        let pkgs = nixpkgs.legacyPackages.${system}; in
        pkgs.stdenv.mkDerivation {
          pname = "open-realiser-a16-driver";
          version = "0.1.0";
          src = pkgs.lib.cleanSourceWith {
            src = self;
            filter = path: type:
              let name = baseNameOf path; in
              !(pkgs.lib.hasPrefix "build-userspace" name) && name != "result"
              && pkgs.lib.cleanSourceFilter path type;
          };
          nativeBuildInputs = [ pkgs.cmake pkgs.pkg-config pkgs.python3 pkgs.makeWrapper ];
          buildInputs = [ pkgs.libusb1 pkgs.libsamplerate pkgs.pipewire ];
          postInstall = ''
            wrapProgram $out/bin/a16-upmix \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.pipewire ]}
            wrapProgram $out/bin/a16-play-static \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.pipewire pkgs.python3 ]}
          '';
          doCheck = true;
          meta.description = "Userspace USB audio driver and upmixer for the Smyth Realiser A16";
          meta.platforms = pkgs.lib.platforms.linux;
          meta.license = pkgs.lib.licenses.mit;
          meta.mainProgram = "a16-driver";
        };
    in {
      packages = each (system:
        let driver = package system; in {
          default = driver;
          open-realiser-a16-driver = driver;
          a16-userspace-driver = driver;
        });
      checks = each (system: {
        driver = self.packages.${system}.default;
      });
      apps = each (system:
        let driver = package system; in {
          default = { type = "app"; program = "${driver}/bin/a16-driver"; };
          upmix = { type = "app"; program = "${driver}/bin/a16-upmix"; };
        });
      devShells = each (system:
        let pkgs = nixpkgs.legacyPackages.${system}; in {
          default = pkgs.mkShell {
            packages = [ pkgs.cmake pkgs.pkg-config pkgs.libusb1 pkgs.libsamplerate pkgs.pipewire pkgs.python3 pkgs.gcc pkgs.gnumake ];
          };
        });
    };
}
