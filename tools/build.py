#!/usr/bin/env python3
"""Terra-owned build and package pipeline. The OS only provides a generic SDK."""
import argparse
import hashlib
import json
import os
import pathlib
import re
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


def engine_rustflags(sdk, shared_sdk, prefix):
    """Use the shared SDK as the sole final linker, preserving source remapping."""
    if shared_sdk is None:
        raise ValueError('A shared C/C++ SDK is required')
    compiler = shared_sdk / 'bin/mochios-c++'
    if not compiler.is_file():
        raise ValueError('A real shared C/C++ SDK is required for the engine build')
    library = prefix / 'usr/lib/libwebkit.so'
    elf(library, shared=True)
    flags = []; source = iter(sdk['rustflags'])
    for flag in source:
        if flag == '-C':
            value = next(source)
            if value.startswith(('linker=', 'link-arg=')):
                continue
            flags += [flag, value]
        elif not flag.startswith(('-Clinker=', '-Clink-arg=')):
            flags.append(flag)
    flags += ['-C', 'target-feature=-crt-static', '-C', 'linker=' + str(compiler), '-C', 'link-arg=-L' + str(prefix / 'usr/lib'),
              '-C', 'link-arg=-Wl,-rpath-link,' + str(prefix / 'usr/lib')]
    return flags


def engine_dependency(path):
    """Read explicit package identity; never invent a publisher or signing key."""
    data = tomllib.loads(path.read_text())
    fields = {'package', 'developer', 'subject_key_id', 'provenance', 'abi'}
    if set(data) != fields or any(not isinstance(v, str) or not v for v in data.values()):
        raise ValueError('Engine dependency requires the five native library identity fields')
    if not re.fullmatch('[0-9a-fA-F]{64}', data['subject_key_id']):
        raise ValueError('Engine dependency subject_key_id must be 64 hex digits')
    if data['provenance'] not in {'built-in', 'verified-package', 'development'}:
        raise ValueError('Invalid engine dependency provenance')
    return data


def has_engine_dependency(binary):
    """Inspect DT_NEEDED without executing the native program."""
    dynamic = subprocess.run(['readelf', '-d', str(binary)], check=True,
                             text=True, capture_output=True).stdout
    return bool(re.search(r'\(NEEDED\).*\[libwebkit\.so\.1\]', dynamic))


def build(sdk_file, output, shared_sdk=None, webkit_prefix=None, *, engine_probe=False):
    """Compile against SDK metadata, without invoking the OS build system."""
    if engine_probe and webkit_prefix is None:
        raise ValueError('The native integration probe requires WebKit')
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
    flags = list(engine_rustflags(sdk, shared_sdk, webkit_prefix) if webkit_prefix else sdk['rustflags'])
    # Panic locations retain registry paths even when debug sections are stripped.
    # Resolve Cargo's actual cache independently of the caller's SDK location.
    cargo_home = pathlib.Path(os.environ.get('CARGO_HOME', str(pathlib.Path.home() / '.cargo'))).expanduser().resolve()
    flags += ['--remap-path-prefix', str(cargo_home) + '=/build/cargo']
    env.update(RUSTC=sdk['rustc'], RUSTDOC=sdk['rustdoc'], CARGO_ENCODED_RUSTFLAGS='\x1f'.join(flags))
    env.pop('RUSTFLAGS', None)
    args = [sdk['cargo'], 'build', '--offline', '-Z', 'build-std=std,panic_abort,compiler_builtins', '-Z', 'json-target-spec']
    for config in sdk['cargo_config']:
        args += ['--config', config]
    args += ['--release', '--target', sdk['target'], '--target-dir', sdk['target_dir'], '--manifest-path', staging / 'Cargo.toml', '--bin', 'terra']
    if webkit_prefix:
        args += ['--features', 'webkit-engine']
    if engine_probe:
        args += ['--features', 'engine-probe']
    run(args, env=env, cwd=staging)
    binary = pathlib.Path(sdk['target_dir']) / pathlib.Path(sdk['target']).stem / 'release/terra'
    elf(binary)
    return binary


