"""Source and graph controls for the synchronous, storage-free contract."""

import importlib.util
from pathlib import Path
import tempfile
import tomllib
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("check-bus-purity.py")
SPEC = importlib.util.spec_from_file_location("bus_purity", SCRIPT)
BUS_PURITY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUS_PURITY)


def metadata_fixture(*, extra=(), kind=None):
    packages = [
        {"id": "bus", "name": "tinychannels-bus", "version": "0.2.0"},
        {"id": "serde", "name": "serde", "version": "1.0.0"},
        *extra,
    ]
    nodes = [
        {"id": "bus", "features": ["storage"], "deps": [
            {"pkg": "serde", "name": "serde", "dep_kinds": [{"kind": None}]},
        ]},
        {"id": "serde", "features": [], "deps": []},
    ]
    for package in extra:
        nodes[0]["deps"].append({"pkg": package["id"], "name": package["name"], "dep_kinds": [{"kind": kind}]})
        nodes.append({"id": package["id"], "features": [], "deps": []})
    return {"packages": packages, "resolve": {"nodes": nodes}}


class BusPurityTests(unittest.TestCase):
    def test_bus_storage_schema_is_a_pure_collection_declaration(self):
        self.assertEqual(BUS_PURITY.source_tree_violations(), [])
        schema = (BUS_PURITY.BUS_SOURCE / "storage/schema.rs").read_text(encoding="utf-8")
        self.assertIn("CollectionSpec", schema)
        self.assertNotIn("async fn", schema)
        self.assertNotIn("tinystoragedrivers_", schema)
        implementation = BUS_PURITY.ROOT / "src/storage"
        self.assertIn("pub async fn prepare", (implementation / "mod.rs").read_text(encoding="utf-8"))
        self.assertIn("pub fn stored_delivery_from_queued", (implementation / "records.rs").read_text(encoding="utf-8"))

    def test_bus_declares_no_runtime_or_storage_driver_dependencies(self):
        manifest = tomllib.loads(BUS_PURITY.BUS_MANIFEST.read_text(encoding="utf-8"))
        self.assertEqual(BUS_PURITY.declared_forbidden_dependencies(manifest), set())
        self.assertEqual(manifest["features"]["storage"], [])
        self.assertEqual(BUS_PURITY.dependency_violations(metadata_fixture()), [])

    def test_storage_clients_and_types_are_rejected_for_any_version_or_source(self):
        for name in ("tinystoragedrivers-busproxy", "tinystoragedrivers-types", "tinystoragedrivers-core"):
            for version in ("1.9.0", "2.1.2"):
                with self.subTest(name=name, version=version):
                    graph = metadata_fixture(extra=[{
                        "id": "storage", "name": name, "version": version,
                        "source": "git+https://github.com/tinyhumansai/tinystoragedrivers?rev=bf1f0a97fbe6165a6edd9521659c7f6859700761",
                    }])
                    self.assertTrue(any("unsafe contract dependencies" in error for error in BUS_PURITY.dependency_violations(graph)))

    def test_tokio_is_rejected_even_with_only_sync_features(self):
        for features in (["sync"], ["sync", "rt"]):
            with self.subTest(features=features):
                graph = metadata_fixture(extra=[{"id": "tokio", "name": "tokio"}])
                graph["resolve"]["nodes"][-1]["features"] = features
                self.assertTrue(BUS_PURITY.dependency_violations(graph))

    def test_normal_and_build_dependencies_are_audited_but_dev_edges_are_excluded(self):
        for kind in (None, "normal", "build", "dev"):
            with self.subTest(kind=kind):
                graph = metadata_fixture(extra=[{"id": "http", "name": "reqwest"}], kind=kind)
                self.assertEqual(bool(BUS_PURITY.dependency_violations(graph)), kind != "dev")

    def test_transitive_build_dependency_is_rejected(self):
        graph = metadata_fixture(extra=[{"id": "http", "name": "reqwest"}])
        edge = graph["resolve"]["nodes"][0]["deps"].pop()
        edge["dep_kinds"] = [{"kind": "build"}]
        graph["resolve"]["nodes"][1]["deps"].append(edge)
        self.assertTrue(BUS_PURITY.dependency_violations(graph))

    def test_unrecognized_dependencies_are_rejected(self):
        graph = metadata_fixture(extra=[{"id": "new", "name": "unreviewed-crate"}])
        self.assertTrue(BUS_PURITY.dependency_violations(graph))

    def test_incomplete_metadata_fails_closed(self):
        mutations = (
            lambda g: g.pop("packages"),
            lambda g: g.update(resolve=None),
            lambda g: g["resolve"].pop("nodes"),
            lambda g: g["resolve"]["nodes"].pop(),
            lambda g: g["resolve"]["nodes"][0].pop("features"),
            lambda g: g["resolve"]["nodes"][0].update(features=[None]),
            lambda g: g["resolve"]["nodes"][0]["deps"][0].update(dep_kinds=[{}]),
            lambda g: g["resolve"]["nodes"][0]["deps"][0].update(dep_kinds=[{"kind": []}]),
            lambda g: g["resolve"]["nodes"][0].pop("deps"),
            lambda g: g["resolve"]["nodes"][0]["deps"][0].pop("dep_kinds"),
            lambda g: g["resolve"]["nodes"][0]["deps"][0].update(dep_kinds=[]),
            lambda g: g["resolve"]["nodes"][0]["deps"][0].update(dep_kinds=[{"kind": "unknown"}]),
            lambda g: g["resolve"]["nodes"][0]["deps"][0].pop("pkg"),
            lambda g: g["packages"].append(dict(g["packages"][0])),
            lambda g: g["resolve"]["nodes"].append(dict(g["resolve"]["nodes"][0])),
        )
        for mutate in mutations:
            with self.subTest(mutation=mutate):
                graph = metadata_fixture()
                mutate(graph)
                self.assertTrue(BUS_PURITY.dependency_violations(graph))

    def test_execution_and_async_interfaces_are_rejected_including_traits(self):
        for source in (
            "async fn send() {}", "#[async_trait] trait Channel {}",
            "tokio::sync::mpsc::Sender<Message>", "std::net::TcpStream",
            "std::process::Command", "std::env::var(\"KEY\")", "std::time::Instant",
            "std::fs::read(path)", "std::thread::spawn(f)", "std::sync::Arc<X>",
            "parking_lot::Mutex<X>", "sha2::Sha256", "hmac::Hmac", "rand::random()",
            "uuid::Uuid", "reqwest::Client", "impl Channel for Provider {}",
            "fn build_session_key() {}", "fn classify_send_error() {}",
            "fn authenticated_inbound_event() {}", "fn segment_for_delivery() {}",
            "use tinystoragedrivers_types::CollectionSpec;",
        ):
            with self.subTest(source=source):
                self.assertTrue(BUS_PURITY.source_violations(source, source_path=Path("traits.rs")))

    def test_migrations_and_storage_projections_are_rejected_but_tests_are_skipped(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            storage = root / "crates/tinychannels-bus/src/storage"
            storage.mkdir(parents=True)
            (storage / "records.rs").write_text("impl From<Delivery> for StoredDelivery {}", encoding="utf-8")
            (storage / "migrate.rs").write_text("", encoding="utf-8")
            (storage / "records_tests.rs").write_text("std::fs::read(path)", encoding="utf-8")
            with patch.object(BUS_PURITY, "ROOT", root):
                errors = BUS_PURITY.source_tree_violations()
            self.assertEqual(len(errors), 2)
            self.assertTrue(any("migration execution" in error for error in errors))
            self.assertTrue(any("storage projections" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
