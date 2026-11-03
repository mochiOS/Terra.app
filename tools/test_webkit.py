"""Source integrity, target isolation, failure propagation and real link-probe tests."""
import argparse
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('webkit_build', Path(__file__).with_name('webkit.py'))
webkit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(webkit)


class WebKitBuildTests(unittest.TestCase):
    def archive(self, root, entries=None):
        archive = root / 'source.tar.xz'
        with tarfile.open(archive, 'w:xz') as tar:
            for name, kind, value in entries or [('source/CMakeLists.txt', 'file', b'project(Fixture)')]:
                info = tarfile.TarInfo(name)
                if kind == 'link':
                    info.type = tarfile.SYMTYPE
                    info.linkname = value
                    tar.addfile(info)
                elif kind == 'device':
                    info.type = tarfile.CHRTYPE
                    tar.addfile(info)
                else:
                    info.size = len(value)
                    tar.addfile(info, io.BytesIO(value))
        return archive, dict(format=1, directory='source', sha256=webkit.digest(archive), version='test', url='https://invalid.example/source.tar.xz')

    def test_offline_cache_requires_the_pinned_hash(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source, lock = self.archive(root)
            source.rename(root / 'source.tar.xz.saved')
            with self.assertRaisesRegex(ValueError, 'missing'):
                webkit.fetch(lock, root)
            (root / 'source.tar.xz.saved').rename(source)
            self.assertEqual(webkit.fetch(lock, root), source)
            source.write_bytes(b'corrupt')
            with self.assertRaisesRegex(ValueError, 'hash mismatch'):
                webkit.fetch(lock, root, network=True)

    def test_archive_traversal_external_links_and_devices_are_rejected(self):
        for entries in [
            [('../escape', 'file', b'x')],
            [('/source/absolute', 'file', b'x')],
            [('source/../../escape', 'file', b'x')],
            [('other/file', 'file', b'x')],
            [('source/link', 'link', '../../escape')],
            [('source/link', 'link', '/etc/passwd')],
            [('source/device', 'device', b'')],
            [('source/duplicate', 'file', b'a'), ('source/duplicate', 'file', b'b')],
            [('source/duplicate', 'file', b'a'), ('source/./duplicate', 'file', b'b')],
        ]:
            with self.subTest(entries=entries), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                archive, lock = self.archive(root, entries)
                with self.assertRaises(ValueError):
                    webkit.prepare(lock, archive, root / 'src')
                self.assertFalse((root / 'src/source').exists())

    def test_identical_regular_duplicates_in_the_official_archive_are_safe(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive, lock = self.archive(root, [('source/theme.css', 'file', b'same'), ('source/theme.css', 'file', b'same')])
            with tarfile.open(archive) as tar:
                self.assertEqual(len(webkit.archive_members(tar, lock['directory'])), 1)

    def test_prepare_is_repeatable_and_refuses_unowned_sources_or_low_space(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive, lock = self.archive(root)
            with patch.object(webkit.shutil, 'disk_usage', return_value=argparse.Namespace(free=0)):
                with self.assertRaisesRegex(ValueError, 'Insufficient'):
                    webkit.prepare(lock, archive, root / 'src')
            with patch.object(webkit.shutil, 'disk_usage', return_value=argparse.Namespace(free=10 * 1024**3)):
                source = webkit.prepare(lock, archive, root / 'src')
            before = (source / 'CMakeLists.txt').stat().st_mtime_ns
            self.assertEqual(webkit.prepare(lock, archive, root / 'src'), source)
            self.assertEqual(before, (source / 'CMakeLists.txt').stat().st_mtime_ns)
            (source / '.terra-source.json').unlink()
            with self.assertRaisesRegex(ValueError, 'Untracked'):
                webkit.prepare(lock, archive, root / 'src')

    def test_cross_pkg_config_cannot_inherit_host_search_paths(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'target.json').write_text(json.dumps(dict(format=1, target='linux')))
            with self.assertRaises(ValueError):
                webkit.environment('mochios', root)
            (root / 'target.json').write_text(json.dumps(dict(format=1, target='mochios-1')))
            with patch.dict(os.environ, {'PKG_CONFIG_PATH': '/host/pkgconfig', 'CPATH': '/host/include', 'LIBRARY_PATH': '/host/lib'}):
                env = webkit.environment('mochios', root)
            self.assertEqual(env['PKG_CONFIG_PATH'], '')
            self.assertEqual(env['PKG_CONFIG_SYSROOT_DIR'], str(root))
            self.assertNotIn('/host', env['PKG_CONFIG_LIBDIR'])
            self.assertNotIn('CPATH', env)
            self.assertNotIn('LIBRARY_PATH', env)

    def test_insecure_redirect_is_rejected_before_requesting_it(self):
        with self.assertRaisesRegex(ValueError, 'insecure'):
            webkit.HTTPSRedirects().redirect_request(None, None, 302, '', {}, 'http://invalid.example/source')

    def test_tool_failure_keeps_diagnostics_and_propagates_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / 'failure.log'
            with self.assertRaises(subprocess.CalledProcessError):
                webkit.execute([shutil.which('python3'), '-c', 'print("actual failure"); raise SystemExit(7)'], os.environ.copy(), log)
            self.assertIn('actual failure', log.read_text())

    def test_unconfigured_build_never_runs_a_build_tool(self):
        with tempfile.TemporaryDirectory() as tmp:
            with patch.object(webkit.sys, 'argv', ['webkit.py', 'build', '--build-root', tmp]), patch.object(webkit, 'execute') as execute:
                with self.assertRaisesRegex(ValueError, 'configure successfully'):
                    webkit.main()
                execute.assert_not_called()
            self.assertFalse((Path(tmp) / 'linux/installed.json').exists())

    def test_build_failure_invalidates_an_older_install_receipt(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            output = root / 'linux'
            output.mkdir()
            args = argparse.Namespace(target='linux', generator='Ninja', toolchain=None, deps_prefix=None)
            config = webkit.configuration(args, webkit.lockfile())
            webkit.write_json(output / 'inputs.json', config)
            webkit.write_json(output / 'configured.json', config)
            webkit.write_json(output / 'installed.json', {'old': 'receipt'})
            with patch.object(webkit.sys, 'argv', ['webkit.py', 'build', '--build-root', tmp]), patch.object(webkit, 'execute', side_effect=subprocess.CalledProcessError(1, ['cmake'])):
                with self.assertRaises(subprocess.CalledProcessError):
                    webkit.main()
            self.assertFalse((output / 'installed.json').exists())

    @unittest.skipUnless(shutil.which('cmake') and shutil.which('make') and shutil.which('c++'), 'host C++ toolchain required')
    def test_real_shared_and_executable_links_and_host_toolchain_rejection(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            args = argparse.Namespace(target='linux', generator='Unix Makefiles', toolchain=None)
            webkit.probe(args, os.environ.copy(), root / 'host')
            self.assertTrue((root / 'host/terra_probe').is_file())
            # A host compiler with a renamed target is not accepted as a mochiOS SDK.
            args.target = 'mochios'
            args.toolchain = root / 'wrong-toolchain.cmake'
            args.toolchain.write_text('set(CMAKE_SYSTEM_NAME Linux)\n')
            with self.assertRaises(subprocess.CalledProcessError):
                webkit.probe(args, os.environ.copy(), root / 'cross')
            self.assertIn('Expected a real mochiOS cross toolchain', (root / 'cross/configure.log').read_text())
            args.toolchain.write_text('set(CMAKE_SYSTEM_NAME mochiOS)\nset(CMAKE_SYSTEM_PROCESSOR x86_64)\n')
            with self.assertRaises(subprocess.CalledProcessError):
                webkit.probe(args, os.environ.copy(), root / 'renamed-host')
            self.assertIn('requires a real CMAKE_SYSROOT', (root / 'renamed-host/configure.log').read_text())


if __name__ == '__main__':
    unittest.main()
