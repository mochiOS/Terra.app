#!/usr/bin/env python3
"""Terra-owned build and package pipeline. The OS only provides a generic SDK."""
import argparse
import hashlib
import json
import os
import pathlib
import shutil
import struct
import subprocess
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[1]


def run(args, **kwargs):
    """Run tools without shell interpolation and propagate their real failures."""
    subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def elf(path, shared=False):
    """Reject wrong architecture, non-ELF files and the wrong executable kind."""
    with path.open('rb') as stream:
        header = stream.read(64)
    if len(header) != 64 or header[:6] != b'\x7fELF\x02\x01' or struct.unpack_from('<H', header, 18)[0] != 62:
        raise ValueError(f'Not an x86-64 ELF: {path}')
    kind = struct.unpack_from('<H', header, 16)[0]
    if kind != (3 if shared else 2):
        raise ValueError(f'Unexpected ELF type {kind}: {path}')


def build(sdk_file, output):
    """Compile against SDK metadata, without invoking the OS build system."""
    sdk = json.loads(sdk_file.read_text())
    if sdk.get('format') != 1:
        raise ValueError('Unsupported SDK description')
    source = ROOT / 'Cargo.toml'
    manifest = source.read_text()
    import re
    manifest, count = re.subn(r'appcore = \{[^\n]+\}', 'appcore = { package = "mochios-appcore", path = ' + json.dumps(sdk['appcore']) + ' }', manifest)
    if count != 1:
        raise ValueError('Cannot locate AppCore dependency')
    staging = output / 'rust-project'
    staging.mkdir(parents=True, exist_ok=True)
    (staging / 'Cargo.toml').write_text(manifest)
    if not (staging / 'src').exists():
        (staging / 'src').symlink_to(ROOT / 'src', target_is_directory=True)
    # Retain the application's dependency resolution rather than inventing one.
    shutil.copy2(ROOT / 'Cargo.lock', staging / 'Cargo.lock')
    env = os.environ.copy()
    env.update(sdk.get('versions', {})); env.update(sdk['fonts'])
    env.update(RUSTC=sdk['rustc'], RUSTDOC=sdk['rustdoc'], CARGO_ENCODED_RUSTFLAGS='\x1f'.join(sdk['rustflags']))
    env.pop('RUSTFLAGS', None)
    args = [sdk['cargo'], 'build', '--offline', '-Z', 'build-std=std,panic_abort,compiler_builtins', '-Z', 'json-target-spec']
    for config in sdk['cargo_config']:
        args += ['--config', config]
    args += ['--release', '--target', sdk['target'], '--target-dir', sdk['target_dir'], '--manifest-path', staging / 'Cargo.toml', '--bin', 'terra']
    run(args, env=env, cwd=staging)
    binary = pathlib.Path(sdk['target_dir']) / pathlib.Path(sdk['target']).stem / 'release/terra'
    elf(binary)
    return binary


