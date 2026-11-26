{
  description = "trim: find local git branches that are already merged, also by rebase or squash";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "trim";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          # By default, a hook of the Rust builder sets LIBGIT2_NO_VENDOR=1 and
          # libgit2-sys then needs the libgit2 from nixpkgs. Turn off the hook,
          # so the package uses the same bundled libgit2 as `cargo build`. The
          # build then does not depend on the libgit2 version in nixpkgs.
          dontCargoUseSystemLibs = true;
          # The integration tests make scratch repositories with the git CLI.
          nativeCheckInputs = [ pkgs.git ];
        };
      });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            git
            # bench/run.sh
            hyperfine
          ];
        };
      });
    };
}
