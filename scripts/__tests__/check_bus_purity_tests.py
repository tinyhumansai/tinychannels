import importlib.util
import json
import pathlib
import tempfile
import unittest
from contextlib import redirect_stdout
from io import StringIO
from unittest.mock import patch

SCRIPT = pathlib.Path(__file__).parents[1] / 'check-bus-purity.py'
SPEC = importlib.util.spec_from_file_location('check_bus_purity', SCRIPT)
CHECKER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKER)


class ForbiddenDependencyTests(unittest.TestCase):
    def test_rejects_runtime_dependencies_in_any_manifest_section(self):
        manifest = {
            'dependencies': {'serde': '1'},
            'dev-dependencies': {'tokio': '1'},
            'build-dependencies': {'runtime-alias': {'package': 'anyhow'}},
            'target': {
                'cfg(windows)': {
                    'dev-dependencies': {'async-trait': '0.1'},
                },
            },
        }
        self.assertEqual(
            CHECKER.declared_forbidden_dependencies(manifest),
            {'anyhow', 'async-trait', 'tokio'},
        )

    def test_allows_contract_dependencies_in_manifest(self):
        manifest = {
            'dependencies': {'serde': '1'},
            'dev-dependencies': {'toml': '1'},
        }
        self.assertEqual(CHECKER.declared_forbidden_dependencies(manifest), set())

    def test_all_probe_includes_implicit_optional_features_but_not_suppressed_aliases(self):
        manifest = {
            'features': {'storage': ['dep:driver'], 'named': []},
            'dependencies': {
                'driver': {'optional': True},
                'optional-http': {'optional': True},
            },
            'target': {'cfg(windows)': {'dependencies': {'windows-http': {'optional': True}}}},
        }
        self.assertEqual(
            CHECKER.contract_features(manifest),
            ['named', 'optional-http', 'storage', 'windows-http'],
        )

    def test_storage_exemption_does_not_hide_unrelated_optional_dependencies(self):
        manifest = {
            'dependencies': {
                'tinystoragedrivers-types': {'optional': True},
                'optional-http-client': {'optional': True},
            },
        }
        packages = {
            'bus': 'tinychannels-bus',
            'storage': 'tinystoragedrivers-types',
            'storage-port': 'tinystoragedrivers-bus',
            'http': 'optional-http-client',
        }
        graph = {
            'packages': [{'id': package_id, 'name': name} for package_id, name in packages.items()],
            'resolve': {'nodes': [
                {'id': 'bus', 'features': [], 'deps': [
                    {'pkg': 'storage', 'dep_kinds': [{'kind': None}]},
                    {'pkg': 'http', 'dep_kinds': [{'kind': None}]},
                ]},
                {'id': 'storage', 'features': [], 'deps': [
                    {'pkg': 'storage-port', 'dep_kinds': [{'kind': None}]},
                ]},
                {'id': 'storage-port', 'features': [], 'deps': []},
                {'id': 'http', 'features': [], 'deps': []},
            ]},
        }
        errors = CHECKER.dependency_violations(graph)
        self.assertTrue(any('optional-http-client' in error for error in errors))
        self.assertTrue(any('tinystoragedrivers-types' in error for error in errors))
        self.assertTrue(any('tinystoragedrivers-bus' in error for error in errors))
        self.assertEqual(CHECKER.declared_forbidden_dependencies(manifest), {'tinystoragedrivers-types'})



