#!/usr/bin/env python3
"""Critical counter privacy/exactness gates; no root or kernel tracing."""
import importlib.util
import ctypes
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

SPEC=importlib.util.spec_from_file_location('counter_under_test',Path(__file__).with_name('wakeup_counter.py'))
H=importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(H)
WSPEC=importlib.util.spec_from_file_location('wakeups_under_test',Path(__file__).with_name('wakeups.py'))
W=importlib.util.module_from_spec(WSPEC);WSPEC.loader.exec_module(W)


def request():
    return dict(controller=dict(pid=100,start=1),duration_s=10,
        roots=[dict(pid=101,start=2,session=101)],
        targets=[dict(pid=101,start=2,session=101,role='probe',sha256='a'*64,
                      threads=[dict(tid=101,start=2)])])


class FakePerf:
    def __init__(self):
        self.calls=[];self.next_fd=1;self.closed=[];self.event_id=42;self.success_field=False
    def open(self,cpu):
        fd=self.next_fd;self.next_fd+=1;self.calls.append(('open',fd,cpu));return fd
    def filter(self,fd,value):self.calls.append(('filter',fd,value))
    def enable(self,fd):self.calls.append(('enable',fd))
    def disable(self,fd):self.calls.append(('disable',fd))
    def read(self,fd):self.calls.append(('read',fd));return struct.pack('QQQ',7,100,100)
    def close(self,fd):self.closed.append(fd)


