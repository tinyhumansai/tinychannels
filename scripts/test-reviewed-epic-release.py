"""Keep the release workflow to the main tag path, with no reviewed-epic mode."""

import os
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib
import unittest


ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github/workflows/release.yml"
TINYBUS_REV = "61dce96aac5d20d8b6fc7cfe27daef559c7d54c9"


def workflow_run_block(step_name):
    lines = WORKFLOW.read_text(encoding="utf-8").splitlines()
    step = next(
        i
        for i, line in enumerate(lines)
        if line.strip() == step_name
    )
    run = next(i for i in range(step + 1, len(lines)) if lines[i].strip() == "run: |")
    block = []
    for line in lines[run + 1 :]:
        if line.startswith("          "):
            block.append(line[10:])
        elif not line.strip():
            block.append("")
        else:
            break
    if not block:
        raise AssertionError(f"workflow run block is empty for {step_name}")
    return "\n".join(block) + "\n"


def gitlink_revision(repo_root, path):
    """Return a committed gitlink revision without requiring submodule contents."""
    result = subprocess.run(
        ["git", "-C", str(repo_root), "ls-tree", "HEAD", "--", path],
        check=True,
        capture_output=True,
        text=True,
    )
    entry = result.stdout.strip()
    if not entry:
        return None
    metadata, separator, entry_path = entry.partition("\t")
    fields = metadata.split()
    if not separator or entry_path != path or len(fields) != 3:
        return None
    mode, object_type, revision = fields
    if mode != "160000" or object_type != "commit":
        return None
    return revision


def run_tag_resolver(tag):
    bash = os.environ.get("FEATURE_TEST_BASH", "bash")
    with tempfile.TemporaryDirectory(prefix="ordinary release tag ") as directory:
        output_file = Path(directory) / "github-output.txt"
        result = subprocess.run(
            [bash, "-c", workflow_run_block("- id: tag")],
            cwd=directory,
            env=dict(os.environ, TAG=tag, GITHUB_OUTPUT="github-output.txt"),
            capture_output=True,
            text=True,
        )
        output = output_file.read_text(encoding="utf-8") if output_file.exists() else ""
        return result, output


