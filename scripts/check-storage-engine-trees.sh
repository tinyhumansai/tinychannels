#!/usr/bin/env bash
set -euo pipefail

blocked='^(rusqlite|libsqlite3-sys|mongodb|bson|aws-sdk-s3|lapin|amq-protocol|sqlx|redb|sled|rocksdb|tinystoragedrivers-(core|file|mongodb|s3|sqlite|document|kv|blob|queue|secrets|connect)|tinystoragedrivers)$'

check_tree() {
  package="$1"
  names="$2"
  if [[ -z "$names" ]] || ! printf '%s\n' "$names" | grep -Fxq "$package"; then
    printf 'failed closed: cargo tree for %s did not contain its root package\n' "$package" >&2
    return 1
  fi
  offender="$(printf '%s\n' "$names" | grep -Ex "$blocked" || true)"
  if [[ -n "$offender" ]]; then
    # SQLite is the one deliberate feature exception: the implementation's
    # opt-in iMessage reader uses rusqlite and ships in the module artifact.
    # Confine it to those implementations; the library's no-default-features
    # build below must remain SQLite-free.
    if [[ ( "$package" == "tinychannels" || "$package" == "tinychannels-module" ) && "${ALLOW_IMESSAGE_SQLITE:-0}" == 1 ]]; then
      offender="$(printf '%s\n' "$offender" | grep -Ev '^(rusqlite|libsqlite3-sys)$' || true)"
    fi
  fi
  if [[ -n "$offender" ]]; then
    printf '%s has a normal/build dependency on a storage engine: %s\n' "$package" "$offender" >&2
    return 1
  fi
}

library_packages() {
  python3 -c '
import json, sys
data = json.load(sys.stdin)
members = set(data["workspace_members"])
library_kinds = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}
for package in data["packages"]:
    if package["id"] in members and any(library_kinds.intersection(target["kind"]) for target in package["targets"]):
        print(package["name"])
'
}

normalize_names() {
  # Git Bash on Windows can leave CRLF in captured cargo output.
  tr -d '\r' | awk 'NF {print $1}'
}

tree_names() {
  cargo tree --locked "$@" --prefix none | normalize_names
}

