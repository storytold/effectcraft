"""A Windows media path saved in the project, too long to be a file name here, reads as missing
media instead of stopping the export (Earnedits projects saved on Windows)."""
import unittest
from pathlib import Path

from aep_ir_export import is_file


class LongPathTest(unittest.TestCase):
    def test_too_long_windows_path_is_not_a_file(self):
        raw = 'D:\\earner.co\\15th reel\\(Footage)\\' + 'Big shot iphone ' * 30 + '.mp4'
        self.assertFalse(is_file(Path(raw)))
        self.assertTrue(is_file(Path(__file__)))


if __name__ == '__main__':
    unittest.main()
