{
  description = "SaavyLab Fabric development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix.url = "github:nix-community/fenix";
  };

  outputs =
    {
      nixpkgs,
      fenix,
      ...
    }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      rustToolchain = fenix.packages.${system}.combine [
        fenix.packages.${system}.complete.toolchain
        fenix.packages.${system}.targets.wasm32-unknown-unknown.latest.rust-std
      ];
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        packages = [
          rustToolchain
          pkgs.cargo-nextest
          pkgs.go-task
          pkgs.mold
          pkgs.pkg-config
        ];

        RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
      };
    };
}