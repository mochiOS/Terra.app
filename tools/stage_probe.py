#!/usr/bin/env python3
"""Stage native WebKit validation applications in disposable VM overlays."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def stage(probe, output, msign, *, manual=False):
    """Bind a private built-in fixture with the normal signed helper dependencies."""
    if output.exists():
        raise ValueError('Use a fresh private overlay directory')
    if manual:
        report = json.loads((probe / 'artifacts.json').read_text())
        if report.get('shell_only') is not False or report.get('engine_adapter_linked') is not True:
            raise ValueError('Expected a normal product linked to the real engine')
        probe = probe / 'payload/bundle/entry.elf'
    elif probe.name != 'entry.elf' or probe.parent.name != 'native-probe':
        raise ValueError('Expected tools/build.py --engine-probe output')
    dynamic = subprocess.check_output(['readelf', '-d', str(probe)], text=True)
    if '[libwebkit.so.1]' not in dynamic:
        raise ValueError('The fixture must link the real native engine')
    config = tomllib.loads((ROOT / 'manifest.toml').read_text())
    identity = tomllib.loads((ROOT / 'resources/webkit-dependency.toml').read_text())
    requirements = list(config['binary'][0]['requires'])
    for capability in ['process.spawn', 'system.random.read', 'system.time.read',
                       'fs.read.tmp', 'fs.write.tmp', 'net.connect']:
        if capability not in requirements:
            requirements.append(capability)
    bundle = output / 'system/applications/Terra.app'
    bundle.mkdir(parents=True)
    shutil.copy2(probe, bundle / 'entry.elf')
    (bundle / 'entry.elf').chmod(0o755)
    shutil.copy2(ROOT / 'appicon.png', bundle / 'appicon.png')
    text = 'format=1\n[package]\nprovenance="built-in"\n'
    for key, value in config['package'].items():
        if key != 'provenance':
            text += key + '=' + json.dumps(value) + '\n'
    text += '\n[application]\n'
    for key, value in config['application'].items():
        text += key + '=' + json.dumps(value) + '\n'
    text += '\n[[binary]]\npath="/system/applications/Terra.app/entry.elf"\nkind="application"\n'
    text += 'requires=' + json.dumps(requirements) + '\n'
    text += 'executable_dependencies=["webkit-web-process","webkit-network-process"]\n'
    for alias, export in [('webkit-web-process', 'web-process'), ('webkit-network-process', 'network-process')]:
        text += '\n[[executable_dependency]]\nalias=' + json.dumps(alias) + '\nexport=' + json.dumps(export) + '\n'
        for key, value in identity.items():
            text += key + '=' + json.dumps(value) + '\n'
    text += '\n[[library_dependency]]\n'
    for key, value in identity.items():
        text += key + '=' + json.dumps(value) + '\n'
    package = output / 'system/packages/terra'
    package.mkdir(parents=True)
    source = package / 'fixture-input.toml'
    source.write_text(text)
    subprocess.run([str(msign), 'package', 'bind-built-in', str(source), '--rootfs',
                    str(output), '--output', str(package / 'manifest.toml')], check=True)
    subprocess.run([str(msign), 'package', 'built-in-record', str(package / 'manifest.toml'),
                    '--output', str(package / 'verification.bin')], check=True)
    # Workspace discovers normal applications through their bundle manifest.
    # Reuse the bound identity; executable authority remains the verified record.
    shutil.copy2(package / 'manifest.toml', bundle / 'manifest.toml')
    source.unlink()
    if not manual:
        resources = output / 'system/resources'
        resources.mkdir(parents=True)
        (resources / 'native-test.conf').write_text(
            'application=/system/applications/Terra.app/entry.elf\n'
            'result=/tmp/terra-engine-results.log\npass=TERRA-ENGINE PASS\n'
            'fail=TERRA-ENGINE FAIL\ntimeout_seconds=1260\n')
    print('Private native application overlay:', output)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    inputs = parser.add_mutually_exclusive_group(required=True)
    inputs.add_argument('--probe', type=Path)
    inputs.add_argument('--manual-product', type=Path, help='Normal linked product for manual desktop verification')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--msign', type=Path, required=True)
    args = parser.parse_args()
    selected = args.probe or args.manual_product
    stage(selected.resolve(strict=True), args.output.resolve(), args.msign.resolve(strict=True),
          manual=args.manual_product is not None)
