"""Negative controls for the standalone, storage-driver-free bus consumer."""

import importlib.util
import json
from pathlib import Path
import unittest
from contextlib import redirect_stdout
from io import StringIO
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("check-storage-needs-boundary.py")
SPEC = importlib.util.spec_from_file_location("storage_boundary", SCRIPT)
BOUNDARY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BOUNDARY)


def fixture(*, extra=(), kind=None):
    packages = [
        {"id": "proof", "name": "tinychannels-storage-needs-consumer", "version": "0.1.0"},
        {"id": "bus", "name": "tinychannels-bus", "version": "0.2.0"},
        {"id": "serde", "name": "serde", "version": "1.0.0"},
        *extra,
    ]
    nodes = [
        {"id": "proof", "features": [], "deps": [
            {"pkg": "bus", "dep_kinds": [{"kind": None}]},
        ]},
        {"id": "bus", "features": ["storage"], "deps": [
            {"pkg": "serde", "dep_kinds": [{"kind": None}]},
        ]},
        {"id": "serde", "features": [], "deps": []},
    ]
    for package in extra:
        nodes[1]["deps"].append({"pkg": package["id"], "dep_kinds": [{"kind": kind}]})
        nodes.append({"id": package["id"], "features": [], "deps": []})
    return {"packages": packages, "resolve": {"nodes": nodes}}


class StorageBoundaryTests(unittest.TestCase):
    def test_pure_bus_collection_declarations_need_no_storage_driver(self):
        self.assertEqual(BOUNDARY.violations(fixture()), [])

    def test_storage_clients_types_and_runtime_packages_are_rejected(self):
        for name in (
            "tinystoragedrivers-busproxy", "tinystoragedrivers-core",
            "tinystoragedrivers-types", "tinychannels-runtime", "tinychannels",
            "tinychannels-module", "tinybus", "tokio", "async-trait", "reqwest",
        ):
            with self.subTest(package=name):
                errors = BOUNDARY.violations(fixture(extra=[{"id": "extra", "name": name}]))
                self.assertTrue(any(name in error for error in errors))

    def test_storage_types_are_rejected_for_every_version_and_source(self):
        for version in ("1.9.0", "2.1.2"):
            for source in (
                "git+https://github.com/senamakel/tinystoragedrivers?rev=c447da3#c447da3",
                "git+https://github.com/tinyhumansai/tinystoragedrivers?rev=bf1f0a97fbe6165a6edd9521659c7f6859700761#bf1f0a97",
            ):
                with self.subTest(version=version, source=source):
                    errors = BOUNDARY.violations(fixture(extra=[{
                        "id": "types", "name": "tinystoragedrivers-types",
                        "version": version, "source": source,
                    }]))
                    self.assertTrue(any("tinystoragedrivers-types" in error for error in errors))

    def test_unreviewed_dependency_is_rejected(self):
        self.assertTrue(BOUNDARY.violations(fixture(extra=[{"id": "extra", "name": "unknown-driver"}])))

    def test_build_dependencies_are_audited_and_dev_edges_do_not_change_the_contract(self):
        for kind in (None, "build", "dev"):
            with self.subTest(kind=kind):
                errors = BOUNDARY.violations(fixture(extra=[{"id": "runtime", "name": "tokio"}], kind=kind))
                self.assertEqual(bool(errors), kind != "dev")

    def test_missing_or_malformed_graph_evidence_fails_closed(self):
        mutations = (
            lambda g: g.pop("packages"),
            lambda g: g.pop("resolve"),
            lambda g: g.update(resolve=None),
            lambda g: g["resolve"].pop("nodes"),
            lambda g: g["resolve"]["nodes"].pop(),
            lambda g: g["resolve"]["nodes"][1].pop("features"),
            lambda g: g["resolve"]["nodes"][1].pop("deps"),
            lambda g: g["resolve"]["nodes"][1]["deps"][0].pop("dep_kinds"),
        )
        for mutate in mutations:
            with self.subTest(mutation=mutate):
                graph = fixture()
                mutate(graph)
                self.assertTrue(BOUNDARY.violations(graph))

    def test_required_bus_must_be_reachable_from_the_proof(self):
        graph = fixture()
        graph["resolve"]["nodes"][0]["deps"] = []
        self.assertTrue(BOUNDARY.violations(graph))

    def test_duplicate_required_packages_are_rejected(self):
        for package in ("proof", "bus"):
            with self.subTest(package=package):
                graph = fixture()
                original = next(p for p in graph["packages"] if p["id"] == package)
                graph["packages"].append({**original, "id": "duplicate"})
                graph["resolve"]["nodes"].append({"id": "duplicate", "features": [], "deps": []})
                self.assertTrue(BOUNDARY.violations(graph))

    def test_cli_accepts_current_pure_proof_graph(self):
        with (
            patch.object(BOUNDARY.subprocess, "check_output", return_value=json.dumps(fixture())) as metadata,
            redirect_stdout(StringIO()) as output,
        ):
            self.assertEqual(BOUNDARY.main(), 0)
        self.assertIn("pure bus contract", output.getvalue())
        self.assertIn("--locked", metadata.call_args.args[0])


if __name__ == "__main__":
    unittest.main()
