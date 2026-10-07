#!/usr/bin/env python3
"""Copy a WASM module, removing only the optional function-name custom section."""
from pathlib import Path
import sys

source, target = map(Path, sys.argv[1:])
data = source.read_bytes()
if data[:8] != b"\0asm\x01\0\0\0":
    raise ValueError("expected a WASM 1 module")

def uleb(offset):
    value = shift = 0
    while True:
        byte = data[offset]
        offset += 1
        value |= (byte & 127) << shift
        if byte < 128:
            return value, offset
        shift += 7
        if shift >= 35:
            raise ValueError("invalid section length")

out = bytearray(data[:8])
offset = 8
while offset < len(data):
    start = offset
    kind = data[offset]
    length, payload = uleb(offset + 1)
    end = payload + length
    if end > len(data):
        raise ValueError("truncated section")
    name = b""
    if kind == 0:
        n, text = uleb(payload)
        name = data[text:text + n]
    if kind != 0 or name != b"name":
        out.extend(data[start:end])
    offset = end
target.write_bytes(out)
print(f"{len(data)} -> {len(out)} bytes; removed {len(data) - len(out)} bytes")
