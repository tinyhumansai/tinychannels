#!/usr/bin/env python3
"""Audit contract source and independent normal/build dependency closures."""

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib


ROOT = Path(__file__).resolve().parents[1]
BUS_SOURCE = ROOT / "crates/tinychannels-bus/src"
BUS_MANIFEST = ROOT / "crates/tinychannels-bus/Cargo.toml"
ALLOWED = frozenset({
    "tinychannels-bus", "serde", "serde_core", "serde_derive", "serde_json",
    "schemars", "schemars_derive", "thiserror", "thiserror-impl",
    "proc-macro2", "quote", "syn", "unicode-ident", "itoa", "memchr", "zmij",
    "serde_derive_internals", "dyn-clone", "ref-cast", "ref-cast-impl", "base64",
})
FORBIDDEN_DEPS = frozenset({"anyhow", "async-trait", "tokio"})

# This is the observed default/all-feature normal + build closure. New packages
# require review because this crate is consumed directly by host libraries.
FORBIDDEN_PACKAGES = frozenset({
    "axum", "hyper", "mongodb", "reqwest", "rusqlite", "sqlx",
    "tinybus", "tinybus-macros", "tinybus-module",
    "tinystoragedrivers-api", "tinystoragedrivers-busproxy",
    "tinystoragedrivers-core", "tinystoragedrivers-file",
    "tinystoragedrivers-mongodb", "tinystoragedrivers-s3",
    "tinystoragedrivers-secrets", "tinystoragedrivers-sqlite",
})
NORMAL_BUILD_KINDS = {None, "normal", "build"}

# The contract is synchronous; execution and storage-driver types belong in
# the implementation even when a runtime feature is disabled.
FORBIDDEN_SOURCE = (
    (re.compile(r"\basync\s+fn\b|\basync_trait\b"), "async contract interface or implementation"),
    (re.compile(r"\btokio::"), "Tokio runtime dependency"),
    (re.compile(r"\bstd::sync\b"), "runtime synchronization"),
    (re.compile(r"\btinystoragedrivers_\w*::"), "storage driver dependency"),
    (re.compile(r"\bstd::net\b"), "socket/network implementation"),
    (re.compile(r"\btokio::net\b"), "Tokio network implementation"),
    (re.compile(r"\bstd::fs\b|\btokio::fs\b"), "filesystem implementation"),
    (re.compile(r"\bstd::process\b|\btokio::process\b"), "process execution"),
    (re.compile(r"\bstd::env\b"), "process environment access"),
    (re.compile(r"\bstd::time\b"), "clock access"),
    (re.compile(r"\bstd::thread\b"), "thread execution"),
    (re.compile(r"\bstd::sync::(?:mpsc|atomic)::"), "thread/IPC coordination"),
    (re.compile(r"\bparking_lot::"), "runtime synchronization"),
    (re.compile(r"\btokio::(?:spawn|task::|time::)"), "Tokio task/time execution"),
    (
        re.compile(
            r"\btokio::sync::(?:Mutex|RwLock|OnceCell|Notify|oneshot::|watch::|broadcast::)"
        ),
        "Tokio runtime synchronization",
    ),
    (
        re.compile(r"\b(?:TcpListener|TcpStream|UdpSocket|UnixListener|UnixStream)\b"),
        "socket type",
    ),
    (
        re.compile(r"\b(?:rusqlite|mongodb|sqlx|reqwest|hyper|axum)::"),
        "database or HTTP implementation",
    ),
    (re.compile(r"\bimpl\s+Channel\s+for\b"), "concrete provider executor"),
    (
        re.compile(r"\b(?:sha2|hmac|rand|uuid)::"),
        "cryptographic, random, or identity implementation",
    ),
    (
        re.compile(
            r"\bfn\s+(?:build_session_key|classify_send_error|"
            r"authenticated_inbound_event|segment_for_delivery)\b"
        ),
        "implementation algorithm",
    ),
)


def source_violations(source_text, source_path=None):
    """Return runtime/implementation constructs found in one Rust source."""
    errors = [
        label
        for pattern, label in FORBIDDEN_SOURCE
        if pattern.search(source_text)
    ]
    return errors


