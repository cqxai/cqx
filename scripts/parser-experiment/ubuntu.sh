#!/bin/bash
set -euo pipefail
# Run inside the isolated Ubuntu 24.04 container; everything lives in /work.
export PATH=/work/linux-bin:/work/linux-cargo/bin:/work/sccache-v0.18.0-x86_64-unknown-linux-musl:/usr/bin:/bin
export CARGO_HOME=/work/linux-cargo RUSTUP_HOME=/work/linux-rustup TMPDIR=/work/linux-tmp
export CARGO_TARGET_DIR=/work/linux-target RUSTC_WRAPPER=/work/sccache-v0.18.0-x86_64-unknown-linux-musl/sccache
export SCCACHE_DIR=/work/linux-sccache ZIG_GLOBAL_CACHE_DIR=/work/linux-zig-cache
export CQX_EXPERIMENT_ZIG=/work/zig-x86_64-linux-0.15.2/zig
cd /work/combined
cargo metadata --locked --format-version 1 > /work/linux-metadata.json
headers=$(python3 -c 'import json,pathlib;p=next(p for p in json.load(open("/work/linux-metadata.json"))["packages"] if p["name"]=="tree-sitter-language");print(pathlib.Path(p["manifest_path"]).parent/"wasm/include")')
export CC_wasm32_unknown_unknown=/work/linux-bin/wasm-cc AR_wasm32_unknown_unknown=/work/linux-bin/wasm-ar
export CFLAGS_wasm32_unknown_unknown="-I$headers -include wchar.h -Dstatic_assert=_Static_assert"
! command -v clang
! command -v llvm-ar
cargo build --locked --release --target wasm32-unknown-unknown -p cqx-wasm
mkdir -p /work/artifacts
cp /work/linux-target/wasm32-unknown-unknown/release/cqx_wasm.wasm /work/artifacts/ubuntu-zig.wasm
sccache --show-stats