if [[ "${1:-}" == "--self-test" ]]; then
  check_tree tinychannels $'tinychannels\nserde\ntinystoragedrivers-api\ntinystoragedrivers-types'
  if check_tree tinychannels $'tinychannels\nserde\ntinystoragedrivers-document'; then
    printf 'self-test failed: a document engine was accepted\n' >&2
    exit 1
  fi
  if check_tree tinychannels $'tinychannels\nserde\ntinystoragedrivers'; then
    printf 'self-test failed: the facade package was accepted\n' >&2
    exit 1
  fi
  if check_tree tinychannels $'serde\ntinystoragedrivers-api'; then
    printf 'self-test failed: a missing root package was accepted\n' >&2
    exit 1
  fi
  if check_tree tinychannels $'tinychannels\nserde\nrusqlite\nlibsqlite3-sys'; then
    printf 'self-test failed: the iMessage SQLite exception was not enabled\n' >&2
    exit 1
  fi
  if ALLOW_IMESSAGE_SQLITE=1 check_tree tinychannels $'tinychannels\nserde\nrusqlite\nlibsqlite3-sys'; then
    :
  else
    printf 'self-test failed: the explicit iMessage SQLite exception was rejected\n' >&2
    exit 1
  fi
  if ALLOW_IMESSAGE_SQLITE=1 check_tree tinychannels $'tinychannels\nserde\nrusqlite\nlibsqlite3-sys\nmongodb'; then
    printf 'self-test failed: the SQLite exception accepted an unrelated offender\n' >&2
    exit 1
  fi
  for engine in core file mongodb s3 sqlite; do
    if check_tree tinychannels "$(printf 'tinychannels\ntinystoragedrivers-%s\n' "$engine")"; then
      printf 'self-test failed: TSD implementation %s was accepted\n' "$engine" >&2
      exit 1
    fi
  done
  ALLOW_IMESSAGE_SQLITE=1 check_tree tinychannels-module $'tinychannels-module\nrusqlite\nlibsqlite3-sys'
  if ALLOW_IMESSAGE_SQLITE=1 check_tree tinychannels-module $'tinychannels-module\nrusqlite\ntinystoragedrivers-sqlite'; then
    printf 'self-test failed: module iMessage exception accepted a storage engine\n' >&2
    exit 1
  fi
  if ALLOW_IMESSAGE_SQLITE=1 check_tree tinychannels-bus $'tinychannels-bus\nrusqlite'; then
    printf 'self-test failed: iMessage exception escaped to the bus contract\n' >&2
    exit 1
  fi
  metadata='{"workspace_members":["lib-id","rlib-id","dylib-id","cdylib-id","staticlib-id","proc-macro-id","binary-id"],"packages":[{"id":"lib-id","name":"lib-only","targets":[{"kind":["lib"]}]},{"id":"rlib-id","name":"rlib-only","targets":[{"kind":["rlib"]}]},{"id":"dylib-id","name":"dylib-only","targets":[{"kind":["dylib"]}]},{"id":"cdylib-id","name":"cdylib-only","targets":[{"kind":["cdylib"]}]},{"id":"staticlib-id","name":"staticlib-only","targets":[{"kind":["staticlib"]}]},{"id":"proc-macro-id","name":"proc-macro-only","targets":[{"kind":["proc-macro"]}]},{"id":"binary-id","name":"binary-only","targets":[{"kind":["bin"]}]},{"id":"external-id","name":"external-dylib","targets":[{"kind":["dylib"]}]}]}'
  selected="$(printf '%s' "$metadata" | library_packages)"
  expected=$'lib-only\nrlib-only\ndylib-only\ncdylib-only\nstaticlib-only\nproc-macro-only'
  if [[ "$selected" != "$expected" ]]; then
    printf 'self-test failed: library enumeration did not select all six native library kinds by workspace member ID\n' >&2
    exit 1
  fi
  empty_metadata='{"workspace_members":[],"packages":[]}'
  if [[ -n "$(printf '%s' "$empty_metadata" | library_packages)" ]]; then
    printf 'self-test failed: empty workspace metadata selected a package\n' >&2
    exit 1
  fi
  normalized="$(printf 'tinychannels\r\nserde\r\n' | normalize_names)"
  if [[ "$normalized" != $'tinychannels\nserde' ]]; then
    printf 'self-test failed: CRLF cargo output was not normalized\n' >&2
    exit 1
  fi
  printf 'storage engine tree gate self-test passed\n'
  exit 0
fi

metadata="$(cargo metadata --locked --format-version 1 --no-deps | tr -d '\r')"
packages="$(printf '%s' "$metadata" | library_packages)"
if [[ -z "$packages" ]]; then
  printf 'failed closed: workspace metadata contained no native library packages\n' >&2
  exit 1
fi

while IFS= read -r package; do
  [[ -n "$package" ]] || continue
  names="$(tree_names -p "$package" --all-features -e normal,build)"
  if [[ "$package" == "tinychannels" ]]; then
    ALLOW_IMESSAGE_SQLITE=1 check_tree "$package" "$names"
    default_names="$(tree_names -p "$package" --no-default-features -e normal,build)"
    if printf '%s\n' "$default_names" | grep -Ex '^(rusqlite|libsqlite3-sys)$' >/dev/null; then
      printf 'tinychannels no-default-features build unexpectedly includes SQLite\n' >&2
      exit 1
    fi
    check_tree "$package" "$default_names"
  elif [[ "$package" == "tinychannels-module" ]]; then
    ALLOW_IMESSAGE_SQLITE=1 check_tree "$package" "$names"
  else
    check_tree "$package" "$names"
  fi
done <<< "$packages"

printf 'storage engine dependency boundary passed for all native library packages\n'
