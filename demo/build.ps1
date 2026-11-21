# Builds the scheduler for the browser and puts it in demo/pkg. Run from the repository root:
#
#   ./demo/build.ps1
#
# Needs, once:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version 0.2.130 --locked
#
# The wasm-bindgen-cli version must be the wasm-bindgen version in Cargo.lock; a test in
# crates/maat-wasm fails if this file falls behind.
$ErrorActionPreference = "Stop"
cargo build -p maat-wasm --release --target wasm32-unknown-unknown
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
wasm-bindgen --target web --out-dir demo/pkg target/wasm32-unknown-unknown/release/maat_wasm.wasm
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Write-Host "Built demo/pkg. Serve the demo folder and open it, for example:"
Write-Host "  python -m http.server 8000 --directory demo"
Write-Host "then http://localhost:8000"
