"""Check feature lanes without compiling the workspace."""
import os
from pathlib import Path
import subprocess
import shlex
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent


class FeatureLaneTests(unittest.TestCase):
    def test_send_only_lane_cannot_unify_full_email_from_workspace(self):
        with tempfile.TemporaryDirectory(prefix="feature lanes ") as directory:
            root = Path(directory)
            calls = root / "calls.log"
            # Functions avoid Windows PATH/PATHEXT lookup and executable-script
            # handling; forward slashes work in both native and Git Bash paths.
            result = subprocess.run(
                [os.environ.get("FEATURE_TEST_BASH", "bash"), "-c", '\n'.join([
                    'cargo() { printf "%s\\n" "$*" >> "$FEATURE_LANE_CALLS"; }',
                    'python3() { printf "%s\\n" "$*" >> "$FEATURE_LANE_CALLS"; }',
                    'export -f cargo python3',
                    'source "$1"',
                ]), "feature-lanes", (ROOT / "scripts/test-features.sh").as_posix()],
                env=dict(
                    os.environ,
                    FEATURE_LANE_CALLS=calls.as_posix(),
                    FEATURE_TEST_SKIP_CLIENT_PREPARE="1",
                    RUNNER_OS="Linux",
                ),
                capture_output=True, text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            commands = [shlex.split(line) for line in calls.read_text().splitlines()]
            self.assertIn(["scripts/check-bus-purity.py"], commands)
            self.assertIn(["scripts/__tests__/check_bus_purity_tests.py"], commands)
            self.assertIn(
                ["test", "--locked", "-p", "tinychannels", "--no-default-features", "--features", "email-send"],
                commands,
            )
            self.assertIn(["test", "--locked", "-p", "tinychannels", "--no-default-features"], commands)


if __name__ == "__main__":
    unittest.main()
