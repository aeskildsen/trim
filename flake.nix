{
  # Tailor created the first version of the nix/ fragments, but this flake does
  # not use them (loader.nix, hooks.nix): trim has no Tailor language enabled,
  # and the GPU driver hook has no use here. Tailor writes nix/ again on each
  # `tailor alter`, so .gitignore ignores it.
  description = "trim: find local git branches that are already merged, also by rebase or squash";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nix-packages.url = "github:wimpysworld/nix-packages";
    nix-packages.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      self,
      nixpkgs,
      nix-packages,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f system nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (
        _system: pkgs: {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "trim";
            version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
            src = self;
            cargoLock.lockFile = ./Cargo.lock;
            # The integration tests make scratch repositories with the git CLI.
            nativeCheckInputs = [ pkgs.git ];
          };
        }
      );

      devShells = forAll (
        system: pkgs:
        let
          # Keep the development shell available when Tailor has no package for this system.
          tailorPkgs = nix-packages.packages.${system} or { };
        in
        {
          default = pkgs.mkShell {
            packages =
              with pkgs;
              [
                cargo
                rustc
                clippy
                rustfmt
                rust-analyzer
                git
                actionlint
                gh
                just
                # bench/run.sh
                hyperfine
              ]
              ++ (if tailorPkgs ? tailor then [ tailorPkgs.tailor ] else [ ]);
          };
        }
      );
    };
}
