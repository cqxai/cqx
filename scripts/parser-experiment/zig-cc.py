#!/usr/bin/env python3
"""cc-rs emits Rust's target spelling; translate it into Zig's spelling."""
import os
import sys

args = ["--target=wasm32-freestanding" if a == "--target=wasm32-unknown-unknown" else a for a in sys.argv[1:]]
os.execv(os.environ["CQX_EXPERIMENT_ZIG"], [os.environ["CQX_EXPERIMENT_ZIG"], "cc", *args])
