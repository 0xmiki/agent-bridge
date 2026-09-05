{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  packages = with pkgs; [
    rustc
    cargo
    rustfmt
    clippy
    rust-analyzer
    gcc
    pkg-config
  ];

  RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
}
