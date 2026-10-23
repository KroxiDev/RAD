#!/usr/bin/env python3
"""Critical real-service isolation controls; pure mocks, never connect to a service."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import stat
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('services_real_under_test', Path(__file__).with_name('services_real.py'))
S = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(S)


class CriticalReadOnlyServices(unittest.TestCase):
    def test_read_query_allowlist_excludes_every_mutation_and_option_override(self):
        for args in (('-f', 'json', 'list', 'sinks'), ('-f', 'json', 'list', 'sources'),
                     ('-f', 'json', 'info'), ('subscribe',)):
            self.assertTrue(S.allowed_pactl_args(args))
        for args in (('set-sink-mute', '@DEFAULT_SINK@', 'toggle'), ('set-source-volume', 'x', '0'),
                     ('play-sample', 'x'), ('exit',), ('load-module', 'x'), ('list',),
                     ('--server=other', 'subscribe'), ('-f', 'json', 'info', 'extra')):
            self.assertFalse(S.allowed_pactl_args(args))

    def test_default_does_not_inspect_endpoints_or_launch(self):
        with patch.object(S, 'validate_endpoint', side_effect=AssertionError('endpoint inspection')), \
             patch.object(S, 'provenance', side_effect=AssertionError('binary inspection')), \
             patch.object(S, 'validate_filters', side_effect=AssertionError('application launch')):
            self.assertEqual(S.main(['target/performance/unused-services']), 0)

    def test_media_selector_only_accepts_fixed_public_generic_stems(self):
        self.assertEqual(S.player_identity('brave'), 'brave')
        for value in ('personal-user-player', 'brave.instance1234', 'org.mpris.MediaPlayer2.brave',
                      'brave.*', 'brave=@/other', ''):
            with self.assertRaises(ValueError):
                S.player_identity(value)

    def test_proxy_grants_getall_and_propertieschanged_but_never_talk_or_actions(self):
        command = S.proxy_command(Path('/owned/proxy'), 'unix:path=/owned/input',
                                  Path('/owned/output'), 'brave')
        self.assertIn('--filter', command)
        self.assertIn('--call=org.mpris.MediaPlayer2.brave.*=org.freedesktop.DBus.Properties.GetAll@/org/mpris/MediaPlayer2', command)
        self.assertIn('--broadcast=org.mpris.MediaPlayer2.brave.*=org.freedesktop.DBus.Properties.PropertiesChanged@/org/mpris/MediaPlayer2', command)
        self.assertFalse(any(arg.startswith(('--talk', '--own', '--log', '--sloppy')) for arg in command))
        self.assertFalse(any('.Set@' in arg or '.Play@' in arg for arg in command))

    def test_proxy_report_requires_same_connection_and_both_known_unique_denials(self):
        report = dict.fromkeys(S.PROXY_REPORT_FIELDS, True)
        self.assertEqual(S.validate_proxy_report(report), report)
        for field in S.PROXY_REPORT_FIELDS:
            with self.assertRaises(ValueError):
                S.validate_proxy_report(report | {field:False})
        for value in (report | {'owner':'private-sentinel'}, {'complete':True}, None):
            with self.assertRaises(ValueError):
                S.validate_proxy_report(value)

    def test_rpc_guard_refuses_execution_and_nonvisual_edits_before_transport(self):
        peer = S.ReadPeer.__new__(S.ReadPeer)
        with patch.object(S.COMMON.Bridge, 'call', return_value={}) as transport:
            for operation, args in (
                    ('run', dict(button_id='b1')), ('set_machine_settings', dict(use_stream_deck=True)),
                    ('edit', dict(changes=[dict(kind='set', field='plan', value=[])])),
                    ('edit', dict(changes=[dict(kind='set', field='visual', value=dict(type='org.rad.obs#scene'))]))):
                with self.assertRaises(ValueError):
                    peer.call(operation, **args)
            transport.assert_not_called()

    def test_generated_pactl_guard_forces_private_config_and_rejects_before_exec(self):
        texts, blobs = {}, {}
        with patch.object(Path, 'write_text', autospec=True, side_effect=lambda path, value:texts.update({str(path):value})), \
             patch.object(Path, 'write_bytes', autospec=True, side_effect=lambda path, value:blobs.update({str(path):value})), \
             patch.object(Path, 'chmod'):
            S.write_pactl_guard(Path('/owned/profile'), Path('/owned/elf'))
        config, guard = texts['/owned/profile/pulse-client.conf'], texts['/owned/profile/guard-pactl']
        self.assertIn('autospawn=no\n', config)
        self.assertIn('cookie-file=/owned/profile/synthetic-cookie', config)
        self.assertEqual(blobs['/owned/profile/synthetic-cookie'], bytes(256))
        self.assertIn('PULSE_CLIENTCONFIG=', guard)
        self.assertIn('--server unix:/owned/profile/endpoint/pulse', guard)
        self.assertLess(guard.index('|| exit 97'), guard.index('exec '))

    def test_sanitizer_never_retains_untrusted_text_images_or_identifiers(self):
        for plugin, text in (('audio', 'Muted'), ('media', 'Playing')):
            result = S.visual_observation(plugin, dict(availability='current', text=text, image=False))
            self.assertTrue(result['valid_current'])
            self.assertFalse({'text', 'image_hex', 'jpeg_hex', 'player', 'device'} & result.keys())
        for text in ('private track title', 'file:///private/art.png', 'device-serial', None):
            result = S.visual_observation('media', dict(availability='current', text=text, image=False))
            self.assertFalse(result['valid_current'])
            self.assertNotIn(str(text), json.dumps(result))
        self.assertFalse(S.visual_observation('media', dict(availability='current', text=None, image=True))['valid_current'])
        self.assertFalse(S.visual_observation('audio', dict(availability='stale', text='On', image=False))['valid_current'])

    def test_namespace_does_not_bind_home_repo_tmp_or_host_proc(self):
        root, elf = Path('/owned/profile'), Path('/owned/elf')
        command = S.namespace_command(root, elf, pulse=Path('/explicit/pulse'))
        self.assertIn('--unshare-pid', command)
        self.assertIn('--unshare-net', command)
        self.assertNotIn('--new-session', command)
        self.assertEqual(command[command.index('--proc')+1], '/proc')
        self.assertFalse(any(command[i] in ('/home', '/tmp', '/proc', str(S.REPO))
                             for i, arg in enumerate(command[:-1]) if arg in ('--bind', '--ro-bind')))
        self.assertIn('/explicit/pulse', command)
        self.assertIn(str(root/'endpoint/pulse'), command)
        self.assertNotIn('/explicit/bus', command)

    def test_endpoint_requires_owned_absolute_unix_socket_without_fallback(self):
        with self.assertRaises(ValueError):
            S.validate_endpoint(Path('relative'))
        with patch.object(Path, 'lstat', return_value=SimpleNamespace(st_mode=stat.S_IFREG, st_uid=os.geteuid())):
            with self.assertRaises(ValueError):
                S.validate_endpoint(Path('/explicit/socket'))
        with patch.object(Path, 'lstat', return_value=SimpleNamespace(st_mode=stat.S_IFSOCK, st_uid=os.geteuid()+1)):
            with self.assertRaises(ValueError):
                S.validate_endpoint(Path('/explicit/socket'))

    def test_runtime_host_identity_is_hash_and_session_based_not_namespace_pid(self):
        processes = [dict(pid=91, session=90, start_ticks=1, executable='bwrap'),
                     dict(pid=92, session=90, start_ticks=2, executable='runtime')]
        self.assertEqual(S.select_runtime(processes, 90), 92)
        with self.assertRaises(ValueError):
            S.select_runtime([dict(processes[1], session=77)], 90)
        with self.assertRaises(ValueError):
            S.select_runtime(processes+[dict(processes[1], pid=93)], 90)

    def test_missing_player_and_missing_current_are_not_a_fake_pass(self):
        for rows in ([], [dict(phase='many', sources=[dict(availability='unavailable', valid_current=False)])]):
            self.assertEqual(S.availability_summary(rows)['status'], 'not_verified')
        rows = [dict(phase='one', sources=[dict(availability='current', valid_current=True)]),
                dict(phase='many', sources=[dict(availability='current', valid_current=True)])]
        self.assertEqual(S.availability_summary(rows)['status'], 'observed_current')

    def test_synthetic_filter_failure_prevents_real_launch_and_does_not_log_endpoint(self):
        # Retained ignored fixtures are retired only by ROOT's coordinated trash pass.
        root = Path(tempfile.mkdtemp(dir=S.REPO/'target/performance', prefix='services-safety-'))
        out = root/'output'
        metadata = dict(source_sha256={}, executable_sha256={})
        with patch.object(S.COMMON, 'validate_output', return_value=out), \
             patch.object(S, 'validate_endpoint', return_value=Path('/private-endpoint-sentinel')), \
             patch.object(S, 'provenance', return_value=metadata), \
             patch.object(S, 'retain_provenance', return_value=out/'elf'), \
             patch.object(S, 'validate_filters', side_effect=ValueError('synthetic gate only')), \
             patch.object(S, 'run_case', side_effect=AssertionError('real launch')) as launch:
            self.assertEqual(S.main([str(out), '--run-services', '--plugins', 'audio',
                                     '--pulse-socket', '/private-endpoint-sentinel']), 1)
        launch.assert_not_called()
        saved = (out/'metadata.json').read_text()
        self.assertFalse(json.loads(saved)['complete'])
        self.assertNotIn('private-endpoint-sentinel', saved)
        self.assertNotIn('synthetic gate only', saved)

    def test_cancellation_is_deferred_until_child_registration(self):
        world = S.ServicesWorld.__new__(S.ServicesWorld)
        world.roots = []
        previous = {sig:signal.getsignal(sig) for sig in (signal.SIGTERM, signal.SIGINT)}
        def registered(*args, **kwargs):
            signal.getsignal(signal.SIGTERM)(signal.SIGTERM, None)
            world.roots.append(('synthetic', 'runtime_namespace'))
        try:
            S.DESKTOP.install_shutdown_signals()
            with patch.object(S.COMMON.World, 'spawn', side_effect=registered):
                with self.assertRaises(S.DESKTOP.MeasurementCancelled):
                    world.spawn(['synthetic'], 'runtime_namespace')
            self.assertEqual(world.roots, [('synthetic', 'runtime_namespace')])
        finally:
            for sig, handler in previous.items():
                signal.signal(sig, handler)


if __name__ == '__main__':
    unittest.main()