class CriticalCounters(unittest.TestCase):
    def test_default_never_inspects_or_opens_counters(self):
        with patch.object(H.os,'geteuid',side_effect=AssertionError('privilege inspection')):
            H.main([])

    def test_filter_is_payload_pid_only_numeric_bounded_and_deduplicated(self):
        self.assertEqual(H.target_filter([123,124,123]),'(pid == 123 || pid == 124)')
        for bad in ([],[True],[0],[-1],['123 || common_pid == 0'],list(range(1,66))):
            with self.assertRaises(ValueError): H.target_filter(bad)

    def test_counts_require_complete_running_time_without_scaling(self):
        self.assertEqual(H.decode_counter(struct.pack('QQQ',7,100,100)),
                         dict(count=7,enabled_ns=100,running_ns=100))
        for bad in (b'',struct.pack('QQQ',0,0,0),struct.pack('QQQ',7,100,99)):
            with self.assertRaises(ValueError): H.decode_counter(bad)

    def test_request_has_no_arbitrary_paths_commands_or_unknown_fields(self):
        good=request();self.assertEqual(H.validate_request(good),good)
        for field,value in (('command','/bin/sh'),('output','/etc/unsafe'),('unknown',True)):
            with self.assertRaises(ValueError): H.validate_request(good|{field:value})
        for duration in (0,21,True,'10'):
            with self.assertRaises(ValueError): H.validate_request(good|{'duration_s':duration})

    def test_request_requires_fixed_unique_owned_processes_and_threads(self):
        good=request()
        for bad in (good|{'roots':[]},good|{'targets':[]},good|{'targets':good['targets']*2}):
            with self.assertRaises(ValueError): H.validate_request(bad)
        for field,value in (('role','personal-title'),('sha256','not-a-hash'),('pid',True),('threads',[])):
            changed=good|{'targets':[good['targets'][0]|{field:value}]}
            with self.assertRaises(ValueError): H.validate_request(changed)

    def test_filter_precedes_enable_and_each_cpu_uses_new_counter(self):
        perf=FakePerf();c=H.Counters(perf,[0,1],request()['targets'])
        self.assertFalse(any(call[0]=='enable' for call in perf.calls))
        c.start();rows=c.stop();c.close()
        self.assertEqual(perf.closed,[1,2])
        self.assertEqual([row['cpu'] for row in rows],[0,1])
        for fd in (1,2):
            self.assertLess(perf.calls.index(('filter',fd,'(pid == 101)')),
                            perf.calls.index(('enable',fd)))
            self.assertLess(perf.calls.index(('disable',fd)),perf.calls.index(('read',fd)))

    def test_filter_rejection_closes_all_created_fds_without_enabling(self):
        perf=FakePerf()
        with patch.object(perf,'filter',side_effect=OSError('kernel rejected filter')):
            with self.assertRaises(OSError): H.Counters(perf,[0,1],request()['targets'])
        self.assertEqual(perf.closed,[1])
        self.assertFalse(any(call[0]=='enable' for call in perf.calls))

    def test_controller_loss_closes_counters_and_cannot_emit_complete(self):
        perf=FakePerf();events=[]
        with patch.object(H,'validate_live'),patch.object(H,'online_cpus',return_value=[0]), \
             patch.object(H,'Perf',return_value=perf),patch.object(H.os,'pidfd_open',side_effect=[100,101]), \
             patch.object(H.os,'close') as close,patch.object(H,'emit',side_effect=events.append):
            protocol=unittest.mock.Mock()
            protocol.read.side_effect=['start',EOFError('controller gone')]
            with self.assertRaises(EOFError): H.capture(protocol,request(),1000)
            self.assertEqual(perf.closed,[1])
            self.assertEqual([call.args[0] for call in close.call_args_list],[100,101])
            self.assertFalse(any(e['event']=='complete' for e in events))

    def test_incomplete_count_closes_all_fds_and_never_claims_complete(self):
        perf=FakePerf();events=[]
        with patch.object(perf,'read',return_value=struct.pack('QQQ',7,100,99)), \
             patch.object(H,'validate_live'),patch.object(H,'online_cpus',return_value=[0]), \
             patch.object(H,'Perf',return_value=perf),patch.object(H.os,'pidfd_open',side_effect=[100,101]), \
             patch.object(H.os,'close'),patch.object(H,'emit',side_effect=events.append):
            protocol=unittest.mock.Mock();protocol.read.side_effect=['start','stop']
            with self.assertRaises(ValueError):H.capture(protocol,request(),1000)
            self.assertEqual(perf.closed,[1])
            self.assertFalse(any(e['event']=='complete' for e in events))

    def test_perf_attributes_never_request_sampling_and_fds_are_cloexec(self):
        seen={}
        def syscall(number,pointer,pid,cpu,group,flags):
            attr=ctypes.cast(pointer,ctypes.POINTER(H.PerfAttr)).contents
            seen.update(type=attr.type,size=attr.size,config=attr.config,period=attr.period,
                        sample_type=attr.sample_type,read_format=attr.read_format,flags=attr.flags,
                        pid=pid.value,cpu=cpu.value,group=group.value,fd_flags=flags.value)
            return 11
        perf=H.Perf.__new__(H.Perf);perf.event_id=42;perf.libc=SimpleNamespace(syscall=syscall)
        self.assertEqual(perf.open(0),11)
        self.assertEqual(seen,dict(type=2,size=128,config=42,period=0,sample_type=0,read_format=3,
                                  flags=1,pid=-1,cpu=0,group=-1,fd_flags=8))

    def test_lost_lifeline_is_not_hidden_by_buffered_stop(self):
        protocol=H.Protocol();protocol.buffer=b'stop\n'
        with patch.object(H.select,'select',return_value=([11],[],[])):
            with self.assertRaises(ValueError):protocol.read(1,[11])

    def test_blocked_output_cannot_extend_privileged_helper_lifetime(self):
        with patch.object(H.select,'select',return_value=([],[],[])):
            with self.assertRaises(TimeoutError):H.emit(dict(event='synthetic'))

    def test_success_field_if_present_is_required_without_changing_target_filter(self):
        perf=H.Perf.__new__(H.Perf);perf.success_field=True
        with patch.object(H.fcntl,'ioctl') as ioctl:
            perf.filter(11,'(pid == 101)')
            self.assertEqual(ioctl.call_args.args,(11,0x40082406,b'((pid == 101) && success == 1)\0'))

    def test_controller_default_never_requests_privilege_or_inspects_inputs(self):
        with patch.object(W,'digest',side_effect=AssertionError('inspection')), \
             patch.object(W.PrivilegedBatch,'start',side_effect=AssertionError('privilege')):
            W.main(['target/performance/unused-wakeup-default'])

    def test_privileged_shutdown_uses_eof_without_signalling_a_root_pid(self):
        child=subprocess.Popen([sys.executable,'-I','-S','-B','-c','import sys;sys.stdin.read()'],
            stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True)
        batch=W.PrivilegedBatch(None,None);batch.child=child
        with patch.object(child,'terminate',side_effect=AssertionError('root signal')), \
             patch.object(child,'kill',side_effect=AssertionError('root signal')):
            batch.close()
        self.assertEqual(child.returncode,0)

    def test_bound_children_do_not_survive_supervisor_sigkill(self):
        root=Path(tempfile.mkdtemp(dir=W.REPO/'target/performance',prefix='wake-safety-'))
        code='''import importlib.util,json,sys,time
from pathlib import Path
s=importlib.util.spec_from_file_location('w',sys.argv[1]);w=importlib.util.module_from_spec(s);s.loader.exec_module(w)
root=Path(sys.argv[2])
with (root/'world.log').open('w') as log:
 world=w.World(root/'world',log)
 child=world.spawn([sys.executable,'-I','-S','-B','-c',"import time;print('ready',flush=True);time.sleep(60)"],'probe')
 assert world.line(child)=='ready'
 st=w.S.parse_stat(Path('/proc',str(child.pid),'stat').read_text())
 (root/'ready.json').write_text(json.dumps([child.pid,st['start_ticks'],st['session']]))
 time.sleep(60)
'''
        with (root/'supervisor.log').open('w') as log:
            child=subprocess.Popen([sys.executable,'-I','-S','-B','-c',code,
                str(W.REPO/'scripts/performance/wakeups.py'),str(root)],stdout=log,stderr=log,
                start_new_session=True)
            identity=None
            try:
                deadline=time.monotonic()+5
                while not (root/'ready.json').exists() and child.poll() is None and time.monotonic()<deadline:
                    time.sleep(.01)
                self.assertTrue((root/'ready.json').exists())
                identity=json.loads((root/'ready.json').read_text())
                child.kill();child.wait(timeout=5)
                deadline=time.monotonic()+2
                while W.S.same_owned_process(*identity) and time.monotonic()<deadline:time.sleep(.01)
                self.assertFalse(W.S.same_owned_process(*identity),'owned target survived its supervisor')
            finally:
                if child.poll() is None:child.kill();child.wait(timeout=5)
                if identity is not None:W.S.terminate_owned(*identity)

    def test_counter_windows_cannot_be_labelled_after_fixture_phase_ended(self):
        edges=[dict(event='phase_begin',phase='idle',at_monotonic_ns=100),
               dict(event='phase_end',phase='idle',at_monotonic_ns=1000)]
        row=dict(enable_before_ns=200,disable_after_ns=900)
        self.assertTrue(W.validate_phase_window(edges,'idle',dict(counters=[row])))
        for bad in (row|{'enable_before_ns':99},row|{'disable_after_ns':1001}):
            with self.assertRaises(ValueError):W.validate_phase_window(edges,'idle',dict(counters=[bad]))
        with self.assertRaises(ValueError):W.validate_phase_window(edges[:1],'idle',dict(counters=[row]))

    def test_cancel_pending_owned_auth_launcher_uses_identity_and_pidfd(self):
        child=subprocess.Popen([sys.executable,'-I','-S','-B','-c','import time;time.sleep(60)'],
            stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True)
        batch=W.PrivilegedBatch(None,None);batch.child=child;batch.close_timeout=.02
        stat=W.S.parse_stat((Path('/proc')/str(child.pid)/'stat').read_text())
        batch.identity=(child.pid,stat['start_ticks'],stat['session'])
        try:
            with patch.object(child,'terminate',side_effect=AssertionError('bare PID')), \
                 patch.object(child,'kill',side_effect=AssertionError('bare PID')):
                batch.close()
            self.assertEqual(child.returncode,-15)
            self.assertTrue(batch.cleanup['owned_launcher_sigterm'])
        finally:
            if child.poll() is None:child.kill();child.wait(timeout=5)

    def test_failed_privileged_cleanup_still_persists_incomplete_metadata(self):
        root=Path(tempfile.mkdtemp(dir=W.REPO/'target/performance',prefix='wake-safety-'))
        batch=SimpleNamespace(close=unittest.mock.Mock(side_effect=RuntimeError('synthetic')),
                              events=[dict(event='failed')],cleanup=dict(returncode=None))
        with self.assertRaises(RuntimeError):W.finalize_batch(batch,root,dict(complete=True))
        self.assertTrue((root/'metadata.json').exists(),'cleanup exception lost failure evidence')
        saved=json.loads((root/'metadata.json').read_text())
        self.assertFalse(saved['complete']);self.assertEqual(saved['cleanup_error_type'],'RuntimeError')

    def test_four_plugin_fixture_has_six_process_targets_not_only_four(self):
        value=request()
        value['targets']=[value['targets'][0]|dict(pid=101+i,threads=[dict(tid=101+i,start=2)])
                          for i in range(6)]
        self.assertEqual(H.validate_request(value),value)
        with self.assertRaises(ValueError):H.validate_request(value|{'targets':value['targets']*2})


if __name__=='__main__':unittest.main()
