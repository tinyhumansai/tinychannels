#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
target="$(rustc -vV | sed -n 's/^host: //p')"
version="$(cargo metadata --manifest-path Cargo.toml --format-version 1 --no-deps | jq -er '.packages[] | select(.name == "tinychannels-module") | .version')"
case "${RUNNER_OS:-$(uname -s)}" in
  Windows|MINGW*|MSYS*|CYGWIN*)
    pwsh -NoProfile -File scripts/test-e2e.ps1 -Target "$target" -Version "$version"
    ;;
  *)
    scripts/build-module.sh --crate tinychannels-module --version "$version" --target "$target" --host ci --output-dir target/module-e2e
    scripts/verify-module.sh --archive "target/module-e2e/tinychannels-module-$version-ci.tar.gz"
    bash scripts/test-client-e2e.sh
    ;;
esac