def source_tree_violations(source_root=None):
    """Scan production Rust files while leaving colocated behavior tests alone."""
    source_root = source_root if source_root is not None else ROOT / "crates/tinychannels-bus/src"
    errors = []
    storage = Path(source_root) / "storage"
    for name in ("migrate.rs", "migrate_tests.rs"):
        if (storage / name).exists():
            errors.append(f"{(storage / name).relative_to(ROOT)}: migration execution belongs to tinychannels")
    records = storage / "records.rs"
    if records.exists():
        text = records.read_text(encoding="utf-8")
        if "impl From<" in text or "to_stored_ms" in text or "from_stored_ms" in text:
            errors.append(f"{records.relative_to(ROOT)}: storage projections belong to tinychannels")
    for source in sorted(Path(source_root).rglob("*.rs")):
        if source.name.endswith("_tests.rs"):
            continue
        for violation in source_violations(
            source.read_text(encoding="utf-8"), source_path=source
        ):
            errors.append(f"{source.relative_to(ROOT)}: {violation}")
    return errors


def reachable_normal_build(metadata, root_package="tinychannels-bus"):
    """Resolve the bus package's complete normal/build closure, fail closed."""
    errors = []
    packages = metadata.get("packages")
    resolve = metadata.get("resolve")
    if not isinstance(packages, list) or not isinstance(resolve, dict):
        return {}, {}, set(), ["Cargo metadata is missing packages or resolve data"]

    packages_by_id = {}
    for package in packages:
        if (
            not isinstance(package, dict)
            or not isinstance(package.get("id"), str)
            or not isinstance(package.get("name"), str)
        ):
            errors.append("Cargo metadata contains a malformed package record")
            continue
        if package["id"] in packages_by_id:
            errors.append(f"Cargo metadata repeats package ID {package['id']}")
        packages_by_id[package["id"]] = package

    nodes = resolve.get("nodes")
    if not isinstance(nodes, list):
        return packages_by_id, {}, set(), errors + [
            "Cargo metadata is missing resolved nodes"
        ]
    nodes_by_id = {}
    for node in nodes:
        if not isinstance(node, dict) or not isinstance(node.get("id"), str):
            errors.append("Cargo resolve graph contains a malformed node")
            continue
        if node["id"] in nodes_by_id:
            errors.append(f"Cargo resolve graph repeats node {node['id']}")
        nodes_by_id[node["id"]] = node

    bus_ids = [
        package_id
        for package_id, package in packages_by_id.items()
        if package["name"] == root_package
    ]
    if len(bus_ids) != 1:
        return packages_by_id, nodes_by_id, set(), errors + [
            f"expected one {root_package} package, found {len(bus_ids)}"
        ]

    reachable = set()
    pending = list(bus_ids)
    while pending:
        package_id = pending.pop()
        if package_id in reachable:
            continue
        package = packages_by_id.get(package_id)
        node = nodes_by_id.get(package_id)
        if package is None or node is None:
            errors.append(f"missing resolved normal/build node or package record for {package_id}")
            continue
        reachable.add(package_id)
        if not isinstance(node.get("features"), list) or any(
            not isinstance(feature, str) for feature in node.get("features", [])
        ):
            errors.append(f"resolved node {package_id} has no feature evidence")
        dependencies = node.get("deps")
        if not isinstance(dependencies, list):
            errors.append(f"resolved node {package_id} has no dependency evidence")
            continue
        for dependency in dependencies:
            if not isinstance(dependency, dict):
                errors.append(f"resolved node {package_id} has a malformed dependency")
                continue
            kinds = dependency.get("dep_kinds")
            if not isinstance(kinds, list) or not kinds:
                errors.append(f"resolved dependency from {package_id} has no kind evidence")
                continue
            if any(
                not isinstance(kind, dict)
                or "kind" not in kind
                or kind.get("kind") not in (None, "normal", "dev", "build")
                for kind in kinds
            ):
                errors.append(f"resolved dependency from {package_id} has malformed kind evidence")
                continue
            if not any(kind.get("kind") in NORMAL_BUILD_KINDS for kind in kinds):
                continue
            child = dependency.get("pkg")
            if not isinstance(child, str):
                errors.append(f"resolved dependency from {package_id} has no package ID")
            elif child not in nodes_by_id or child not in packages_by_id:
                errors.append(
                    f"missing resolved normal/build node for "
                    f"{dependency.get('name', child)} ({child})"
                )
            else:
                pending.append(child)

    return packages_by_id, nodes_by_id, reachable, errors


