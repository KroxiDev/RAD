"""Critical isolation and privacy contracts; never real OBS, GUI or credentials."""
import importlib.util
import contextlib
import errno
import io
import json
import os
import signal
import subprocess
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch, MagicMock

SPEC = importlib.util.spec_from_file_location('obs_controller', Path(__file__).with_name('obs_real.py'))
OBS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(OBS)


class CriticalObs(unittest.TestCase):
    @contextlib.contextmanager
    def owned_proc_fd(self):
        child = subprocess.Popen(['/usr/bin/sleep', '5'], start_new_session=True,
            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        path = Path('/proc')/str(child.pid)/'stat'
        handle = path.open()
        try:
            stat = OBS.SAMPLER.parse_stat(handle.read())
            handle.seek(0)
            yield child, path, handle, stat
        finally:
            handle.close()
            if child.poll() is None:
                child.kill()
            child.wait(timeout=3)

    def test_exited_owned_process_proc_fd_esrch_is_not_a_live_process(self):
        with self.owned_proc_fd() as (child, path, handle, stat):
            child.kill()
            child.wait(timeout=3)
            original = Path.open
            def open_owned(p, *a, **kw):
                return handle if p == path else original(p, *a, **kw)
            with patch.object(Path, 'open', new=open_owned):
                self.assertFalse(OBS.SAMPLER.same_owned_process(child.pid, stat['start_ticks'], child.pid))

    def test_stop_runtime_tolerates_native_proc_fd_esrch_after_reap(self):
        with self.owned_proc_fd() as (child, path, handle, stat):
            original, consumed = Path.open, False
            def wait(timeout):
                child.kill()
                child.wait(timeout=timeout)
                return 0
            def open_owned(p, *a, **kw):
                nonlocal consumed
                if p == path and child.poll() is not None and not consumed:
                    consumed = True
                    return handle
                return original(p, *a, **kw)
            world = OBS.ObsWorld.__new__(OBS.ObsWorld)
            world.runtime = SimpleNamespace(pid=child.pid, wait=wait,
                stdin=MagicMock(), stdout=MagicMock(), reader=MagicMock())
            world.runtime_pid, world.owned, world.events = child.pid, {child.pid:(stat['start_ticks'],child.pid)}, []
            world.remember_descendants, world.signal = MagicMock(), MagicMock()
            with patch.object(Path, 'open', new=open_owned):
                world.stop_runtime()
            self.assertTrue(consumed)
            self.assertEqual(world.events[-1], {'event':'runtime_stopped', 'clean_shutdown':True})
            self.assertIsNone(world.runtime)
            world.signal.assert_called_once_with(child.pid, signal.SIGTERM)

    def test_proc_identity_errno_none_and_permissions_never_become_gone(self):
        for error in (ProcessLookupError('identity guard'), PermissionError(errno.EPERM, 'permission guard')):
            with patch.object(Path, 'read_text', side_effect=error), self.assertRaises(type(error)):
                OBS.SAMPLER.same_owned_process(999998, 20, 999998)
        value = dict(state='S', session=30, start_ticks=20)
        with patch.object(Path, 'read_text', return_value='owned'), \
             patch.object(OBS.SAMPLER, 'parse_stat', return_value=value):
            self.assertFalse(OBS.SAMPLER.same_owned_process(999998, 20, 999998))
            self.assertFalse(OBS.SAMPLER.same_owned_process(999998, 19, 30))
            world = OBS.ObsWorld.__new__(OBS.ObsWorld)
            world.owned = {999998:(20,999998)}
            with patch.object(OBS.COMMON.os, 'pidfd_open', return_value=42), \
                 patch.object(OBS.COMMON.os, 'close'), \
                 patch.object(OBS.COMMON.signal, 'pidfd_send_signal') as send:
                world.signal(999998, signal.SIGTERM)
                send.assert_not_called()

    def test_protected_relay_fields_are_unverified_not_zero_in_aggregates(self):
        rows = []
        for phase in OBS.PHASES:
            for n in range(2):
                rows.append({'phase':phase, 'elapsed_s':n+1., 'processes':[],
                             'relay_resource':{'pid':7, 'start_ticks':8, 'session':7,
                                               'ticks':10+n, 'rss_bytes':4096, 'threads':2}})
        with patch.object(OBS.SERVICES, 'summarize', return_value=[{'group':'readonly_proxy'}]):
            result = OBS.summarize(SimpleNamespace(rows=rows), 0)
        self.assertEqual(len(result), 5)
        for value in result:
            self.assertEqual(value['group'], 'readonly_relay')
            self.assertEqual(value['rss_bytes_sampled_peak'], 4096)
            self.assertIsNone(value['pss_bytes_sampled_peak'])
            self.assertIsNone(value['private_bytes_sampled_peak'])
            self.assertIsNone(value['fds_sampled_peak'])

    def test_window_cannot_pass_after_disconnect_missing_reads_or_reconnect(self):
        before = {'active_connections':1, 'upstream_connections_opened':1,
                  'authenticated_upstreams':1, 'reads':{'GetRecordStatus':1, 'GetStreamStatus':1}}
        valid = before | {'reads':{'GetRecordStatus':2, 'GetStreamStatus':2}}
        for after in (before, valid | {'active_connections':0},
                      valid | {'upstream_connections_opened':2, 'authenticated_upstreams':2}):
            with self.subTest(after=after), self.assertRaises(ValueError):
                OBS.validate_window(before, after, 32)
        OBS.validate_window(before, valid, 32)

    def test_nondumpable_relay_uses_stat_without_fake_private_memory_or_fd_values(self):
        relay = OBS.RelayProcess.__new__(OBS.RelayProcess)
        relay.child = SimpleNamespace(pid=999998, poll=lambda:None)
        relay.owner = SimpleNamespace(owned={999998:(20,999998)})
        stat = dict(state='S', ppid=100, session=999998, ticks=10, threads=2,
                    start_ticks=20, rss_bytes=4096)
        with patch.object(OBS.SAMPLER, 'read_process', side_effect=PermissionError('non-dumpable')), \
             patch.object(OBS.SAMPLER, 'parse_stat', side_effect=lambda _:dict(stat)), \
             patch.object(OBS.SAMPLER, 'descendants', return_value=[999998]), \
             patch.object(Path, 'read_text', return_value='owned-stat'):
            value = relay.resource(None)
        self.assertEqual(value['rss_bytes'], 4096)
        self.assertNotIn('pss_bytes', value)
        self.assertNotIn('private_bytes', value)
        self.assertNotIn('fds', value)

    def test_snapshot_keeps_only_bounded_public_versions_and_read_counters(self):
        good = {'event':'snapshot', 'reads':{'GetVersion':1, 'GetRecordStatus':2, 'GetStreamStatus':3},
                'denied_frames':0, 'upstream_connections_opened':1, 'authenticated_upstreams':1,
                'active_connections':0, 'front_auth_failures':0, 'events_forwarded':0, 'events_dropped':0,
                'upstream_version':{'obsVersion':'32.2.2', 'obsWebSocketVersion':'5.6.3'}}
        self.assertEqual(OBS.validate_snapshot(good), good)
        bad_versions = [{'obsVersion':'/PRIVATE-SENTINEL', 'obsWebSocketVersion':None},
                        {'obsVersion':None, 'obsWebSocketVersion':None, 'path':'PRIVATE-SENTINEL'}]
        for bad in [good | {'credential':'PRIVATE-SENTINEL'}, good | {'denied_frames':1},
                    good | {'active_connections':2}, *[good | {'upstream_version':v} for v in bad_versions]]:
            with self.subTest(keys=sorted(bad)), self.assertRaises(ValueError):
                OBS.validate_snapshot(bad)

    def test_validate_only_never_constructs_credential_prompt(self):
        with patch.object(OBS, 'validate_gate'):
            with patch.object(OBS, 'RelayProcess') as relay:
                self.assertEqual(OBS.execute(None, None, {}, SimpleNamespace(validate_only=True)),
                                 {'gate_only':True})
                relay.assert_not_called()

    def test_case_failure_still_finishes_relay(self):
        relay = MagicMock()
        args = SimpleNamespace(validate_only=False, run_obs=True, repetitions=1, seconds=5, interval=.2)
        with patch.object(OBS, 'validate_gate'), patch.object(OBS, 'RelayProcess', return_value=relay):
            with patch.object(OBS, 'run_case', side_effect=ValueError('unavailable')):
                with self.assertRaises(ValueError):
                    OBS.execute(None, None, {'executable_sha256':{}}, args)
        relay.close.assert_called_once()

    def test_failed_resource_ack_is_not_complete_and_still_reaps(self):
        relay = OBS.RelayProcess.__new__(OBS.RelayProcess)
        relay.credential, relay.final = 'ephemeral-only', None
        relay.owner, relay.child = MagicMock(), MagicMock()
        relay.child.poll.return_value = None
        relay.reply = MagicMock(return_value={'active_connections':0, 'owned_fds_closed':False,
                                              'ui_child_reaped':True})
        with self.assertRaises(ValueError):
            relay.close()
        self.assertIsNone(relay.credential)
        self.assertIsNone(relay.final)
        relay.owner.close.assert_called_once()

    def test_no_personal_secret_environment_reaches_relay(self):
        with tempfile.TemporaryDirectory() as tmp:
            with patch.dict(os.environ, {'OBS_PASSWORD':'PRIVATE-SENTINEL', 'WAYLAND_DISPLAY':'wayland-1'}):
                owner = OBS.RelayOwner(Path(tmp)/'relay', True)
                self.assertNotIn('OBS_PASSWORD', owner.env)
                self.assertEqual(owner.env['WAYLAND_DISPLAY'], 'wayland-1')
                self.assertTrue(owner.env['HOME'].startswith(tmp))
                self.assertNotIn('PRIVATE-SENTINEL', str(owner.env))

    def test_nonapproved_server_port_rejected_before_any_inspection(self):
        with patch.object(OBS, 'provenance') as probe, contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                OBS.main(['target/performance/unused', '--run-obs', '--port', '4444'])
            probe.assert_not_called()

    def test_failure_metadata_excludes_exception_text_and_personal_data(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)/'owned'
            with patch.object(OBS.COMMON, 'validate_output', return_value=out), \
                 patch.object(OBS, 'provenance', return_value={'source_sha256':{}, 'executable_sha256':{}}), \
                 patch.object(OBS, 'retain', return_value=out/'elf'), \
                 patch.object(OBS.subprocess, 'check_output', return_value='fixed-base'), \
                 patch.object(OBS, 'execute', side_effect=ValueError('PRIVATE-SENTINEL')), \
                 contextlib.redirect_stdout(io.StringIO()) as output:
                self.assertEqual(OBS.main([str(out), '--run-obs']), 1)
            metadata = json.loads((out/'metadata.json').read_text())
            self.assertFalse(metadata['complete'])
            self.assertEqual(metadata['error_type'], 'ValueError')
            self.assertNotIn('PRIVATE-SENTINEL', json.dumps(metadata)+output.getvalue())
            self.assertTrue(metadata['error_frames'])
            for frame in metadata['error_frames']:
                self.assertEqual(set(frame), {'file', 'function', 'line'})

    def test_primary_and_cleanup_failures_are_separate_and_no_cleanup_ack_is_lost(self):
        relay = MagicMock()
        relay.close.side_effect = TimeoutError('PRIVATE-CLEANUP')
        relay.final = {'event':'finished', 'owned_fds_closed':True, 'ui_child_reaped':True,
                       'active_connections':0}
        relay.child.poll.return_value = 0
        args = SimpleNamespace(validate_only=False, run_obs=True, repetitions=1, seconds=5, interval=.2)
        metadata = {'executable_sha256':{}}
        with patch.object(OBS, 'validate_gate'), patch.object(OBS, 'RelayProcess', return_value=relay), \
             patch.object(OBS, 'run_case', side_effect=ValueError('PRIVATE-PRIMARY')):
            with self.assertRaises(ValueError):
                OBS.execute(None, None, metadata, args)
        self.assertEqual(metadata['observation_error_type'], 'ValueError')
        self.assertEqual(metadata['relay_cleanup_error_type'], 'TimeoutError')
        self.assertTrue(metadata['relay_reaped'])
        self.assertFalse(metadata['all_owned_reaped'])
        self.assertEqual(metadata['relay_cleanup'], relay.final)
        self.assertNotIn('PRIVATE-', json.dumps(metadata))

    def test_boolean_schema_version_is_not_an_integer_contract(self):
        peer = OBS.ObsPeer.__new__(OBS.ObsPeer)
        peer.world = SimpleNamespace(relay=SimpleNamespace(port=32123, credential='ephemeral-only'))
        with patch.object(OBS.COMMON.Bridge, 'call') as transport:
            with self.assertRaises(ValueError):
                peer.call('set_plugin_settings', plugin_id='org.rad.obs',
                          parameters={'schema_version':True, 'values':{'port':32123}})
            transport.assert_not_called()
    def test_gate_requires_every_boolean_and_never_persists_extra_fields(self):
        good = dict.fromkeys(OBS.GATE_FIELDS, True) | {'event':'validation'}
        for bad in ({'event':'validation', 'complete':True}, good | {'zero_upstream_effects':False},
                    good | {'credential':'PRIVATE-SENTINEL'}, good | {'complete':1}):
            with self.subTest(keys=sorted(bad)), self.assertRaises(ValueError):
                OBS.validate_gate_report(bad)
    def test_failed_synthetic_gate_prevents_password_dialog_and_real_connection(self):
        args = SimpleNamespace(run_obs=True)
        with patch.object(OBS, 'validate_gate', side_effect=ValueError('gate failed')):
            with patch.object(OBS, 'RelayProcess') as relay:
                with self.assertRaises(ValueError):
                    OBS.execute(Path('/unused'), Path('/unused'), {'executable_sha256':{}}, args)
                relay.assert_not_called()

    def test_default_has_no_inspection_gui_or_service_launch(self):
        with patch.object(OBS, 'provenance', side_effect=AssertionError('inspection forbidden')) as probe:
            self.assertEqual(OBS.main(['target/performance/obs-dry']), 0)
            probe.assert_not_called()

    def test_observations_drop_all_untrusted_state(self):
        value = dict(availability='current', text='Stopped', image=False, fault=False,
                     title='PRIVATE-SENTINEL', outputPath='PRIVATE-SENTINEL')
        clean = OBS.visual_observation('record', value)
        self.assertNotIn('PRIVATE-SENTINEL', str(clean))
        self.assertTrue(clean['valid_current'])

    def test_settings_must_match_guard_before_demand(self):
        with self.assertRaises(ValueError):
            OBS.verify_settings({'plugin_id':'org.rad.obs', 'encoding':'hex',
                'hex':b'{"schema_version":1,"values":{"port":4455}}'.hex()}, 32123)

    def test_private_rpc_refuses_actions_options_personal_credentials_and_wrong_port_before_transport(self):
        peer = OBS.ObsPeer.__new__(OBS.ObsPeer)
        peer.world = SimpleNamespace(relay=SimpleNamespace(port=32123, credential='ephemeral-only'))
        forbidden = [
            ('run', {}), ('options', {}), ('machine_settings_set', {}),
            ('credential_replace', {'plugin_id':'org.rad.obs', 'slot':'password', 'secret':'personal'}),
            ('set_plugin_settings', {'plugin_id':'org.rad.obs',
                                    'parameters':{'schema_version':1, 'values':{'port':4455}}}),
            ('edit', {'changes':[{'kind':'set', 'id':'b1', 'field':'visual',
                                'value':{'type':'org.rad.obs#scene',
                                         'parameters':{'schema_version':1, 'values':{}}}}]}),
        ]
        with patch.object(OBS.COMMON.Bridge, 'call') as transport:
            for operation, args in forbidden:
                with self.subTest(operation=operation), self.assertRaises(ValueError):
                    peer.call(operation, **args)
            transport.assert_not_called()


if __name__ == '__main__':
    unittest.main()
