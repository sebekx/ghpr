#!/usr/bin/env bash
set -euo pipefail

echo "🔨 Building ghpr..."

# Check for local Rust toolchain first (preferred for macOS)
if command -v cargo &>/dev/null; then
    echo "  Using local Rust toolchain"
    cargo build --release
    echo ""
    echo "✅ Binary ready at: ./target/release/ghpr"
    echo "Run with:"
    echo "  GITHUB_TOKEN=\$(gh auth token) ./target/release/ghpr"
    exit 0
fi

# Fall back to Docker
if command -v docker &>/dev/null; then
    ARCH=$(uname -m)
    OS=$(uname -s)

    if [[ "$OS" == "Darwin" ]]; then
        echo "  ⚠️  Docker builds produce Linux binaries, not macOS."
        echo "  Install Rust for native macOS build: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        echo "  Building Linux binary anyway..."
    fi

    mkdir -p dist
    docker build --target export --output type=local,dest=./dist .
    chmod +x dist/ghpr
    echo ""
    echo "✅ Linux binary ready at: ./dist/ghpr"
else
    echo "❌ Neither cargo nor docker found."
    echo "Install Rust: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
fi
