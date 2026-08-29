{
  description = "hb development environment";

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
      format = pkgs.writeShellApplication {
        name = "hb-format";
        runtimeInputs = [ rustToolchain ];
        text = ''
          cargo fmt --manifest-path hb-auth/Cargo.toml --all
          cargo fmt --manifest-path hb-d1c/Cargo.toml --all
        '';
      };
      formatCheck = pkgs.writeShellApplication {
        name = "hb-format-check";
        runtimeInputs = [ rustToolchain ];
        text = ''
          cargo fmt --manifest-path hb-auth/Cargo.toml --all -- --check
          cargo fmt --manifest-path hb-d1c/Cargo.toml --all -- --check
        '';
      };
      clippy = pkgs.writeShellApplication {
        name = "hb-clippy";
        runtimeInputs = [ rustToolchain ];
        text = ''
          cargo clippy --manifest-path hb-auth/Cargo.toml --all-targets -- -D warnings
          cargo clippy --manifest-path hb-d1c/Cargo.toml --all-targets -- -D warnings
        '';
      };
      test = pkgs.writeShellApplication {
        name = "hb-test";
        runtimeInputs = [ rustToolchain ];
        text = ''
          cargo test --manifest-path hb-auth/Cargo.toml
          cargo test --manifest-path hb-d1c/Cargo.toml
        '';
      };
      packageD1c = pkgs.writeShellApplication {
        name = "hb-package-d1c";
        runtimeInputs = [
          rustToolchain
          pkgs.gnused
        ];
        text = ''
          cargo package --manifest-path hb-d1c/Cargo.toml --allow-dirty
          version="$(cargo pkgid --manifest-path hb-d1c/Cargo.toml | sed 's/.*[#@]//')"
          cargo test --manifest-path "hb-d1c/target/package/hb-d1c-$version/Cargo.toml"
        '';
      };
      ci = pkgs.writeShellApplication {
        name = "hb-ci";
        text = ''
          ${formatCheck}/bin/hb-format-check
          ${clippy}/bin/hb-clippy
          ${test}/bin/hb-test
          ${packageD1c}/bin/hb-package-d1c
        '';
      };
      mkApp = package: name: description: {
        type = "app";
        program = "${package}/bin/${name}";
        meta = { inherit description; };
      };
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

      formatter.${system} = format;

      apps.${system} = {
        fmt-check = mkApp formatCheck "hb-format-check" "Check Rust formatting";
        clippy = mkApp clippy "hb-clippy" "Run strict Clippy";
        test = mkApp test "hb-test" "Run all Rust tests";
        package-d1c = mkApp packageD1c "hb-package-d1c" "Test the hb-d1c publish archive";
        ci = mkApp ci "hb-ci" "Run the complete local Rust QA gate";
      };
    };
}