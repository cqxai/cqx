#!/usr/bin/env bash
# Build tooling only: the scanner itself has no environment configuration.
set -euo pipefail
cd "$(dirname "$0")/.."
# Use LLVM clang, whose wasm32 backend is required by the grammar C sources.
if [[ "$(uname -s)" == Darwin ]]; then
  llvm_prefix="$(brew --prefix llvm)"
  export CC_wasm32_unknown_unknown="$llvm_prefix/bin/clang"
  export AR_wasm32_unknown_unknown="$llvm_prefix/bin/llvm-ar"
else
  export CC_wasm32_unknown_unknown=clang
  export AR_wasm32_unknown_unknown=llvm-ar
fi
# Resolve the pinned dependency rather than hard-coding Cargo's registry path.
wasm_headers="$(cargo metadata --locked --format-version 1 | python3 -c '
import json,sys,pathlib
p=next(p for p in json.load(sys.stdin)["packages"] if p["name"]=="tree-sitter-language")
print(pathlib.Path(p["manifest_path"]).parent/"wasm"/"include")
')"
# The C++ scanner expects wchar_t from wctype.h and the C11 static_assert alias.
# tree-sitter-language provides wchar.h; clang implements _Static_assert.
export CFLAGS_wasm32_unknown_unknown="-I\"$wasm_headers\" -include wchar.h -Dstatic_assert=_Static_assert"
export CC_SHELL_ESCAPED_FLAGS=1
cargo build --locked --release --target wasm32-unknown-unknown -p cqx-wasm
