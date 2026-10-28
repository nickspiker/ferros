#!/bin/bash
set -e

cd "$(dirname "$0")"
echo "Building ferros-attest-mark (development profile)..."
cargo build
echo ""
echo "Run: target/debug/ferros-attest-mark"