def stage(binary, output, engine=None, *, engine_linked=False, dependency=None, font_licenses=None):
    """Produce real bundle/payload files and a digest-bound install manifest."""
    elf(binary)
    if engine is not None:
        raise ValueError('WebKit is an independent mochiOS/libs package; --engine-prefix bundling is no longer supported. Use --shell-only until the native engine adapter is connected.')
    linked = has_engine_dependency(binary)
    if linked != engine_linked:
        raise ValueError('ELF engine dependency does not match the requested shell/engine build mode')
    if engine_linked and dependency is None:
        raise ValueError('A reviewed native engine package identity is required')
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
    if font_licenses is not None:
        if not font_licenses.is_dir():
            raise ValueError('Font license directory is missing')
        for source in sorted(font_licenses.rglob('*')):
            if source.is_file():
                target = bundle / 'licenses/fonts' / source.relative_to(font_licenses)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, target)
                target.chmod(0o644)
                inputs.append(('$/'+target.relative_to(bundle).as_posix(), target, '0644'))
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
    requirements = list(config['binary'][0]['requires'])
    if engine_linked:
        # Parent ceiling only: the signed helpers still declare their own subset.
        for capability in ['process.spawn', 'system.random.read', 'system.time.read',
                           'fs.read.tmp', 'fs.write.tmp', 'net.connect']:
            if capability not in requirements:
                requirements.append(capability)
    text += '\n[[binary]]\npath = "/applications/Terra.app/entry.elf"\nfile = "f0"\nkind = "application"\nrequires = ' + json.dumps(requirements) + '\n'
    if engine_linked:
        text += 'executable_dependencies = [\"webkit-web-process\", \"webkit-network-process\"]\n'
        for alias, export in [('webkit-web-process', 'web-process'), ('webkit-network-process', 'network-process')]:
            text += '\n[[executable_dependency]]\nalias = ' + json.dumps(alias) + '\nexport = ' + json.dumps(export) + '\n'
            for key in ['package', 'developer', 'subject_key_id', 'provenance', 'abi']:
                text += key + ' = ' + json.dumps(dependency[key]) + '\n'
    if engine_linked:
        text += '\n[[library_dependency]]\n'
        for key in ['package', 'developer', 'subject_key_id', 'provenance', 'abi']:
            text += key + ' = ' + json.dumps(dependency[key]) + '\n'
    (temporary / 'manifest.toml').write_text(text)
    # Keep a named application artifact as well as the standard MPKG payload.
    shutil.copytree(bundle, temporary / 'Terra.app')
    (temporary / 'Terra.app/manifest.toml').write_text(text)
    report = dict(format=1, shell_only=not engine_linked, engine_adapter_connected=False, engine_adapter_linked=engine_linked, runtime_tested=False,
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
    mode.add_argument('--webkit-prefix', type=pathlib.Path, help='Compile/link against a separately installed native WebKit prefix; never bundle it')
    parser.add_argument('--engine-probe', action='store_true', help='Build an opt-in native integration fixture, never a distributable app')
    parser.add_argument('--shared-sdk', type=pathlib.Path, help='Shared C/C++ SDK used for final linking')
    parser.add_argument('--engine-dependency', type=pathlib.Path, help='Override the built-in System WebKit dependency identity')
    parser.add_argument('--mpack', type=pathlib.Path)
    parser.add_argument('--msign', type=pathlib.Path)
    parser.add_argument('--identity')
    parser.add_argument('--font-licenses', type=pathlib.Path, help='Include the original SDK font licenses in the bundle')
    args = parser.parse_args()
    if args.engine_probe and (args.command != 'build' or not args.webkit_prefix or args.binary is not None):
        parser.error('--engine-probe requires build --webkit-prefix and cannot package or reuse a binary')
    if args.binary is None and args.sdk is None:
        parser.error('--sdk is required to compile; --binary accepts an already-built ELF')
    if args.command == 'package' and args.mpack is None:
        parser.error('--mpack is required for package generation')
    if args.webkit_prefix and args.shared_sdk is None:
        parser.error('--webkit-prefix requires --shared-sdk')
    if args.webkit_prefix and args.engine_dependency is None:
        args.engine_dependency = ROOT / 'resources/webkit-dependency.toml'
    args.output = args.output.resolve(); args.output.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve() if args.binary else build(args.sdk.resolve(), args.output, args.shared_sdk.resolve() if args.shared_sdk else None, args.webkit_prefix.resolve() if args.webkit_prefix else None, engine_probe=args.engine_probe)
    if args.engine_probe:
        destination = args.output / 'native-probe'
        destination.mkdir(exist_ok=True)
        shutil.copy2(binary, destination / 'entry.elf')
        print(destination / 'entry.elf')
        return
    product = stage(binary, args.output, args.engine_prefix.resolve() if args.engine_prefix else None, engine_linked=bool(args.webkit_prefix), dependency=engine_dependency(args.engine_dependency) if args.engine_dependency else None, font_licenses=args.font_licenses)
    if args.command == 'package':
        package(product, args.output, args.mpack.resolve(), args.msign.resolve() if args.msign else None, args.identity)
    print(product)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError) as error:
        print(f'Terra build failed: {error}', file=sys.stderr)
        raise SystemExit(1)
