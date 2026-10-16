"""Names AE saved in Windows-1252 (a ’ kept as a lone surrogate) export as valid UTF-8 text."""
import unittest

from aep_ir_export import windows_bytes_to_text


class WindowsNamesTest(unittest.TestCase):
    def test_lone_surrogates_become_windows_characters(self):
        self.assertEqual(windows_bytes_to_text("it\udc92s"), "it’s")
        self.assertEqual(windows_bytes_to_text("plain ✓"), "plain ✓")
        windows_bytes_to_text("x\udc81y").encode("utf-8")  # undefined in cp1252: still encodable


if __name__ == "__main__":
    unittest.main()
