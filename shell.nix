{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  buildInputs = with pkgs; [
    rustc
    cargo
    rust-analyzer
    clippy
    rustfmt
    pkg-config
    openssl

    # For LLM CLI backends
    pkgs.nodejs_22
    pkgs.nodePackages.npm
  ];

  shellHook = ''
    export RUST_BACKTRACE=1
    export NPM_CONFIG_PREFIX=$HOME/.npm-global
    export PATH=$HOME/.local/bin:$NPM_CONFIG_PREFIX/bin:$PATH
    export LD_LIBRARY_PATH=${pkgs.openssl.out}/lib:$LD_LIBRARY_PATH

    # Allow claude CLI to run inside Claude Code session
    unset CLAUDECODE

    mkdir -p $NPM_CONFIG_PREFIX

    # Install codex if not present
    if ! command -v codex &> /dev/null; then
      echo "Installing Codex CLI..."
      npm i -g @openai/codex
    fi

    # Install gemini CLI if not present
    if ! npm list -g @google/gemini-cli &> /dev/null; then
      echo "Installing Gemini CLI..."
      npm i -g @google/gemini-cli
    fi

    echo ""
    echo "finna dev shell"
    echo "  rustc: $(rustc --version)"
    echo "  cargo: $(cargo --version)"
    echo ""
    echo "Commands:"
    echo "  cargo build    - Build the project"
    echo "  cargo run      - Run finna"
    echo "  cargo clippy   - Lint"
    echo ""
    echo "LLM CLIs available:"
    if command -v claude &> /dev/null; then
      echo "  ✓ claude"
    else
      echo "  ✗ claude (not found in PATH)"
    fi
    if command -v codex &> /dev/null; then
      echo "  ✓ codex"
    else
      echo "  ✗ codex"
    fi
    if command -v npx &> /dev/null; then
      echo "  ✓ gemini (via npx @google/gemini-cli)"
    else
      echo "  ✗ gemini"
    fi
    echo ""
  '';
}