class WindowsEncodingTests(unittest.TestCase):
    def test_checker_cli_reads_utf8_rust_source_with_cp1252_default(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source_dir = root / 'crates/tinychannels-bus/src'
            source_dir.mkdir(parents=True)
            source = source_dir / 'lib.rs'
            source.write_bytes('/// right quote: ”\n'.encode('utf-8'))
            manifest = root / 'crates/tinychannels-bus/Cargo.toml'
            manifest.write_text('[package]\nname = "tinychannels-bus"\n[features]\n', encoding='utf-8')
            bus_id = 'tinychannels-bus 0.1.0 (path+file:///tinychannels-bus)'
            graph = {
                'packages': [{'id': bus_id, 'name': 'tinychannels-bus'}],
                'resolve': {'nodes': [{'id': bus_id, 'features': [], 'deps': []}]},
            }
            original_read_text = pathlib.Path.read_text

            def cp1252_default(path, encoding=None, errors=None):
                return original_read_text(path, encoding=encoding or 'cp1252', errors=errors)

            with (
                patch.object(CHECKER, 'ROOT', root),
                patch.object(CHECKER, 'BUS_MANIFEST', manifest),
                patch('pathlib.Path.read_text', cp1252_default),
                patch.object(CHECKER.subprocess, 'check_output', return_value=json.dumps(graph).encode()),
                patch('sys.argv', ['check-bus-purity.py', '--offline']),
                redirect_stdout(StringIO()) as output,
            ):
                self.assertEqual(CHECKER.main(), 0)

            self.assertIn('default: pure contract closure (1 packages)', output.getvalue())
            self.assertIn('all: pure contract closure (1 packages)', output.getvalue())


class IndependentFeatureProbeTests(unittest.TestCase):
    def run_checker(self, *, forbidden_mode=None, feature_names=('storage', 'optional-http')):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            source_dir = root / 'crates/tinychannels-bus/src'
            source_dir.mkdir(parents=True)
            (source_dir / 'lib.rs').write_text('// pure contract\n', encoding='utf-8')
            manifest = source_dir.parent / 'Cargo.toml'
            manifest.write_text('[features]\n' + ''.join(f'{name} = []\n' for name in feature_names), encoding='utf-8')
            probes = {}

            def metadata(command, **kwargs):
                path = pathlib.Path(command[command.index('--manifest-path') + 1])
                mode = path.parent.name
                probes[mode] = path.read_text(encoding='utf-8')
                self.assertEqual(command[0:2], ['cargo', 'metadata'])
                self.assertIn('--offline', command)
                graph = {
                    'packages': [{'id': 'bus', 'name': 'tinychannels-bus'}],
                    'resolve': {'nodes': [{'id': 'bus', 'features': [], 'deps': []}]},
                }
                if mode == forbidden_mode:
                    graph['packages'].append({'id': 'http', 'name': 'reqwest'})
                    graph['resolve']['nodes'][0]['deps'].append({'pkg': 'http', 'dep_kinds': [{'kind': None}]})
                    graph['resolve']['nodes'].append({'id': 'http', 'features': [], 'deps': []})
                return json.dumps(graph)

            with (
                patch.object(CHECKER, 'ROOT', root),
                patch.object(CHECKER, 'BUS_MANIFEST', manifest),
                patch.object(CHECKER.subprocess, 'check_output', side_effect=metadata),
                redirect_stdout(StringIO()),
                patch('sys.stderr', StringIO()) as errors,
            ):
                result = CHECKER.main(['--offline'])
            return result, probes, errors.getvalue()

    def test_default_all_and_storage_are_independent_cargo_consumers(self):
        result, probes, _ = self.run_checker()
        self.assertEqual(result, 0)
        self.assertEqual(set(probes), {'default', 'all', 'storage'})
        self.assertNotIn('features = ', probes['default'])
        self.assertIn('features = ["optional-http", "storage"]', probes['all'])
        self.assertIn('features = ["storage"]', probes['storage'])
        for probe in probes.values():
            self.assertIn('[workspace]', probe)
            self.assertIn('tinychannels-bus = { path = ', probe)

    def test_optional_dependency_enabled_only_by_all_features_is_rejected(self):
        result, probes, errors = self.run_checker(forbidden_mode='all')
        self.assertEqual(result, 1)
        self.assertEqual(set(probes), {'default', 'all'})
        self.assertIn('all: contract dependency boundary failed', errors)
        self.assertIn('reqwest', errors)

    def test_storage_probe_does_not_exempt_storage_dependencies(self):
        result, probes, errors = self.run_checker(forbidden_mode='storage')
        self.assertEqual(result, 1)
        self.assertEqual(set(probes), {'default', 'all', 'storage'})
        self.assertIn('storage: contract dependency boundary failed', errors)

    def test_contract_without_storage_feature_still_audits_default_and_all(self):
        result, probes, _ = self.run_checker(feature_names=())
        self.assertEqual(result, 0)
        self.assertEqual(set(probes), {'default', 'all'})


if __name__ == '__main__':
    unittest.main()
