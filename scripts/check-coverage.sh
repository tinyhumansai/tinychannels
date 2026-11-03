#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$root"
scripts/test-client-e2e.sh --prepare-only
source target/tinychannels-client-e2e.env
.github/scripts/check-file-coverage.sh 90 coverage.json
