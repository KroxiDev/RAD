#!/usr/bin/env python3
"""Critical observer safety checks. Never opens/inspects real hardware or services.

Build physical_profile and rad-launcher-helper first. Run outside a sandbox that
blocks fuser sockets. Own fixtures/evidence stay private in target/performance.
"""
import importlib.util
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import time
import unittest
from unittest.mock import patch

_SPEC=importlib.util.spec_from_file_location('rad_physical',Path(__file__).with_name('physical.py'))
physical=importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(physical)
observer=physical.observer


def own_root(label):
    root=physical.REPO/'target/performance'/f'safety-{label}-{time.monotonic_ns()}'
    root.mkdir(mode=0o700)
    return root


class GuardTests(unittest.TestCase):
    def test_fuser_consistency_and_errors_fail_closed(self):
        for code,out,err,expected in [(1,'','',set()),(0,' 12 34','/dev/fixture:',{12,34})]:
            with patch.object(physical.subprocess,'run',return_value=subprocess.CompletedProcess([],code,out,err)):
                self.assertEqual(physical.holders('/dev/fixture'),expected)
        for code,out,err in [(0,'',''),(1,'12',''),(0,'not-a-pid',''),(1,'','Permission denied'),(2,'','')]:
            with self.subTest(code=code,out=out,err=err),patch.object(physical.subprocess,'run',return_value=subprocess.CompletedProcess([],code,out,err)):
                with self.assertRaises(RuntimeError):physical.holders('/dev/fixture')

    def test_installed_fuser_observes_only_our_fixture(self):
        path=own_root('fuser')/'owned-file'
        path.write_bytes(b'own file, never a device')
        with path.open('rb'):
            self.assertIn(os.getpid(),physical.holders(path))
        self.assertFalse(physical.holders(path))

    def test_default_dry_does_not_even_inspect_devices(self):
        output=physical.REPO/'target/performance'/f'dry-guard-{time.monotonic_ns()}'
        with patch.object(sys,'argv',['physical.py',str(output)]),patch.object(physical,'preflight',side_effect=AssertionError('hardware inspection')) as pre,patch.object(Path,'mkdir',side_effect=RuntimeError('own output setup')):
            with self.assertRaisesRegex(RuntimeError,'own output setup'):physical.main()
            pre.assert_not_called()

    def test_failed_preflight_precedes_output_or_launch(self):
        output=physical.REPO/'target/performance'/f'physical-guard-{time.monotonic_ns()}'
        with patch.object(sys,'argv',['physical.py',str(output),'--physical']),patch.object(physical,'preflight',side_effect=RuntimeError('not exclusive')),patch.object(Path,'mkdir') as mkdir,patch.object(observer,'observe') as launch:
            with self.assertRaisesRegex(RuntimeError,'not exclusive'):physical.main()
            mkdir.assert_not_called();launch.assert_not_called()

    def test_mode_shutdown_and_human_evidence_are_required(self):
        event=dict(event='complete',mode='physical',clean_shutdown=True,human_complete=True,
            logo_restored=True,human_counts=dict(navigation=12,sleep=1,wake=1,all_released=True))
        self.assertTrue(physical.completion_status(event,True)['complete'])
        changes=[dict(mode='dry_run'),dict(clean_shutdown=False),dict(human_complete=1),
                 dict(human_counts=dict(navigation=0,sleep=0,wake=0,all_released=True)),
                 dict(human_counts=dict(navigation=12,sleep=1,wake=1,all_released=False))]
        for change in changes:
            with self.subTest(change=change),self.assertRaises(ValueError):
                physical.completion_status(dict(event,**change),True)
        self.assertFalse(physical.completion_status(dict(event,human_complete=False),True)['complete'])
        self.assertFalse(physical.completion_status(dict(event,logo_restored=False),True)['complete'])

    def test_callbacks_propagate_failure_and_reap_only_owned_child(self):
        root=own_root('callback');seen=[];samples=[]
        def reject(event):
            seen.append(event['pid']);raise RuntimeError('synthetic callback failure')
        script='import os,json,time; print(json.dumps({"event":"ready","pid":os.getpid()}),flush=True);time.sleep(60)'
        with self.assertRaisesRegex(RuntimeError,'synthetic callback failure'):
            observer.observe([sys.executable,'-I','-S','-B','-c',script],root/'observer',.05,5,
                             event_callback=reject,sample_callback=samples.append)
        self.assertTrue(seen);self.assertTrue(samples)
        identities={p['pid']:p['start_ticks'] for row in samples for p in row['processes']}
        for pid in seen:
            self.assertFalse(observer.same_owned_process(pid,identities[pid],pid))

    def test_sigterm_dry_idle_releases_without_waiting_for_human(self):
        root=own_root('cancel')
        binary=physical.REPO/'target/release/examples/physical_profile'
        self.assertTrue(binary.is_file(),'build the offline release example before this check')
        env=observer.clean_environment(root/'env')
        lines=[];buffer=b''
        with (root/'stderr.log').open('wb') as error:
            child=subprocess.Popen([str(binary),str(root/'profile'),'--dry-run'],env=env,
                stdin=subprocess.DEVNULL,stdout=subprocess.PIPE,stderr=error,start_new_session=True)
            try:
                deadline=time.monotonic()+15
                idle=False
                while not idle:
                    self.assertLess(time.monotonic(),deadline,'dry idle readiness deadline')
                    self.assertIsNone(child.poll(),'dry process exited before idle')
                    if not select.select([child.stdout],[],[],.2)[0]:continue
                    chunk=os.read(child.stdout.fileno(),65536)
                    self.assertTrue(chunk,'EOF before dry idle')
                    buffer+=chunk
                    while b'\n' in buffer:
                        line,buffer=buffer.split(b'\n',1);event=json.loads(line);lines.append(event)
                        idle|=event.get('event')=='phase_begin' and event.get('phase')=='idle'
                started=time.monotonic();child.terminate()
                rest,_=child.communicate(timeout=5)
                elapsed=time.monotonic()-started
                buffer+=rest
                lines.extend(json.loads(line) for line in buffer.splitlines())
                complete=[e for e in lines if e.get('event')=='complete']
                self.assertEqual(len(complete),1)
                self.assertEqual(complete[0]['mode'],'dry_run')
                self.assertTrue(complete[0]['synthetic'])
                self.assertTrue(complete[0]['clean_shutdown'])
                self.assertTrue(complete[0]['logo_restored'])
                self.assertFalse(complete[0]['human_complete'])
                self.assertLess(elapsed,5)
            finally:
                if child.poll() is None:
                    child.kill();child.wait(timeout=5)
                child.stdout.close()
        (root/'events.jsonl').write_text(''.join(json.dumps(e)+'\n' for e in lines))


if __name__=='__main__':unittest.main()
