#!/usr/bin/env bash
# Quick local demo without Docker: control plane + echo upstream + proxy + storm.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
cargo build -q --workspace
echo "Binaries ready: target/debug/{control-plane,edge-agent,edge-proxy,storm}"
echo "See docs/interview-demo.md for the 5-minute walkthrough."