class ReleaseWorkflowTests(unittest.TestCase):
    def test_a_dispatch_names_only_a_tag_and_runs_only_the_module_release(self):
        # Releases come only from official main, so there is no mode to choose.
        source = WORKFLOW.read_text(encoding="utf-8")
        inputs = source.split("    inputs:\n", 1)[1].split("  push:\n", 1)[0]
        self.assertEqual(re.findall(r"(?m)^      (\w+):$", inputs), ["tag"])
        jobs = re.findall(r"(?m)^  ([\w-]+):$", source.split("\njobs:\n", 1)[1])
        self.assertEqual(jobs, ["resolve", "module-release"])
        for epic_only in ("reviewed_pr", "inputs.mode", "module-reviewed-epic-release", "epic/"):
            self.assertNotIn(epic_only, source)
        self.assertEqual(source.count("uses: tinyhumansai/.github/"), 1)

    def test_tag_must_be_v_prefixed_semver(self):
        for value in ("0.1.37", "v0.1", "vrelease"):
            with self.subTest(tag=value):
                result, output = run_tag_resolver(value)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(output, "")

        result, output = run_tag_resolver("v0.1.37")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(output, "tag=v0.1.37\nversion=0.1.37\n")

    def test_release_workflow_test_runs_in_existing_feature_test_ci_lane(self):
        test_features = (ROOT / "scripts/test-features.sh").read_text(encoding="utf-8")
        ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn(
            'FEATURE_TEST_BASH="$feature_test_bash" python3 scripts/test-reviewed-epic-release.py',
            test_features,
        )
        self.assertIn("feature-test-script: scripts/test-features.sh", ci)

    def test_release_is_the_registered_tag_workflow(self):
        source = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("on:\n  workflow_dispatch:", source)
        self.assertIn("  push:\n    tags: ['v*']", source)
        self.assertFalse((ROOT / ".github/workflows/epic-release.yml").exists())

    def test_gitlink_lookup_accepts_exact_and_rejects_stale_or_missing(self):
        def repository_with_gitlink(revision):
            temp = tempfile.TemporaryDirectory()
            self.addCleanup(temp.cleanup)
            repo = Path(temp.name)
            subprocess.run(["git", "init", "-q", str(repo)], check=True)
            subprocess.run(
                ["git", "-C", str(repo), "config", "user.name", "Test"],
                check=True,
            )
            subprocess.run(
                ["git", "-C", str(repo), "config", "user.email", "test@example.invalid"],
                check=True,
            )
            if revision is not None:
                subprocess.run(
                    [
                        "git", "-C", str(repo), "update-index", "--add", "--cacheinfo",
                        f"160000,{revision},vendor/tinybus",
                    ],
                    check=True,
                )
            subprocess.run(
                ["git", "-C", str(repo), "commit", "--allow-empty", "-qm", "fixture"],
                check=True,
            )
            return repo

        exact = repository_with_gitlink(TINYBUS_REV)
        stale = repository_with_gitlink("1" * 40)
        missing = repository_with_gitlink(None)
        self.assertEqual(gitlink_revision(exact, "vendor/tinybus"), TINYBUS_REV)
        self.assertNotEqual(gitlink_revision(stale, "vendor/tinybus"), TINYBUS_REV)
        self.assertIsNone(gitlink_revision(missing, "vendor/tinybus"))

    def test_tinybus_and_module_sdk_share_the_published_workspace_source(self):
        workspace = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
        module = (ROOT / "crates/tinychannels-module/Cargo.toml").read_text(
            encoding="utf-8"
        )
        fixture = (ROOT / "crates/tinychannels-client/test-fixture/Cargo.toml").read_text(
            encoding="utf-8"
        )
        self.assertIn(
            'tinybus = { version = "=0.3.12", git = '
            '"https://github.com/senamakel/tinybus", tag = "v0.3.12", '
            'default-features = false, features = ["macros", "modules"] }',
            module,
        )
        self.assertIn(
            'tinybus-module = { version = "=0.3.12", git = '
            '"https://github.com/senamakel/tinybus", tag = "v0.3.12" }',
            module,
        )
        self.assertIn(
            'tinybus = { path = "vendor/tinybus/crates/tinybus" }\n'
            'tinybus-module = { path = "vendor/tinybus/crates/tinybus-module" }',
            workspace,
        )
        workspace_manifest = tomllib.loads(workspace)
        module_manifest = tomllib.loads(module)
        test_bus = module_manifest["dev-dependencies"]["tinybus"]
        self.assertTrue(test_bus["workspace"])
        self.assertIn("test-support", test_bus["features"])
        shared_bus = workspace_manifest["workspace"]["dependencies"]["tinybus"]
        self.assertEqual(shared_bus["version"], "=0.3.12")
        self.assertEqual(shared_bus["git"], "https://github.com/senamakel/tinybus")
        self.assertEqual(shared_bus["tag"], "v0.3.12")
        self.assertFalse(shared_bus["default-features"])
        self.assertIn(
            'tinybus = { version = "0.3.12", path = "../../../vendor/tinybus/crates/tinybus"',
            fixture,
        )
        self.assertIn(
            'tinybus-module = { version = "0.3.12", path = "../../../vendor/tinybus/crates/tinybus-module"',
            fixture,
        )
        self.assertIn(
            'tinychannels-bus = { path = "../../tinychannels-bus", features = ["storage"] }',
            fixture,
        )
        self.assertEqual(gitlink_revision(ROOT, "vendor/tinybus"), TINYBUS_REV)

    def test_ordinary_tag_release_caller_remains_in_place(self):
        source = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        self.assertIn(
            "tinyhumansai/.github/.github/workflows/module-release.yml@"
            "3e87ade76b622844985d0c665a3b075313e4545c",
            source,
        )
        self.assertIn("module-release:", source)
        self.assertIn("needs: resolve", source)


if __name__ == "__main__":
    unittest.main()
