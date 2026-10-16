"""An unsupported interpreter reports the required version before importing dependencies."""
import subprocess
import sys
import unittest
from pathlib import Path


class PythonVersionTest(unittest.TestCase):
    def test_old_interpreter_gets_an_actionable_error(self):
        script = Path(__file__).with_name("aep_ir_export.py")
        code = "import runpy, sys; sys.version_info = (3, 10, 0); runpy.run_path(sys.argv[1], run_name='__main__')"
        result = subprocess.run([sys.executable, "-c", code, str(script)], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Python 3.11 or newer", result.stderr)
        self.assertIn("EFFECTCRAFT_AEP_PYTHON", result.stderr)
        self.assertNotIn("Traceback", result.stderr)


if __name__ == "__main__":
    unittest.main()
