#!/usr/bin/env python3
"""Critical opt-in, attribution and evidence gates; no production reader is run."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('metrics_real_under_test', Path(__file__).with_name('metrics_real.py'))
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)


def active_summary(phase='many', repetition=0, rss=10*1024*1024):
    return dict(phase=phase, repetition=repetition, group='metrics_plugin_tree',
                rss_bytes_sampled_peak=rss, pss_bytes_sampled_peak=rss//2,
                stable_process_set=True, processes_sampled_peak=1)


def availability(repetition=0):
    return [dict(phase='many', repetition=repetition, elapsed_s=elapsed,
                 sources=[dict(source=source, availability='current', valid_current=True)
                          for source in M.SOURCES]) for elapsed in (0., 2., 5.)]


class CriticalMetricsEvidence(unittest.TestCase):
    def test_default_does_not_inspect_or_launch(self):
        with patch.object(M, 'validate_output', side_effect=AssertionError('inspection')), \
             patch.object(M, 'run_case', side_effect=AssertionError('launch')), \
             patch.object(M, 'provenance', side_effect=AssertionError('ELF inspection')):
            M.main(['target/performance/unused-metrics-real'])

    def test_private_environment_drops_personal_bus_and_loader_injection(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with patch.dict(os.environ, dict(HOME='/personal', DBUS_SESSION_BUS_ADDRESS='personal-bus',
                                             LD_PRELOAD='/untrusted.so', LD_LIBRARY_PATH='/fake-nvml',
                                             PATH='/personal-bin', SECRET_SENTINEL='secret')):
                env = M.private_environment(root)
            for key in ('HOME', 'XDG_RUNTIME_DIR', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME'):
                self.assertTrue(Path(env[key]).is_relative_to(root))
            self.assertEqual(env['PATH'], '/usr/bin:/bin')
            self.assertEqual(env['DBUS_SESSION_BUS_ADDRESS'], 'unix:path='+str(root/'no-personal-bus'))
            self.assertFalse({'LD_PRELOAD', 'LD_LIBRARY_PATH', 'SECRET_SENTINEL'} & env.keys())

    def test_seed_command_is_fixed_and_requires_explicit_real_flag(self):
        command = M.seed_command(Path('/owned/root'))
        self.assertEqual(command[-3:], ['/owned/root', 'metrics', '--real-metrics'])
        self.assertEqual(command[0], str(M.REPO/'target/release/examples/official_profile_seed'))

    def test_machine_is_disabled_before_any_product_launch_and_no_backend_is_prepared(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)/'owned'
            def fake_seed(command, **kwargs):
                self.assertEqual(command, M.seed_command(root))
                self.assertEqual(kwargs['env'], {'PATH':'/usr/bin:/bin'})
                (root/'data/rad').mkdir(mode=0o700, parents=True)
                return SimpleNamespace(stdout=json.dumps(dict(event='seeded', plugin='metrics',
                    real_metrics=True, executable=M.BINARIES['metrics_plugin'])))
            world = M.MetricsWorld.__new__(M.MetricsWorld)
            with patch.object(M.subprocess, 'run', side_effect=fake_seed), \
                 patch.object(M.COMMON.World, 'spawn', side_effect=AssertionError('product launch')), \
                 patch.object(M.COMMON.World, 'prepare_backend', side_effect=AssertionError('backend')):
                world.__init__(root, None, {})
            machine = root/'data/rad/machine.json'
            self.assertEqual(json.loads(machine.read_text()), dict(version=1, use_stream_deck=False,
                                                                  start_with_session=False))
            self.assertEqual(machine.stat().st_mode & 0o777, 0o600)
            self.assertEqual(world.roots, [])

    def test_hardware_enabled_state_is_rejected_before_visual_probe(self):
        world = M.MetricsWorld.__new__(M.MetricsWorld)
        peer = SimpleNamespace(call=lambda operation, **args:
                               dict(deck_state='connected') if operation == 'status' else
                               self.fail('unexpected visual or machine call'))
        with self.assertRaises(ValueError):
            world.observe_visuals(peer, 'many')

    def test_current_requires_valid_source_content_not_just_availability(self):
        for source, text in [('cpu', 'CPU 0%'), ('ram', 'RAM 2.0 GiB used 4.0 GiB avail'),
                             ('gpu', 'GPU 100%'), ('vram', 'VRAM 2.0/8.0 GiB')]:
            value = dict(availability='current', text=text, fault=False)
            self.assertTrue(M.visual_observation(source, value)['valid_current'])
            for state in ('stale', 'unavailable'):
                self.assertFalse(M.visual_observation(source, dict(value, availability=state))['valid_current'])
        for text in ('GPU 101%', 'CPU 10%', 'GPU NaN%', 'GPU serial-private'):
            value = M.visual_observation('gpu', dict(availability='current', text=text))
            self.assertFalse(value['valid_current'])
            self.assertIsNone(value['text'])
        self.assertFalse(M.visual_observation('vram', dict(availability='current', text='VRAM 9.0/8.0 GiB'))['valid_current'])
        self.assertFalse(M.visual_observation('gpu', dict(availability='current', text='GPU 2%', fault=True))['valid_current'])

    def test_plugin_tree_excludes_runtime_and_helper_but_includes_descendants(self):
        values = [dict(pid=10, ppid=1), dict(pid=11, ppid=10),
                  dict(pid=12, ppid=10), dict(pid=13, ppid=12), dict(pid=14, ppid=13)]
        roles = {10:'runtime', 11:'launcher_helper', 12:'metrics_plugin', 13:'other', 14:'other'}
        result = M.attribute_processes(values, roles, active=True)
        self.assertEqual([p['pid'] for p in result if p['role'].startswith('metrics_')], [12, 13, 14])
        self.assertEqual(result[-1]['role'], 'metrics_descendant')
        with self.assertRaises(ValueError):
            M.attribute_processes(values+[dict(pid=15, ppid=10)], roles | {15:'other'}, active=True)
        with self.assertRaises(ValueError):
            M.attribute_processes(values, roles, active=False)

    def test_target_cannot_pass_without_gpu_or_complete_repetitions(self):
        summary = [active_summary('one'), active_summary()]
        missing = availability()
        missing[-1]['sources'][2]['valid_current'] = False
        for readings, repetitions in (([], 1), (missing, 1), (availability(), 2)):
            result = M.evaluate_target(summary, readings, repetitions)
            self.assertEqual(result['status'], 'not_verified')
            self.assertIsNone(result['observed_gpu_scenario_target_met'])
        self.assertEqual(M.evaluate_target([], [], 0)['status'], 'not_verified')
        summary[-1]['stable_process_set'] = False
        self.assertEqual(M.evaluate_target(summary, availability(), 1)['status'], 'not_verified')

    def test_valid_gpu_scenario_reports_only_observed_sampled_target(self):
        # User-approved budget; derive expected bytes independently of implementation.
        summary = [active_summary('one'), active_summary(rss=30*1024*1024)]
        result = M.evaluate_target(summary, availability(), 1)
        self.assertEqual(result['target_bytes'], 30*1024*1024)
        self.assertEqual(result['status'], 'within_observed_target')
        self.assertTrue(result['observed_gpu_scenario_target_met'])
        self.assertFalse(result['product_target_certified'])
        self.assertFalse(result['is_absolute_peak'])
        summary[-1]['rss_bytes_sampled_peak'] += 1
        result = M.evaluate_target(summary, availability(), 1)
        self.assertEqual(result['status'], 'exceeds_observed_target')
        self.assertFalse(result['observed_gpu_scenario_target_met'])

    def test_cancellation_after_registration_retains_child_for_cleanup(self):
        world = M.MetricsWorld.__new__(M.MetricsWorld)
        world.roots = []
        previous = {sig:signal.getsignal(sig) for sig in (signal.SIGTERM, signal.SIGINT)}
        def registered(*args, **kwargs):
            signal.getsignal(signal.SIGTERM)(signal.SIGTERM, None)
            world.roots.append(('synthetic-child', 'runtime_root'))
            return 'synthetic-child'
        try:
            M.DESKTOP.install_shutdown_signals()
            with patch.object(M.COMMON.World, 'spawn', side_effect=registered):
                with self.assertRaises(M.DESKTOP.MeasurementCancelled):
                    world.spawn(['synthetic-only'], 'runtime_root')
            self.assertEqual(world.roots, [('synthetic-child', 'runtime_root')])
        finally:
            for sig, handler in previous.items():
                signal.signal(sig, handler)


if __name__ == '__main__':
    unittest.main()
