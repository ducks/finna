{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  buildInputs = with pkgs; [
    rustc
    cargo
    clippy
    rustfmt
  ];

  shellHook = ''
    echo "finna dev shell"
    echo "  cargo build    - Build"
    echo "  cargo run      - Run"
    echo "  cargo clippy   - Lint"
  '';
}
