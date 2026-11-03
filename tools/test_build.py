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
    def test_missing_wrong_target_and_symlinked_engines_are_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=pathlib.Path(tmp);binary=root/'entry';write_elf(binary)
            engine=root/'engine';engine.mkdir()
            with self.assertRaises(OSError):module.stage(binary,root/'out',engine)
            (engine/'engine.json').write_text(json.dumps(dict(format=1,target='linux',version='1')))
            with self.assertRaises(ValueError):module.stage(binary,root/'out',engine)
            (engine/'engine.json').write_text(json.dumps(dict(format=1,target='mochios-1',version='1')))
            write_elf(engine/'libwebkit.so',3)
            (engine/'runtime').mkdir();(engine/'runtime/escape').symlink_to(binary)
            with self.assertRaises(ValueError):module.stage(binary,root/'out',engine)
    def test_supplied_engine_is_packaged_as_library_without_claiming_adapter_support(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=pathlib.Path(tmp);binary=root/'entry';write_elf(binary)
            engine=root/'engine';engine.mkdir();write_elf(engine/'libwebkit.so',3)
            (engine/'engine.json').write_text(json.dumps(dict(format=1,target='mochios-1',version='fixture-only')))
            product=module.stage(binary,root/'out',engine)
            manifest=module.tomllib.loads((product/'manifest.toml').read_text())
            self.assertEqual(manifest['binary'][1]['kind'],'library')
            self.assertEqual(manifest['binary'][1]['path'],'/libraries/libwebkit.so')
            self.assertFalse(json.loads((product/'artifacts.json').read_text())['engine_adapter_connected'])
    def test_runtime_code_cannot_bypass_binary_declarations(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=pathlib.Path(tmp);binary=root/'entry';write_elf(binary)
            engine=root/'engine';engine.mkdir();write_elf(engine/'libwebkit.so',3)
            (engine/'engine.json').write_text(json.dumps(dict(format=1,target='mochios-1',version='fixture-only')))
            (engine/'runtime').mkdir();helper=engine/'runtime/helper'
            write_elf(helper)
            with self.assertRaises(ValueError):module.stage(binary,root/'out',engine)
            helper.write_bytes(b'#!/bin/sh');helper.chmod(0o755)
            with self.assertRaises(ValueError):module.stage(binary,root/'out',engine)
            helper.unlink();(engine/'runtime/notice.txt').write_text('notice')
            product=module.stage(binary,root/'out',engine)
            self.assertEqual((product/'payload/root/libraries/webkit/notice.txt').read_text(),'notice')

    def test_incompatible_or_non_elf_binary_does_not_produce_an_app(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=pathlib.Path(tmp);binary=root/'entry'
            for kind,machine in [(3,62),(2,183)]:
                write_elf(binary,kind,machine)
                with self.assertRaises(ValueError):module.stage(binary,root/'out')
            binary.write_bytes(b'not an ELF')
            with self.assertRaises(ValueError):module.stage(binary,root/'out')

if __name__=='__main__':unittest.main()
