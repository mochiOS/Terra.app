"""Contract tests for Terra-owned artifact staging and engine rejection."""
import importlib.util
import json
import pathlib
import struct
import tempfile
import unittest
spec=importlib.util.spec_from_file_location('terra_build',pathlib.Path(__file__).with_name('build.py'))
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)

def write_elf(path,kind=2,machine=62):
    data=bytearray(64);data[:6]=b'\x7fELF\x02\x01';struct.pack_into('<HH',data,16,kind,machine);path.write_bytes(data)

class Packaging(unittest.TestCase):
    def test_shell_package_is_explicit_and_has_valid_application_targets(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=pathlib.Path(tmp);binary=root/'entry';write_elf(binary)
            product=module.stage(binary,root)
            manifest=module.tomllib.loads((product/'manifest.toml').read_text())
            self.assertEqual(manifest['binary'][0]['path'],'/applications/Terra.app/entry.elf')
            self.assertNotIn('provenance',manifest['package'])
            self.assertTrue((product/'Terra.app/entry.elf').is_file())
            self.assertTrue(json.loads((product/'artifacts.json').read_text())['shell_only'])
            self.assertFalse((product/'payload/root/libraries/libwebkit.so').exists())
            for file in manifest['file']:
                path=product/'payload/bundle'/file['path'][2:]
                self.assertEqual(file['size'],path.stat().st_size)
                self.assertEqual(file['digest'],'sha256:'+module.hashlib.sha256(path.read_bytes()).hexdigest())
    def test_engine_payload_is_never_bundled_into_the_browser(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=pathlib.Path(tmp);binary=root/'entry';write_elf(binary)
            for content in [False, True]:
                engine=root/('engine-full' if content else 'engine-empty');engine.mkdir()
                if content:
                    write_elf(engine/'libwebkit.so',3)
                    (engine/'engine.json').write_text(json.dumps(dict(format=1,target='mochios-1',version='fixture')))
                with self.assertRaisesRegex(ValueError,'independent'):
                    module.stage(binary,root/'out',engine)
                self.assertFalse((root/'out/product').exists())

if __name__=='__main__':unittest.main()

class NativeEngineBuild(unittest.TestCase):
    def test_final_linker_replaces_static_sdk_flags_without_losing_remapping(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=pathlib.Path(tmp); shared=root/'sdk'; prefix=root/'deps'
            (shared/'bin').mkdir(parents=True); (shared/'bin/mochios-c++').write_text('test compiler identity')
            (prefix/'usr/lib').mkdir(parents=True); write_elf(prefix/'usr/lib/libwebkit.so',3)
            flags=module.engine_rustflags({'rustflags':['--remap-path-prefix','/private=/src','-C','linker=old','-C','link-arg=-static','-C','panic=abort']},shared,prefix)
            self.assertIn('/private=/src',flags); self.assertIn('panic=abort',flags)
            self.assertNotIn('linker=old',flags);self.assertNotIn('link-arg=-static',flags)
            self.assertIn('linker='+str(shared/'bin/mochios-c++'),flags)
            self.assertFalse(any('rpath=' in f for f in flags))
    def test_engine_identity_cannot_default_to_an_unreviewed_publisher(self):
        with tempfile.TemporaryDirectory() as tmp:
            p=pathlib.Path(tmp)/'engine.toml';p.write_text('package="org.example.webkit"\n')
            with self.assertRaisesRegex(ValueError,'five'): module.engine_dependency(p)
    def test_shell_mode_rejects_a_real_engine_dependency(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as tmp:
            root=pathlib.Path(tmp);binary=root/'entry';write_elf(binary)
            with patch.object(module,'has_engine_dependency',return_value=True):
                with self.assertRaisesRegex(ValueError,'does not match'):module.stage(binary,root)
            self.assertFalse((root/'product').exists())

class NativePackageIdentity(unittest.TestCase):
    def test_native_package_records_identity_and_only_parent_spawn_authority(self):
        from unittest.mock import patch
        identity=dict(package='org.example.engine',developer='org.example.publisher',subject_key_id='a'*64,provenance='development',abi='example-1')
        with tempfile.TemporaryDirectory() as tmp:
            root=pathlib.Path(tmp);binary=root/'entry';write_elf(binary)
            with patch.object(module,'has_engine_dependency',return_value=True):
                product=module.stage(binary,root,engine_linked=True,dependency=identity)
            manifest=module.tomllib.loads((product/'manifest.toml').read_text())
            self.assertEqual(manifest['library_dependency'],[identity])
            self.assertIn('process.spawn',manifest['binary'][0]['requires'])
            self.assertIn('net.connect',manifest['binary'][0]['requires'])
            self.assertNotIn('fs.read.all',manifest['binary'][0]['requires'])
            self.assertEqual(manifest['binary'][0]['executable_dependencies'],['webkit-web-process','webkit-network-process'])
            for helper in manifest['executable_dependency']:
                self.assertEqual({k: helper[k] for k in identity}, identity)
            self.assertEqual([h['export'] for h in manifest['executable_dependency']],['web-process','network-process'])
            report=json.loads((product/'artifacts.json').read_text())
            self.assertTrue(report['engine_adapter_linked']);self.assertFalse(report['runtime_tested'])
            self.assertFalse(report['engine_adapter_connected'])
