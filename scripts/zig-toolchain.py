#!/usr/bin/env python3
"""Build tooling: checksum-verified Zig, confined to an explicit local cache."""
import hashlib
import json
import os
import pathlib
import platform
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
PIN = json.loads((ROOT / 'scripts/zig-toolchain.json').read_text())


def host():
    arch = {'AMD64': 'x86_64', 'arm64': 'aarch64'}.get(platform.machine(), platform.machine())
    system = {'Darwin': 'macos', 'Linux': 'linux', 'Windows': 'windows'}.get(platform.system())
    key = f'{arch}-{system}'
    if key not in PIN['hosts']:
        raise RuntimeError(f'no pinned Zig toolchain for {key}')
    return key


def verify(archive, checksum):
    with archive.open('rb') as stream:
        digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    if digest != checksum:
        raise RuntimeError(f'Zig SHA-256 mismatch: {archive}; remove the corrupt cache entry and retry')


def install(cache):
    cache = pathlib.Path(cache).resolve()
    cache.mkdir(parents=True, exist_ok=True)
    pin = PIN['hosts'][host()]
    archive = cache / pin['url'].rsplit('/', 1)[1]
    if not archive.exists():
        with tempfile.TemporaryDirectory(dir=cache) as staging:
            download = pathlib.Path(staging) / archive.name
            urllib.request.urlretrieve(pin['url'], download)
            verify(download, pin['sha256'])
            os.replace(download, archive)
    verify(archive, pin['sha256'])
    directory = cache / archive.name.removesuffix('.tar.xz').removesuffix('.zip')
    zig = directory / ('zig.exe' if os.name == 'nt' else 'zig')
    if not zig.exists():
        with tempfile.TemporaryDirectory(dir=cache) as staging:
            if archive.suffix == '.zip':
                with zipfile.ZipFile(archive) as bundle:
                    for member in bundle.namelist():
                        if not (pathlib.Path(staging) / member).resolve().is_relative_to(staging):
                            raise RuntimeError('unsafe path in Zig archive')
                    bundle.extractall(staging)
            else:
                with tarfile.open(archive) as bundle:
                    bundle.extractall(staging, filter='data')
            try:
                os.rename(pathlib.Path(staging) / directory.name, directory)
            except FileExistsError:
                pass  # Another builder installed the same verified archive.
    if subprocess.check_output([str(zig), 'version'], text=True).strip() != PIN['version']:
        raise RuntimeError(f'cached Zig version disagrees with pin: {zig}')
    return zig


def adapt(args):
    return ['--target=wasm32-freestanding' if a == '--target=wasm32-unknown-unknown' else a for a in args]


if __name__ == '__main__':
    try:
        if sys.argv[1] in ('cc', 'ar'):
            zig = os.environ['CQX_BUILD_ZIG']  # Tooling only; never scan configuration.
            result = subprocess.run([zig, sys.argv[1], *adapt(sys.argv[2:])])
            sys.exit(result.returncode)
        print(install(sys.argv[1]))
    except (RuntimeError, OSError, KeyError) as error:
        sys.exit(f'cqx Zig toolchain: {error}')
