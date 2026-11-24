#!/usr/bin/env python3
"""Run the synthetic packaging suites and the Linux launcher argv tests; no app build or signing."""
import importlib.util
from pathlib import Path
import subprocess
import unittest


def main() -> int:
    here = Path(__file__).resolve().parent
    suite = unittest.TestSuite()
    for name, relative in (
        ("effect_archive_suite", "nightly-archive-tests.py"),
        ("effect_plist_suite", "macos/nightly_plist_tests.py"),
        ("effect_release_dispatch_suite", "nightly-release-tests.py"),
        ("effect_mac_sign_suite", "macos/nightly_sign_tests.py"),
    ):
        spec = importlib.util.spec_from_file_location(name, here / relative)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        suite.addTests(unittest.defaultTestLoader.loadTestsFromModule(module))
    result = 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1
    # The Linux launchers are shell scripts: each runs an argv recorder as the GUI and compares what
    # it would start, so a wrapper that stops passing the launch profile fails here.
    for script in ("linux/nightly-arch-tar-test.sh", "linux/nightly-launch-test.sh"):
        result |= subprocess.run(["bash", str(here / script)], check=False).returncode
    return result


if __name__ == "__main__":
    raise SystemExit(main())
