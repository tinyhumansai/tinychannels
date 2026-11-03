#!/usr/bin/env python3
"""Guard the standalone consumer of pure bus collection declarations."""

import importlib.util
import json
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "proofs/storage-needs-consumer/Cargo.toml"
PROOF_PACKAGE = "tinychannels-storage-needs-consumer"

# Share the contract gate's reviewed serde/schema dependency allowlist and its
# fail-closed normal/build graph traversal. The proof adds only its root crate;
# it must never need a storage driver, module runtime, or transport dependency.
SPEC = importlib.util.spec_from_file_location("bus_purity", ROOT / "scripts/check-bus-purity.py")
BUS_PURITY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUS_PURITY)


def violations(metadata):
    """Require a reachable bus contract and a complete, pure consumer closure."""
    if not isinstance(metadata, dict):
        return ["Cargo metadata is not an object"]
    packages, _, reachable, errors = BUS_PURITY.reachable_normal_build(
        metadata, root_package=PROOF_PACKAGE
    )
    for name in (PROOF_PACKAGE, "tinychannels-bus"):
        package_ids = [package_id for package_id, package in packages.items() if package["name"] == name]
        if len(package_ids) != 1:
            errors.append(f"expected one {name} package, found {len(package_ids)}")
        elif package_ids[0] not in reachable:
            errors.append(f"{name} is missing from the resolved consumer closure")
    allowed = BUS_PURITY.ALLOWED | {PROOF_PACKAGE}
    unsafe = sorted({packages[package_id]["name"] for package_id in reachable} - allowed)
    for name in unsafe:
        errors.append(f"forbidden package in collection declaration graph: {name}")
    return errors


def main():
    try:
        output = subprocess.check_output(
            ["cargo", "metadata", "--locked", "--format-version", "1", "--manifest-path", str(MANIFEST)],
            cwd=ROOT,
            text=True,
        )
        errors = violations(json.loads(output))
    except (subprocess.CalledProcessError, json.JSONDecodeError) as error:
        print(error, file=sys.stderr)
        return 1
    if errors:
        print("storage collection boundary failed:", file=sys.stderr)
        print("\n".join(f"- {error}" for error in errors), file=sys.stderr)
        return 1
    print("storage collection declarations use the standalone pure bus contract")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
