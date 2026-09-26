#!/bin/bash
# Cross-builds dist/Shotlate.exe (x64) on macOS with the mingw toolchain, for trying a build on a Windows
# machine or VM. Release builds come from CI (MSVC, x64 and ARM64); see .github/workflows/release.yml.
#   brew install mingw-w64 && rustup target add x86_64-pc-windows-gnu
set -euo pipefail
cd "$(dirname "$0")/.."
command -v x86_64-w64-mingw32-gcc >/dev/null || { echo "需要 mingw-w64：brew install mingw-w64" >&2; exit 1; }
rustup target list --installed | grep -q x86_64-pc-windows-gnu || rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
mkdir -p dist
cp target/x86_64-pc-windows-gnu/release/shotlate.exe dist/Shotlate.exe
ls -la dist/Shotlate.exe
