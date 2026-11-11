{
  description = "Ozzy A16 userspace USB validation harness";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      each = nixpkgs.lib.genAttrs systems;
      package = system:
        let pkgs = nixpkgs.legacyPackages.${system}; in
        pkgs.stdenv.mkDerivation {
          pname = "ozzy-libusb";
          version = "0.1.0";
          src = pkgs.lib.cleanSourceWith {
            src = self;
            filter = path: type:
              let name = baseNameOf path; in
              !(pkgs.lib.hasPrefix "build-userspace" name) && name != "result"
              && pkgs.lib.cleanSourceFilter path type;
          };
          cmakeDir = "../tools/libusb-harness";
          nativeBuildInputs = [ pkgs.cmake pkgs.pkg-config ];
          buildInputs = [ pkgs.libusb1 ];
          doCheck = true;
          meta.license = pkgs.lib.licenses.mit;
          meta.mainProgram = "ozzy-libusb";
        };
    in {
      packages = each (system: { default = package system; });
      devShells = each (system:
        let pkgs = nixpkgs.legacyPackages.${system}; in {
          default = pkgs.mkShell {
            packages = [ pkgs.cmake pkgs.pkg-config pkgs.libusb1 pkgs.gcc pkgs.gnumake ];
          };
        });
    };
}
