{
  description = "Dagplan";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {inherit system;};
    in {
      packages.default = pkgs.rustPlatform.buildRustPackage {
        pname = "dagplan";
        version = "0.1.0";
        src = ./.;

        cargoLock = {
          lockFile = ./Cargo.lock;
        };

        doCheck = false;
      };

      # Optional: `nix develop` shell
      devShells.default = pkgs.mkShell {
        buildInputs = with pkgs; [rustc cargo];
      };
    });
}
