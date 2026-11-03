"""Exercise the actual reviewed-epic release workflow input validator."""

import os
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest


ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github/workflows/release.yml"
CANONICAL = "tinyhumansai/tinychannels"
EPIC_REF = "refs/heads/epic/8056-modules-wiring"
REVIEWED_RELEASE = "tinyhumansai/.github/.github/workflows/module-reviewed-epic-release.yml@6ed83b04aab2d8977ec62e966cb914531e534fa4"
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


def run_validator(**overrides):
    values = {
        "EVENT_NAME": "workflow_dispatch",
        "REPOSITORY": CANONICAL,
        "REF": EPIC_REF,
        "RELEASE_MODE": "reviewed",
        "REVIEWED_PR": "8056",
    }
    values.update(overrides)
    bash = os.environ.get("FEATURE_TEST_BASH", "bash")
    with tempfile.TemporaryDirectory(prefix="reviewed epic release ") as directory:
        output_file = Path(directory) / "github-output.txt"
        result = subprocess.run(
            [bash, "-c", workflow_run_block("- name: Validate reviewed epic dispatch inputs")],
            cwd=directory,
            env=dict(os.environ, **values, GITHUB_OUTPUT="github-output.txt"),
            capture_output=True,
            text=True,
        )
        output = output_file.read_text(encoding="utf-8") if output_file.exists() else ""
        return result, output


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


class ReviewedEpicReleaseTests(unittest.TestCase):
    def test_valid_dispatch_emits_reviewed_release_outputs(self):
        result, output = run_validator()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(output, "reviewed_pr=8056\n")

    def test_wrong_event_repo_branch_and_mode_are_rejected(self):
        for name, value in (
            ("EVENT_NAME", "push"),
            ("REPOSITORY", "senamakel/tinychannels"),
            ("REF", "refs/heads/main"),
            ("RELEASE_MODE", "tag-only"),
        ):
            with self.subTest(input=name, value=value):
                result, output = run_validator(**{name: value})
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(output, "")

    def test_legacy_zero_and_nonpositive_or_noninteger_pr_values_are_rejected(self):
        for value in ("0", "-1", "1.5", "abc", "01"):
            with self.subTest(reviewed_pr=value):
                result, output = run_validator(REVIEWED_PR=value)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(output, "")

    def test_tag_must_be_v_prefixed_semver(self):
        for value in ("0.1.37", "v0.1", "vrelease"):
            with self.subTest(tag=value):
                result, output = run_tag_resolver(value)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(output, "")

        result, output = run_tag_resolver("v0.1.37")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(output, "tag=v0.1.37\nversion=0.1.37\n")

    def test_workflow_calls_reviewed_release_with_skip_safe_numeric_pr(self):
        source = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("      mode:\n", source)
        self.assertIn("options: [tag, reviewed]", source)
        self.assertIn(REVIEWED_RELEASE, source)
        self.assertIn("fromJSON(needs.validate.outputs.reviewed_pr || '0')", source)
        self.assertIn("default: '0'", source)
        self.assertIn("if: inputs.mode == 'reviewed'", source)
        self.assertIn("if: github.event_name == 'push' || inputs.mode != 'reviewed'", source)

    def test_permissions_are_scoped_to_validation_and_release_needs(self):
        source = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("permissions: {}", source)
        self.assertIn(
            "  validate:\n    if: inputs.mode == 'reviewed'\n"
            "    runs-on: ubuntu-latest\n    permissions: {}",
            source,
        )
        self.assertIn(
            "      contents: write\n      pull-requests: read\n"
            "      checks: read\n      actions: read",
            source,
        )

    def test_release_validator_runs_in_existing_feature_test_ci_lane(self):
        test_features = (ROOT / "scripts/test-features.sh").read_text(encoding="utf-8")
        ci = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn(
            'FEATURE_TEST_BASH="$feature_test_bash" python3 scripts/test-reviewed-epic-release.py',
            test_features,
        )
        self.assertIn("feature-test-script: scripts/test-features.sh", ci)

    def test_reviewed_release_is_in_the_registered_release_workflow(self):
        source = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("on:\n  workflow_dispatch:", source)
        self.assertIn("  push:\n    tags: ['v*']", source)
        self.assertIn("if: github.event_name == 'push' || inputs.mode != 'reviewed'", source)
        self.assertIn("if: inputs.mode == 'reviewed'", source)
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
        self.assertIn("module-reviewed-epic-release.yml", source)
        self.assertIn("module-release:", source)
        self.assertIn("default: tag", source)
        self.assertIn("needs: resolve", source)


if __name__ == "__main__":
    unittest.main()