def dependency_violations(metadata):
    packages_by_id, _, reachable, errors = reachable_normal_build(metadata)
    packages = [packages_by_id[package_id] for package_id in reachable]
    names = {package["name"] for package in packages}
    unsafe = sorted(names - ALLOWED)
    if unsafe:
        errors.append(f"unsafe contract dependencies: {unsafe}")
    return errors


def declared_forbidden_dependencies(manifest):
    """Reject runtime/TSD declarations, including unused optional and dev edges."""
    tables = [manifest, *manifest.get("target", {}).values()]
    found = set()
    for table in tables:
        for section in ("dependencies", "dev-dependencies", "build-dependencies"):
            for alias, spec in table.get(section, {}).items():
                package = spec.get("package", alias) if isinstance(spec, dict) else alias
                if package in FORBIDDEN_DEPS or package in FORBIDDEN_PACKAGES or package.startswith("tinystoragedrivers-"):
                    found.add(package)
    return found


def contract_features(manifest):
    """Include Cargo's implicit optional-dependency features in the all probe."""
    explicit = manifest.get("features", {})
    features = set(explicit)
    suppressed = {
        member.removeprefix("dep:")
        for members in explicit.values()
        for member in members
        if member.startswith("dep:")
    }
    for table in [manifest, *manifest.get("target", {}).values()]:
        for alias, spec in table.get("dependencies", {}).items():
            if isinstance(spec, dict) and spec.get("optional") and alias not in suppressed:
                features.add(alias)
    return sorted(features)


def run_metadata(manifest, offline=False):
    command = ["cargo", "metadata"]
    if offline:
        command.append("--offline")
    command.extend(["--format-version=1", "--manifest-path", str(manifest)])
    output = subprocess.check_output(command, cwd=ROOT, text=True)
    return json.loads(output)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true", help="Use only cached Cargo registry data")
    args = parser.parse_args(argv)

    source_errors = source_tree_violations()
    if source_errors:
        print("unsafe contract source:", file=sys.stderr)
        print("\n".join(f"- {error}" for error in source_errors), file=sys.stderr)
        return 1

    manifest = tomllib.loads(BUS_MANIFEST.read_text(encoding="utf-8"))
    forbidden_direct = declared_forbidden_dependencies(manifest)
    if forbidden_direct:
        print(f"contract manifest declares runtime dependencies: {sorted(forbidden_direct)}", file=sys.stderr)
        return 1
    features = contract_features(manifest)
    modes = ["default", "all"]
    if "storage" in features:
        modes.append("storage")
    for mode in modes:
        probe = ROOT / "target/bus-purity" / mode
        (probe / "src").mkdir(parents=True, exist_ok=True)
        (probe / "src/lib.rs").write_text("pub use tinychannels_bus::*;\n", encoding="utf-8")
        dependency = (
            "tinychannels-bus = { path = "
            + json.dumps(str(ROOT / "crates/tinychannels-bus"))
            + (
                ", features = " + json.dumps(features)
                if mode == "all"
                else ', features = ["storage"]'
                if mode == "storage"
                else ""
            )
            + " }\n"
        )
        (probe / "Cargo.toml").write_text(
            '[package]\nname="contract-purity-probe"\nversion="0.0.0"\nedition="2024"\n'
            "[workspace]\n[dependencies]\n"
            + dependency,
            encoding="utf-8",
        )
        metadata = run_metadata(probe / "Cargo.toml", offline=args.offline)
        errors = dependency_violations(metadata)
        if errors:
            print(f"{mode}: contract dependency boundary failed:", file=sys.stderr)
            print("\n".join(f"- {error}" for error in errors), file=sys.stderr)
            return 1
        print(f"{mode}: pure contract closure ({len(reachable_normal_build(metadata)[2])} packages)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())