def stage(binary, output, engine=None):
    """Produce real bundle/payload files and a digest-bound install manifest."""
    elf(binary)
    if engine is not None:
        metadata = json.loads((engine / 'engine.json').read_text())
        if metadata.get('format') != 1 or metadata.get('target') != 'mochios-1' or not metadata.get('version'):
            raise ValueError('Engine metadata must identify an actual mochiOS build')
        elf(engine / 'libwebkit.so', shared=True)
        for directory, dirs, files in os.walk(engine, followlinks=False):
            for name in dirs + files:
                path = pathlib.Path(directory) / name
                if path.is_symlink() or not (path.is_dir() or path.is_file()):
                    raise ValueError(f'Engine input must be a regular file/directory: {path}')
    destination = output / 'product'
    temporary = output / 'product.new'
    if temporary.exists():
        shutil.rmtree(temporary)
    bundle = temporary / 'payload/bundle'
    bundle.mkdir(parents=True)
    shutil.copy2(binary, bundle / 'entry.elf')
    shutil.copy2(ROOT / 'appicon.png', bundle / 'appicon.png')
    (bundle / 'entry.elf').chmod(0o755)
    (bundle / 'appicon.png').chmod(0o644)
    inputs = [('$/entry.elf', bundle / 'entry.elf', '0755'), ('$/appicon.png', bundle / 'appicon.png', '0644')]
    if engine is not None:
        library = temporary / 'payload/root/libraries/libwebkit.so'
        library.parent.mkdir(parents=True)
        shutil.copy2(engine / 'libwebkit.so', library)
        library.chmod(0o644)
        inputs.append(('/libraries/libwebkit.so', library, '0644'))
        runtime = engine / 'runtime'
        if runtime.exists():
            for path in sorted(runtime.rglob('*')):
                if path.is_file():
                    with path.open('rb') as stream:
                        magic = stream.read(4)
                    if path.stat().st_mode & 0o111 or magic == b'\x7fELF':
                        raise ValueError(f'Runtime code requires explicit binary declarations; only resources are supported here: {path}')
                    relative = path.relative_to(runtime)
                    target = temporary / 'payload/root/libraries/webkit' / relative
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(path, target)
                    target.chmod(0o644)
                    inputs.append(('/libraries/webkit/' + relative.as_posix(), target, '0644'))
    config = tomllib.loads((ROOT / 'manifest.toml').read_text())
    package = config['package']; app = config['application']
    text = 'format = 1\n\n[package]\n'
    for key in ['id', 'name', 'version', 'vendor', 'kind', 'architecture', 'abi']:
        text += f'{key} = {json.dumps(package[key])}\n'
    text += f'revision = {package["revision"]}\n\n[application]\n'
    for key in ['entry', 'description', 'icon', 'resources']:
        text += f'{key} = {json.dumps(app[key])}\n'
    for index, (target, path, mode) in enumerate(inputs):
        text += f'\n[[file]]\nid = "f{index}"\npath = {json.dumps(target)}\ndigest = "sha256:{hashlib.sha256(path.read_bytes()).hexdigest()}"\nsize = {path.stat().st_size}\nmode = "{mode}"\n'
    text += '\n[[binary]]\npath = "/applications/Terra.app/entry.elf"\nfile = "f0"\nkind = "application"\nrequires = ' + json.dumps(config['binary'][0]['requires']) + '\n'
    if engine is not None:
        text += '\n[[binary]]\npath = "/libraries/libwebkit.so"\nfile = "f2"\nkind = "library"\nrequires = []\n'
    (temporary / 'manifest.toml').write_text(text)
    # Keep a named application artifact as well as the standard MPKG payload.
    shutil.copytree(bundle, temporary / 'Terra.app')
    (temporary / 'Terra.app/manifest.toml').write_text(text)
    report = dict(format=1, shell_only=engine is None, engine_adapter_connected=False,
                  files=[dict(path=target, sha256=hashlib.sha256(path.read_bytes()).hexdigest(), size=path.stat().st_size) for target, path, _ in inputs])
    (temporary / 'artifacts.json').write_text(json.dumps(report, indent=2) + '\n')
    if destination.exists():
        shutil.rmtree(destination)
    temporary.rename(destination)
    return destination


def package(product, output, mpack, msign=None, identity=None):
    """Build MPKG; signing requires an explicitly selected existing identity."""
    package_path = output / 'Terra.mpkg'
    run([mpack, 'create', '--manifest', product / 'manifest.toml', '--payload', product / 'payload', '--output', package_path, '--force'])
    if msign is not None:
        if not identity:
            raise ValueError('Choose --identity explicitly; no signing key will be generated')
        run([msign, 'package', 'sign-auto', package_path, '--identity', identity])
    print(package_path)
    return package_path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['build', 'package'])
    parser.add_argument('--sdk', type=pathlib.Path)
    parser.add_argument('--binary', type=pathlib.Path)
    parser.add_argument('--output', type=pathlib.Path, default=ROOT / 'out')
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--shell-only', action='store_true')
    mode.add_argument('--engine-prefix', type=pathlib.Path)
    parser.add_argument('--mpack', type=pathlib.Path)
    parser.add_argument('--msign', type=pathlib.Path)
    parser.add_argument('--identity')
    args = parser.parse_args()
    if args.binary is None and args.sdk is None:
        parser.error('--sdk is required to compile; --binary accepts an already-built ELF')
    if args.command == 'package' and args.mpack is None:
        parser.error('--mpack is required for package generation')
    args.output = args.output.resolve(); args.output.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve() if args.binary else build(args.sdk.resolve(), args.output)
    product = stage(binary, args.output, args.engine_prefix.resolve() if args.engine_prefix else None)
    if args.command == 'package':
        package(product, args.output, args.mpack.resolve(), args.msign.resolve() if args.msign else None, args.identity)
    print(product)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        print(f'Terra build failed: {error}', file=sys.stderr)
        raise SystemExit(1)
