#!/usr/bin/env python3
"""Download only into this experiment's ignored directory, verifying pins."""
import hashlib
import pathlib
import sys
import tarfile
import urllib.request

ROOT=pathlib.Path(__file__).resolve().parents[2]
platform='macos' if sys.platform=='darwin' else 'linux'
hashes={'macos':'375b6909fc1495d16fc2c7db9538f707456bfc3373b14ee83fdd3e22b3d43f7f','linux':'02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239'}
archive=ROOT/f'.experiment/zig-{platform}.tar.xz'
archive.parent.mkdir(parents=True,exist_ok=True)
if not archive.exists():
    urllib.request.urlretrieve(f'https://ziglang.org/download/0.15.2/zig-x86_64-{platform}-0.15.2.tar.xz',archive)
assert hashlib.sha256(archive.read_bytes()).hexdigest()==hashes[platform], 'Zig checksum mismatch'
with tarfile.open(archive) as bundle:
    bundle.extractall(archive.parent)
print(f'Zig 0.15.2 {platform}: {hashes[platform]}')
