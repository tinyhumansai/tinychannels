"""Check that feature tests reuse their invoking shell instead of PATH's bash."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


@unittest.skipIf(os.name == "nt", "POSIX fixture; the feature-lane test runs on Windows CI")
class FeatureShellTests(unittest.TestCase):
    def test_feature_harness_ignores_a_competing_bash_on_path(self):
        with tempfile.TemporaryDirectory(prefix="feature shell ") as directory:
            competing_bash = Path(directory) / "bash"
            competing_bash.write_text("#!/bin/sh\necho wrong-bash >&2\nexit 1\n")
            competing_bash.chmod(0o755)
            result = subprocess.run(
                [shutil.which("bash"), "-c", '\n'.join([
                    'cargo() { return 0; }',
                    'python3() {',
                    '  if [[ "$1" == scripts/test-feature-lanes.py ]]; then',
                    '    "$FEATURE_TEST_PYTHON" "$@"',
                    '  fi',
                    '}',
                    'export -f cargo python3',
                    'source "$1"',
                ]), "feature-shell", str(ROOT / "scripts/test-features.sh")],
                env=dict(os.environ, PATH=directory + os.pathsep + os.environ["PATH"],
                         FEATURE_TEST_PYTHON=sys.executable,
                         FEATURE_TEST_SKIP_CLIENT_PREPARE="1",
                         FEATURE_TEST_BASH=shutil.which("bash"), RUNNER_OS="Linux"),
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
