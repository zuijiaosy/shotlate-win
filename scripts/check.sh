#!/bin/bash
# The standard checks after a change: tests, a Windows type-check, and the offscreen UI demo.
#   SHOTLATE_MODELS=<dir with the two .onnx files> scripts/check.sh   also runs the OCR accuracy tests
set -euo pipefail
cd "$(dirname "$0")/.."
cargo test --release
cargo check --release --target x86_64-pc-windows-gnu
out="${1:-target/ui-demo}"
cargo run --release -q -- --ui-demo "$out"
echo "UI demo frames: $out"
