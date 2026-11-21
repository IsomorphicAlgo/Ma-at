#!/bin/sh
# Builds the scheduler for the browser and puts it in demo/pkg. Run from the repository root:
#
#   sh demo/build.sh
#
# Needs, once:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.130 --locked
#
# The wasm-bindgen-cli version must be the wasm-bindgen version in Cargo.lock; a test in
# crates/maat-wasm fails if this file falls behind.
set -eu
cargo build -p maat-wasm --release --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir demo/pkg target/wasm32-unknown-unknown/release/maat_wasm.wasm
echo "Built demo/pkg. Serve the demo folder and open it, for example:"
echo "  python3 -m http.server 8000 --directory demo"
echo "then http://localhost:8000"
