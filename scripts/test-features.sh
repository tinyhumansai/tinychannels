#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
python3 scripts/test-e2e-version.py
python3 scripts/check-webpki-roots.py --self-test
python3 scripts/check-webpki-roots.py
python3 scripts/check-bus-purity.py
python3 scripts/__tests__/check_bus_purity_tests.py
# Native Windows Python resolves bare bash to the WSL launcher on some runners.
# Reuse this Git Bash executable, converting its MSYS path for native Python.
feature_test_bash="$BASH"
case "${RUNNER_OS:-$(uname -s)}" in
  Windows|MINGW*|MSYS*|CYGWIN*) feature_test_bash="$(cygpath -m "$BASH")" ;;
esac
FEATURE_TEST_BASH="$feature_test_bash" python3 scripts/test-feature-lanes.py
FEATURE_TEST_BASH="$feature_test_bash" python3 scripts/test-reviewed-epic-release.py
python3 scripts/test-bus-purity.py
python3 scripts/check-bus-purity.py
python3 scripts/test-feature-shell.py
python3 scripts/test-client-native-topology.py
FEATURE_TEST_BASH="$feature_test_bash" python3 scripts/test-client-e2e-standalone.py
cargo test --locked --workspace
if [[ "${FEATURE_TEST_SKIP_CLIENT_PREPARE:-0}" != "1" ]]; then
  scripts/test-client-e2e.sh --prepare-only
  source target/tinychannels-client-e2e.env
fi
cargo test --locked --workspace --all-features
cargo test --locked -p tinychannels --no-default-features
cargo test --locked -p tinychannels --no-default-features --features email-send
case "${RUNNER_OS:-$(uname -s)}" in
  Windows|MINGW*|MSYS*|CYGWIN*) ;;
  *) python3 scripts/test-package-layout.py ;;
esac
