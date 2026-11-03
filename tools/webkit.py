#!/usr/bin/env python3
"""Fetch, diagnose and build Terra's pinned WPE sources without invoking mmake."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
VENDOR = ROOT / 'vendor'


def digest(path):
    """Hash a file without keeping a source archive in memory."""
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def lockfile():
    """Read the reviewed release pin, never resolve a floating latest version."""
    lock = json.loads((VENDOR / 'webkit.lock.json').read_text())
    if not isinstance(lock, dict) or lock['format'] != 1 or not lock['url'].startswith('https://'):
        raise ValueError('Unsupported source lock')
    if len(lock['sha256']) != 64 or any(c not in '0123456789abcdef' for c in lock['sha256']):
        raise ValueError('Invalid source digest')
    if PurePosixPath(lock['directory']).name != lock['directory']:
        raise ValueError('Invalid source directory')
    return lock


def write_json(path, value):
    """Publish metadata atomically after the operation actually succeeds."""
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + '.new')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    temporary.replace(path)


class HTTPSRedirects(urllib.request.HTTPRedirectHandler):
    """Reject insecure redirects before issuing the redirected request."""
    def redirect_request(self, request, fp, code, msg, headers, url):
        if not url.startswith('https://'):
            raise ValueError('Refusing an insecure download redirect')
        return super().redirect_request(request, fp, code, msg, headers, url)


def fetch(lock, cache, network=False):
    """Reuse a hash-verified archive; networking is opt-in and confined to fetch."""
    cache.mkdir(parents=True, exist_ok=True)
    archive = cache / (lock['directory'] + '.tar.xz')
    if archive.exists():
        if digest(archive) != lock['sha256']:
            raise ValueError(f'Cached archive hash mismatch: {archive}; remove it explicitly to retry')
        return archive
    if not network:
        raise ValueError('Source archive missing; run fetch --network once or populate the offline cache')
    # Exclusive temporary files keep failed/overlapping fetches from publishing partial sources.
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=cache, prefix='download-', delete=False) as output:
            temporary = Path(output.name)
            with urllib.request.build_opener(HTTPSRedirects()).open(lock['url'], timeout=60) as source:
                if not source.geturl().startswith('https://'):
                    raise ValueError('Refusing an insecure download redirect')
                total = 0
                while chunk := source.read(1024 * 1024):
                    total += len(chunk)
                    if total > 256 * 1024 ** 2:
                        raise ValueError('Source download exceeds the 256 MiB archive limit')
                    output.write(chunk)
        if digest(temporary) != lock['sha256']:
            raise ValueError('Downloaded archive hash mismatch')
        temporary.replace(archive)
        return archive
    finally:
        if temporary and temporary.exists():
            temporary.unlink()


def archive_members(tar, directory):
    """Reject unexpected roots, traversal, external links and device entries before extraction."""
    members = tar.getmembers()
    seen = {}
    unique = []
    for member in members:
        name = PurePosixPath(member.name)
        if name.is_absolute() or '..' in name.parts or not name.parts or name.parts[0] != directory:
            raise ValueError(f'Unsafe archive member: {member.name}')
        if not (member.isfile() or member.isdir() or member.issym() or member.islnk()):
            raise ValueError(f'Unsupported archive member: {member.name}')
        canonical = name.as_posix()
        if previous := seen.get(canonical):
            # The official release contains wpe-theme.css twice. Accept only an
            # identical regular file, never an order-dependent replacement/link.
            identical = (member.isfile() and previous.isfile()
                         and member.mode == previous.mode and member.size == previous.size)
            if identical:
                with tar.extractfile(previous) as first, tar.extractfile(member) as second:
                    identical = hashlib.file_digest(first, 'sha256').digest() == hashlib.file_digest(second, 'sha256').digest()
            if not identical:
                raise ValueError(f'Conflicting duplicate archive member: {member.name}')
            continue
        seen[canonical] = member
        unique.append(member)
        if member.issym() or member.islnk():
            link = PurePosixPath(member.linkname)
            if link.is_absolute():
                raise ValueError(f'External archive link: {member.name}')
            parts = list(name.parent.parts) if member.issym() else []
            for part in link.parts:
                if part == '..':
                    if len(parts) <= 1:
                        raise ValueError(f'External archive link: {member.name}')
                    parts.pop()
                elif part != '.':
                    parts.append(part)
            if not parts or parts[0] != directory:
                raise ValueError(f'External archive link: {member.name}')
    return unique


def prepare(lock, archive, source_root):
    """Extract once into an owned tree; never download, patch or overwrite another tree."""
    if digest(archive) != lock['sha256']:
        raise ValueError('Archive hash mismatch')
    destination = source_root / lock['directory']
    marker = destination / '.terra-source.json'
    if destination.exists():
        if marker.is_file() and json.loads(marker.read_text()) == lock:
            return destination
        raise ValueError(f'Untracked source tree: {destination}; refusing to replace it')
    source_root.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive) as tar:
        members = archive_members(tar, lock['directory'])
        # Extraction overhead and working headroom, not a promise of final build size.
        required = sum(m.size for m in members) + 1024 ** 3
        if shutil.disk_usage(source_root).free < required:
            raise ValueError(f'Insufficient source space: need {required} bytes including 1 GiB reserve')
        with tempfile.TemporaryDirectory(dir=source_root, prefix='extract-') as temporary:
            tar.extractall(temporary, members=members, filter='data')
            staged = Path(temporary) / lock['directory']
            write_json(staged / '.terra-source.json', lock)
            staged.rename(destination)
    return destination


def environment(target, prefix):
    """Isolate target pkg-config metadata; host tools remain host executables."""
    env = os.environ.copy()
    env['CARGO_NET_OFFLINE'] = 'true'
    if target == 'mochios':
        if prefix is None:
            raise ValueError('--deps-prefix must be an explicitly marked mochiOS dependency sysroot')
        marker = json.loads((prefix / 'target.json').read_text())
        if not isinstance(marker, dict) or marker.get('format') != 1 or marker.get('target') != 'mochios-1':
            raise ValueError('Dependency sysroot is not marked mochios-1')
        env['PKG_CONFIG_PATH'] = ''
        env['PKG_CONFIG_LIBDIR'] = os.pathsep.join(str(prefix / path) for path in
            ['lib/pkgconfig', 'lib64/pkgconfig', 'share/pkgconfig', 'usr/lib/pkgconfig', 'usr/share/pkgconfig'])
        env['PKG_CONFIG_SYSROOT_DIR'] = str(prefix)
        for key in ['CPATH', 'C_INCLUDE_PATH', 'CPLUS_INCLUDE_PATH', 'LIBRARY_PATH', 'CMAKE_PREFIX_PATH',
                    'CMAKE_INCLUDE_PATH', 'CMAKE_LIBRARY_PATH', 'CPPFLAGS', 'CFLAGS', 'CXXFLAGS', 'LDFLAGS']:
            env.pop(key, None)
    elif prefix:
        env['PKG_CONFIG_PATH'] = os.pathsep.join([str(prefix / 'lib/pkgconfig'), str(prefix / 'share/pkgconfig'), env.get('PKG_CONFIG_PATH', '')])
    return env


def execute(args, env, log):
    """Keep complete tool diagnostics and propagate a nonzero status."""
    log.parent.mkdir(parents=True, exist_ok=True)
    with log.open('w') as output:
        subprocess.run([str(arg) for arg in args], env=env, stdout=output, stderr=subprocess.STDOUT, check=True)


def cmake_target(args):
    """mochiOS must use its own SDK toolchain rather than masquerading as Linux."""
    result = []
    if args.target == 'mochios':
        if args.toolchain is None or not args.toolchain.is_file():
            raise ValueError('--toolchain must name a real mochiOS shared-library CMake toolchain')
        result += [f'-DCMAKE_TOOLCHAIN_FILE={args.toolchain}']
    return result


def probe(args, env, output):
    """Compile and link C++23, threads, exceptions and an actual shared library; never execute target code."""
    execute(['cmake', '-S', VENDOR / 'cmake/probe', '-B', output, '-G', args.generator,
             f'-DTERRA_EXPECT_MOCHIOS={"ON" if args.target == "mochios" else "OFF"}', *cmake_target(args)], env, output / 'configure.log')
    execute(['cmake', '--build', output, '--parallel', '2'], env, output / 'build.log')


def doctor(args, lock, output):
    """Persist all preflight results; missing tools/dependencies are failures, not completed builds."""
    checks = []
    dependencies = json.loads((VENDOR / 'dependencies.json').read_text())
    def record(name, ok, detail):
        checks.append(dict(name=name, ok=bool(ok), detail=str(detail)))
    tools = dependencies['host_tools'] + (dependencies['linux_sandbox_tools'] if args.target == 'linux' else [])
    tools += ['ninja']
    for tool in tools:
        found = shutil.which(tool)
        record('tool:' + tool, found, found or 'missing')
    free = shutil.disk_usage(output).free
    record('build-space', free >= args.budget_gib * 1024 ** 3, f'{free} bytes free; requested budget {args.budget_gib} GiB')
    try:
        env = environment(args.target, args.deps_prefix)
        cmake_target(args)
    except (ValueError, OSError, json.JSONDecodeError) as error:
        record('target-environment', False, error)
        env = None
    if env is not None:
        record('target-environment', True, args.target)
        for name, minimum in dependencies['pkg_config'].items():
            command = ['pkg-config', '--print-errors', '--exists', name + (' >= ' + minimum if minimum else '')]
            try:
                result = subprocess.run(command, env=env, capture_output=True, text=True)
                version = subprocess.run(['pkg-config', '--modversion', name], env=env, capture_output=True, text=True)
                record('dependency:' + name, result.returncode == 0, version.stdout.strip() or result.stderr.strip())
            except OSError as error:
                record('dependency:' + name, False, error)
        try:
            probe_key = hashlib.sha256(json.dumps(configuration(args, lock), sort_keys=True).encode()).hexdigest()[:16]
            probe(args, env, output / 'probes' / probe_key)
            record('compile-link-probe', True, 'C++23 executable and shared-library links succeeded; not an OS execution test')
        except (ValueError, OSError, subprocess.CalledProcessError) as error:
            record('compile-link-probe', False, f'{error}; see {output / "probes"}')
    report = dict(format=1, target=args.target, source=lock, ready_to_configure=all(c['ok'] for c in checks), checks=checks)
    write_json(output / 'doctor.json', report)
    for check in checks:
        print(('OK   ' if check['ok'] else 'FAIL ') + check['name'] + ': ' + check['detail'])
    return report


def configuration(args, lock):
    """Fingerprint the source/profile/toolchain inputs so a stale configuration is not reused."""
    return dict(format=1, target=args.target, source=lock, generator=args.generator,
                profile_sha256=digest(VENDOR / 'cmake/bootstrap.cmake'),
                dependencies_sha256=digest(VENDOR / 'dependencies.json'),
                toolchain=str(args.toolchain) if args.toolchain else None,
                toolchain_sha256=digest(args.toolchain) if args.toolchain else None,
                deps_prefix=str(args.deps_prefix) if args.deps_prefix else None,
                compiler_environment={k: os.environ.get(k) for k in ['CC', 'CXX', 'CFLAGS', 'CXXFLAGS', 'LDFLAGS', 'PKG_CONFIG_PATH']})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=['fetch', 'prepare', 'doctor', 'configure', 'build', 'install'])
    parser.add_argument('--target', choices=['linux', 'mochios'], default='linux')
    parser.add_argument('--network', action='store_true', help='Permit HTTPS only for the fetch command')
    parser.add_argument('--cache', type=Path, default=VENDOR / 'downloads')
    parser.add_argument('--source-root', type=Path, default=VENDOR / 'src')
    parser.add_argument('--build-root', type=Path, default=VENDOR / 'build')
    parser.add_argument('--deps-prefix', type=Path, help='Target dependency sysroot (requires target.json for mochiOS)')
    parser.add_argument('--toolchain', type=Path, help='SDK CMake toolchain, required for mochiOS')
    parser.add_argument('--generator', choices=['Ninja'], default='Ninja', help='Pinned WPE requires Ninja')
    parser.add_argument('--jobs', type=int, default=2)
    parser.add_argument('--budget-gib', type=int, default=25, help='Local build disk budget, not an upstream minimum')
    args = parser.parse_args()
    if sys.version_info < (3, 12):
        parser.error('Python 3.12 or newer is required')
    if args.jobs < 1 or args.budget_gib < 1:
        parser.error('jobs and budget-gib must be positive')
    if args.network and args.command != 'fetch':
        parser.error('Network access belongs only to fetch; subsequent steps are offline')
    for name in ['cache', 'source_root', 'build_root', 'deps_prefix', 'toolchain']:
        if value := getattr(args, name):
            setattr(args, name, value.resolve())
    lock = lockfile()
    if args.command in ['fetch', 'prepare']:
        archive = fetch(lock, args.cache, args.network)
        print(prepare(lock, archive, args.source_root) if args.command == 'prepare' else archive)
        return
    output = args.build_root / args.target
    output.mkdir(parents=True, exist_ok=True)
    if args.command in ['doctor', 'configure']:
        report = doctor(args, lock, output)
        if not report['ready_to_configure']:
            raise ValueError(f'Preflight failed; see {output / "doctor.json"}')
        if args.command == 'doctor':
            return
    env = environment(args.target, args.deps_prefix)
    expected = configuration(args, lock)
    state = output / 'configured.json'
    cmake_build = output / 'webkit'
    inputs = output / 'inputs.json'
    if inputs.exists() and json.loads(inputs.read_text()) != expected:
        raise ValueError('Configuration changed; choose a fresh --build-root')
    if args.command == 'configure':
        source = prepare(lock, fetch(lock, args.cache), args.source_root)
        # A changed compiler/target requires a separate build root, not a mixed CMake cache.
        if state.exists() and json.loads(state.read_text()) != expected:
            raise ValueError('Configuration changed; choose a fresh --build-root')
        write_json(inputs, expected)
        if state.exists():
            state.unlink()
        execute(['cmake', '-S', source, '-B', cmake_build, '-G', args.generator,
                 '-C', VENDOR / 'cmake/bootstrap.cmake', '-DCMAKE_INSTALL_PREFIX=/usr',
                 '-DFETCHCONTENT_FULLY_DISCONNECTED=ON', '-DFETCHCONTENT_UPDATES_DISCONNECTED=ON',
                 *cmake_target(args)], env, output / 'configure.log')
        write_json(state, expected)
        print(f'Configured: {cmake_build}')
    else:
        if not state.is_file() or json.loads(state.read_text()) != expected:
            raise ValueError('Run configure successfully with these exact inputs before building/installing')
        receipt = output / 'installed.json'
        if receipt.exists():
            receipt.unlink()
        execute(['cmake', '--build', cmake_build, '--parallel', args.jobs], env, output / 'build.log')
        if args.command == 'install':
            destination = Path(tempfile.mkdtemp(dir=output, prefix='install-'))
            # Install into a private DESTDIR; never mutate the host or OS SDK.
            env['DESTDIR'] = str(destination)
            execute(['cmake', '--install', cmake_build], env, output / 'install.log')
            write_json(output / 'installed.json', dict(configuration=expected, destination=str(destination), files=[
                dict(path=str(p.relative_to(destination)), sha256=digest(p))
                for p in sorted(destination.rglob('*')) if p.is_file() and not p.is_symlink()]))
            print(f'Installed development artifacts: {destination}; not a Terra runtime package')
        else:
            print(f'Built: {cmake_build}')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, KeyError, subprocess.CalledProcessError, tarfile.TarError) as error:
        print(f'WebKit build failed: {error}', file=sys.stderr)
        raise SystemExit(1)
