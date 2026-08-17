{
  description = "Fast local-first terminal outliner";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      cargoManifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);

      mkVrac =
        pkgs:
        pkgs.rustPlatform.buildRustPackage {
          pname = "vrac";
          inherit (cargoManifest.workspace.package) version;

          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          LIBCLANG_PATH = nixpkgs.lib.makeLibraryPath [ pkgs.llvmPackages.libclang.lib ];
          nativeBuildInputs = [ pkgs.pkg-config ];
          cargoBuildFlags = [
            "--package"
            "vrac"
          ];
          cargoTestFlags = [ "--workspace" ];

          meta = {
            description = "Fast local-first terminal outliner";
            homepage = "https://github.com/yan-imensar/vrac";
            license = nixpkgs.lib.licenses.agpl3Only;
            mainProgram = "vrac";
            platforms = nixpkgs.lib.platforms.unix;
          };
        };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        rec {
          vrac = mkVrac pkgs;
          default = vrac;
        }
      );

      checks = forAllSystems (system: {
        inherit (self.packages.${system}) vrac;
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ self.packages.${system}.vrac ];
            packages = with pkgs; [
              cargo
              clippy
              rust-analyzer
              rustc
              rustfmt
            ];
            LIBCLANG_PATH = nixpkgs.lib.makeLibraryPath [ pkgs.llvmPackages.libclang.lib ];
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
          };
        }
      );

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
