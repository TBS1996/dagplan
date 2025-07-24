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

        nativeBuildInputs = [pkgs.pkg-config];

        buildInputs = with pkgs; [
          libnotify
          glib
          gdk-pixbuf
        ];
      };

      devShells.default = pkgs.mkShell {
        buildInputs = with pkgs; [
          rustc
          cargo
          rust-analyzer
          pkg-config
          glib
          libnotify
          gdk-pixbuf
        ];

        RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
      };

      PKG_CONFIG_PATH = pkgs.lib.makeLibraryPath [
        pkgs.glib
        pkgs.libnotify
      ];
    });
}
