#!/usr/bin/env python3
"""Exercise the client E2E script from a standalone checkout with fake Cargo."""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import shutil
import subprocess


ROOT = Path(__file__).resolve().parent.parent
WORK = ROOT / "target" / "evidence" / "client-e2e-standalone-smoke"


def fail(message: str) -> None:
    raise SystemExit(message)


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    path.chmod(0o755)


def main() -> None:
    bash_executable = os.environ.get("FEATURE_TEST_BASH", "bash")
    if WORK.exists():
        shutil.rmtree(WORK)
    repo = WORK / "tinychannels"
    (repo / "scripts").mkdir(parents=True)
    shutil.copy2(ROOT / "Cargo.toml", repo / "Cargo.toml")
    shutil.copy2(ROOT / "scripts" / "test-client-e2e.sh", repo / "scripts")
    shutil.copy2(ROOT / "scripts" / "test-client-e2e.ps1", repo / "scripts")
    shutil.copytree(
        ROOT / "crates" / "tinychannels-client" / "test-fixture",
        repo / "crates" / "tinychannels-client" / "test-fixture",
        ignore=shutil.ignore_patterns("src", "target"),
    )

    fake_bin = WORK / "bin"
    write(
        fake_bin / "rustc",
        "#!/bin/sh\nprintf '%s\\n' 'host: aarch64-apple-darwin'\n",
    )
    write(
        fake_bin / "uname",
        "#!/bin/sh\nprintf '%s\\n' \"${TINYCHANNELS_FAKE_UNAME:-Darwin}\"\n",
    )
    pwsh_stub = fake_bin / "pwsh_stub.py"
    write(
        pwsh_stub,
        """#!/usr/bin/env python3
import json
import os
import sys
from pathlib import Path

with Path(os.environ['TINYCHANNELS_FAKE_PWSH_LOG']).open('a', encoding='utf-8') as stream:
    stream.write(json.dumps(sys.argv[1:]) + '\\n')
""",
    )
    write(fake_bin / "pwsh", f"#!/bin/sh\nexec python3 '{pwsh_stub}' \"$@\"\n")
    cargo_stub = fake_bin / "cargo_stub.py"
    write(
        cargo_stub,
        """#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

args = sys.argv[1:]
root = Path(os.environ['TINYCHANNELS_FAKE_REPO'])
target = Path(os.environ['TINYCHANNELS_FAKE_TARGET'])
record = {
    'args': args,
    'fixture_cache': os.environ.get('TINYCHANNELS_CLIENT_TEST_CACHE'),
    'native_cache': os.environ.get('TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST_CACHE'),
}
with Path(os.environ['TINYCHANNELS_FAKE_CARGO_LOG']).open('a', encoding='utf-8') as stream:
    stream.write(json.dumps(record) + '\\n')
if not args:
    raise SystemExit('fake cargo received no command')
if args[0] == 'build':
    output = target / 'debug'
    output.mkdir(parents=True, exist_ok=True)
    library = 'libtinychannels_module.dylib' if '-p' in args else 'libtinychannels_client_test_fixture.dylib'
    (output / library).write_bytes(b'fake native library: ' + library.encode())
elif args[0] == 'test':
    test_name = args[args.index('--test') + 1]
    expected = {'native_e2e': 4, 'runtime_native_e2e': 1, 'runtime_readiness_e2e': 1, 'native_module_runtime_e2e': 1}[test_name]
    if os.environ.get('TINYCHANNELS_FAKE_ZERO_TEST_TARGET') == test_name:
        expected = 0
    print(f'test result: ok. {expected} passed; 0 failed; 0 ignored; 0 measured; 0 filtered out')
else:
    raise SystemExit(f'unexpected fake cargo command: {args[0]}')
""",
    )
    write(fake_bin / "cargo", f"#!/bin/sh\nexec python3 '{cargo_stub}' \"$@\"\n")

    cargo_log = WORK / "cargo.jsonl"
    pwsh_log = WORK / "pwsh.jsonl"
    env = os.environ.copy()
    env.update(
        {
            "PATH": f"{fake_bin}{os.pathsep}{env['PATH']}",
            "RUNNER_OS": "",
            "TINYCHANNELS_FAKE_REPO": str(repo),
            "TINYCHANNELS_FAKE_TARGET": str(repo / "target"),
            "TINYCHANNELS_FAKE_CARGO_LOG": str(cargo_log),
            "TINYCHANNELS_FAKE_PWSH_LOG": str(pwsh_log),
            "TINYCHANNELS_FAKE_UNAME": "Darwin",
        }
    )
    result = subprocess.run(
        [bash_executable, str(repo / "scripts" / "test-client-e2e.sh")],
        cwd=repo,
        env=env,
        check=False,
        capture_output=True,
        text=True,
    )
    if result.returncode:
        fail(
            "standalone Bash E2E script failed with fake Cargo "
            f"(exit {result.returncode}):\n{result.stdout}{result.stderr}"
        )
    if pwsh_log.exists():
        fail("Unix fake-Cargo smoke unexpectedly delegated to PowerShell")

    bash_script = (ROOT / "scripts" / "test-client-e2e.sh").read_text(encoding="utf-8")
    powershell_script = (ROOT / "scripts" / "test-client-e2e.ps1").read_text(
        encoding="utf-8"
    )
    for label, script in (("Bash", bash_script), ("PowerShell", powershell_script)):
        if "ci-cancel-aware.sh" in script or "../../scripts" in script:
            fail(f"{label} client E2E script depends on files outside its standalone repository")
    if re.search(r"\$cancelAware|&\s*bash\s+\$cancelAware", powershell_script):
        fail("PowerShell client E2E script still wraps Cargo through an external helper")
    if len(re.findall(r"(?m)^cargo\s+build\b", bash_script)) != 2 or "cargo test" not in bash_script:
        fail("Bash client E2E script must build both fixtures and run counted native tests")
    if len(re.findall(r"(?m)^&\s+cargo build\b", powershell_script)) != 2 or "& cargo @Arguments" not in powershell_script:
        fail("PowerShell client E2E script must build both fixtures and run counted native tests")

    calls = [json.loads(line) for line in cargo_log.read_text(encoding="utf-8").splitlines()]
    if len(calls) != 6 or [call["args"][0] for call in calls] != [
        "build",
        "build",
        "test",
        "test",
        "test",
        "test",
    ]:
        fail(f"expected two builds and three test commands, captured: {calls!r}")

    expected_tests = ["native_e2e", "runtime_native_e2e", "runtime_readiness_e2e", "native_module_runtime_e2e"]
    for call, expected in zip(calls[2:], expected_tests, strict=True):
        args = call["args"]
        if "--test" not in args or args[args.index("--test") + 1] != expected:
            fail(f"expected Cargo test target {expected}, captured: {args!r}")
        manifest = Path(args[args.index("--manifest-path") + 1])
        if manifest != repo / "Cargo.toml":
            fail(f"Cargo test escaped the standalone repository: {manifest}")
        for key in ("fixture_cache", "native_cache"):
            if not call[key] or not Path(call[key]).is_relative_to(repo / "target"):
                fail(f"Cargo test {key} is missing or outside the standalone target")

    expected_outputs = [
        repo / "target" / "tinychannels-client-e2e" / "cache" / "tinychannels-module-client-test.tar.gz",
        repo / "target" / "tinychannels-module-client-native-e2e" / "cache" / "tinychannels-module-native-test.tar.gz",
    ]
    for output in expected_outputs:
        if not output.is_file():
            fail(f"expected local staged module archive: {output}")
    expected_cargo_call_count = len(calls)

    windows_delegations = (
        ("MINGW64_NT-10.0", True),
        ("MSYS_NT-10.0", True),
        ("CYGWIN_NT-10.0", False),
    )
    for platform, prepare_only in windows_delegations:
        windows_env = dict(env, TINYCHANNELS_FAKE_UNAME=platform)
        windows_env.pop("RUNNER_OS", None)
        command = [bash_executable, str(repo / "scripts" / "test-client-e2e.sh")]
        if prepare_only:
            command.append("--prepare-only")
        delegated = subprocess.run(
            command,
            cwd=repo,
            env=windows_env,
            check=False,
            capture_output=True,
            text=True,
        )
        if delegated.returncode:
            fail(
                f"RUNNER_OS-unset {platform} delegation failed "
                f"(exit {delegated.returncode}): {delegated.stdout}{delegated.stderr}"
            )
        calls = [json.loads(line) for line in pwsh_log.read_text(encoding="utf-8").splitlines()]
        expected_args = ["-NoProfile", "-File", "scripts/test-client-e2e.ps1"]
        if prepare_only:
            expected_args.append("-PrepareOnly")
        if calls[-1:] != [expected_args]:
            fail(f"RUNNER_OS-unset {platform} delegated unexpected PowerShell args: {calls!r}")
        after_calls = [
            json.loads(line) for line in cargo_log.read_text(encoding="utf-8").splitlines()
        ]
        if len(after_calls) != expected_cargo_call_count:
            fail(f"RUNNER_OS-unset {platform} delegation unexpectedly invoked fake Cargo")
    environment_file = repo / "target" / "tinychannels-client-e2e.env"
    if not environment_file.is_file():
        fail("client fixture preparation did not write its reusable environment file")
    environment_bytes = environment_file.read_bytes()
    environment = environment_bytes.decode("utf-8")
    if b"\r" in environment_bytes:
        fail("prepared client fixture environment must use LF-only line endings")
    for key in (
        "TINYCHANNELS_CLIENT_TEST_CACHE",
        "TINYCHANNELS_CLIENT_TEST_ARCHIVE_NAME",
        "TINYCHANNELS_CLIENT_TEST_ARCHIVE_SHA256",
        "TINYCHANNELS_CLIENT_TEST_LIBRARY_SHA256",
        "TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST_CACHE",
        "TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST_ARCHIVE_NAME",
        "TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST_ARCHIVE_SHA256",
        "TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST_LIBRARY_SHA256",
        "TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST_RELEASE_URL",
    ):
        if f"export {key}=" not in environment:
            fail(f"client fixture environment omits {key}")

    source_env = """set -e
source "$1"
printf '%s\\0' \\
  "$TINYCHANNELS_CLIENT_TEST_CACHE" \\
  "$TINYCHANNELS_CLIENT_TEST_ARCHIVE_NAME" \\
  "$TINYCHANNELS_CLIENT_TEST_ARCHIVE_SHA256" \\
  "$TINYCHANNELS_CLIENT_TEST_LIBRARY_SHA256" \\
  "$TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST_CACHE" \\
  "$TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST_ARCHIVE_NAME" \\
  "$TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST_ARCHIVE_SHA256" \\
  "$TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST_LIBRARY_SHA256" \\
  "$TINYCHANNELS_CLIENT_NATIVE_MODULE_TEST_RELEASE_URL"
"""
    sourced_env = subprocess.run(
        [bash_executable, "-c", source_env, "client-env-test", "target/tinychannels-client-e2e.env"],
        cwd=repo,
        check=False,
        capture_output=True,
    )
    if sourced_env.returncode:
        fail(f"Bash could not source the generated client fixture environment: {sourced_env.stderr!r}")
    values = sourced_env.stdout.decode("utf-8").split("\0")[:-1]
    if len(values) != 9 or any("\r" in value for value in values):
        fail(f"sourced client fixture environment has missing or CR-suffixed values: {values!r}")
    normalized_cache = values[0].replace("\\", "/")
    normalized_native_cache = values[4].replace("\\", "/")
    if not normalized_cache.endswith("/target/tinychannels-client-e2e/cache"):
        fail(f"Bash sourced an incorrect fixture cache path: {values[0]!r}")
    if values[1] != "tinychannels-module-client-test.tar.gz" or values[5] != "tinychannels-module-native-test.tar.gz":
        fail("Bash sourced an unexpected local fixture archive name")
    if not normalized_native_cache.endswith("/target/tinychannels-module-client-native-e2e/cache"):
        fail(f"Bash sourced an incorrect production module cache path: {values[4]!r}")
    if values[8] != "https://github.com/tinyhumansai/tinychannels/releases/tag/v0.2.0":
        fail("Bash sourced an unexpected local module release URL")
    if any(not re.fullmatch(r"[0-9a-f]{64}", values[index]) for index in (2, 3, 6, 7)):
        fail("Bash sourced an invalid archive or library digest")

    crlf_value = r"D:\a\repo\target\fixture-cache"
    crlf_line_env = repo / "target" / "windows-crlf-line-ending-control.env"
    crlf_line_env.write_bytes(f"export CLIENT_TEST_VALUE='{crlf_value}'\r\n".encode("utf-8"))
    crlf_line_result = subprocess.run(
        [bash_executable, "-c", 'source "$1"; printf "%s" "$CLIENT_TEST_VALUE"',
         "client-env-crlf-line-ending-control", "target/windows-crlf-line-ending-control.env"],
        cwd=repo,
        check=False,
        capture_output=True,
    )
    crlf_prefix = crlf_value.encode("utf-8")
    if crlf_line_result.returncode or not crlf_line_result.stdout.startswith(crlf_prefix):
        fail(f"Bash CRLF line-ending control changed the value prefix: {crlf_line_result.stdout!r}")
    trailing_cr = crlf_line_result.stdout[len(crlf_prefix) :]
    if trailing_cr not in (b"", b"\r"):
        fail(f"Bash CRLF line-ending control had unexpected trailing bytes: {trailing_cr!r}")

    embedded_cr_env = repo / "target" / "embedded-cr-control.env"
    embedded_cr_env.write_text(
        f"CLIENT_TEST_BASE='{crlf_value}'\n"
        "printf -v CLIENT_TEST_VALUE '%s\\r' \"$CLIENT_TEST_BASE\"\n"
        "export CLIENT_TEST_VALUE\n",
        encoding="utf-8",
        newline="\n",
    )
    embedded_cr_result = subprocess.run(
        [bash_executable, "-c", 'source "$1"; printf "%s" "$CLIENT_TEST_VALUE"',
         "client-env-embedded-cr-control", "target/embedded-cr-control.env"],
        cwd=repo,
        check=False,
        capture_output=True,
    )
    if embedded_cr_result.returncode or embedded_cr_result.stdout.decode("utf-8") != crlf_value + "\r":
        fail("Bash embedded-CR control did not preserve the corrupt environment value")
    print(
        "Bash CRLF trailing-byte repr: "
        f"{trailing_cr!r}; embedded-CR control: retained; prepared env: LF-only"
    )

    prepare_result = subprocess.run(
        [bash_executable, str(repo / "scripts" / "test-client-e2e.sh"), "--prepare-only"],
        cwd=repo,
        env=env,
        check=False,
        capture_output=True,
        text=True,
    )
    if prepare_result.returncode:
        fail(f"prepare-only client E2E mode failed: {prepare_result.stdout}{prepare_result.stderr}")
    prepare_calls = [json.loads(line) for line in cargo_log.read_text(encoding="utf-8").splitlines()]
    if len(prepare_calls) != 8 or [call["args"][0] for call in prepare_calls[-2:]] != ["build", "build"]:
        fail("prepare-only client E2E mode must build both local fixtures without running tests")

    empty_env = dict(env, TINYCHANNELS_FAKE_ZERO_TEST_TARGET="native_e2e")
    empty_result = subprocess.run(
        [bash_executable, str(repo / "scripts" / "test-client-e2e.sh")],
        cwd=repo,
        env=empty_env,
        check=False,
        capture_output=True,
        text=True,
    )
    if empty_result.returncode == 0 or "expected 4" not in empty_result.stderr:
        fail("client E2E runner accepted a successful Cargo command that reported zero native tests")

    print("standalone client E2E scripts use checkout-local files; count guard rejected zero native tests")
    print("RUNNER_OS-unset Git Bash simulation delegated MINGW/MSYS/CYGWIN to PowerShell")


if __name__ == "__main__":
    main()
