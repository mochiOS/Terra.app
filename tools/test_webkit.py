"""The browser must not silently retain an independent engine builder."""
from pathlib import Path
import subprocess
import sys
import unittest
class Ownership(unittest.TestCase):
    def test_old_command_points_to_the_independent_engine_owner(self):
        command = [sys.executable, str(Path(__file__).with_name('webkit.py')), 'build']
        result = subprocess.run(command, capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn('mochiOS/libs', result.stderr)
if __name__ == '__main__': unittest.main()
