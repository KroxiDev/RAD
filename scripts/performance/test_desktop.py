#!/usr/bin/env python3
"""Critical desktop isolation controls; no real GUI, device or personal services."""
import importlib.util
from contextlib import contextmanager
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('desktop_under_test', Path(__file__).with_name('desktop.py'))
D = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(D)


@contextmanager
def fixture():
    # Retain owned ignored fixtures, as the existing physical safety suite does.
    yield Path(tempfile.mkdtemp(dir=D.REPO/'target/performance',prefix='desktop-safety-'))


class CriticalIsolation(unittest.TestCase):
    def test_default_does_not_inspect_or_launch(self):
        with patch.object(D.PHYSICAL, 'preflight', side_effect=AssertionError('device inspection')), \
             patch.object(D, 'desktop_environment', side_effect=AssertionError('desktop preparation')):
            D.main(['target/performance/not-created'])
        self.assertFalse((D.REPO/'target/performance/not-created').exists())

    def test_physical_requires_desktop_opt_in(self):
        with self.assertRaises(SystemExit):
            D.main(['target/performance/not-created', '--physical'])

    def test_output_rejects_existing_and_symlink_parent(self):
        with self.assertRaises(ValueError):
            D.validate_output(D.REPO/'target/performance')
        with self.assertRaises(ValueError):
            D.validate_output(Path('/tmp/not-rad-output'))

    def test_focus_requires_exact_owned_pid(self):
        self.assertTrue(D.focus_matches({'pid':321, 'mapped':True},321))
        self.assertFalse(D.focus_matches({'pid':322, 'mapped':True},321))
        self.assertFalse(D.focus_matches({'pid':321, 'mapped':False},321))
        self.assertFalse(D.focus_matches({},321))

    def test_holder_guard_rejects_every_other_writer(self):
        device = {'hidraw':'/dev/owned-fixture', 'usb':'/dev/owned-usb'}
        with patch.object(D.PHYSICAL,'locate_xl', return_value=device), \
             patch.object(D.PHYSICAL,'holders', return_value={321}):
            self.assertEqual(D.guard_owners(device,{321}),1)
            with self.assertRaises(RuntimeError):
                D.guard_owners(device,set())
        with patch.object(D.PHYSICAL,'locate_xl', return_value=device|{'usb':'/dev/changed'}):
            with self.assertRaises(RuntimeError):
                D.guard_owners(device,{321})

    def test_environment_never_inherits_personal_home_bus_or_path(self):
        with fixture() as root:
            with patch.dict(os.environ,{'HOME':'/private-home','DBUS_SESSION_BUS_ADDRESS':'private-personal',
                                        'PATH':'/private-path','SECRET_SENTINEL':'never-copy'}):
                env=D.desktop_environment(root,Path('/owned/display.sock'),'unix:path='+str(root/'bus'))
            for key in ('HOME','XDG_RUNTIME_DIR','XDG_CONFIG_HOME','XDG_DATA_HOME'):
                self.assertTrue(Path(env[key]).is_relative_to(root))
            self.assertNotIn('SECRET_SENTINEL',env)
            self.assertNotIn('/private-path',env['PATH'])
            self.assertEqual(env['WAYLAND_DISPLAY'],'/owned/display.sock')
            self.assertEqual(env['DBUS_SESSION_BUS_ADDRESS'],'unix:path='+str(root/'bus'))

    def test_systemctl_shim_never_delegates_unknown_command(self):
        with fixture() as root:
            shim=D.write_systemctl_shim(root)
            result=subprocess.run([str(shim),'--user','stop','unrelated.service'],capture_output=True)
            self.assertNotEqual(result.returncode,0)
            self.assertIn('stop unrelated.service',(root/'systemctl.log').read_text())

    def test_exception_teardown_kills_only_owned_writer_without_logo_shutdown(self):
        with fixture() as root, (root/'log').open('w') as log:
            world=D.DesktopWorld.__new__(D.DesktopWorld)
            world.roots,world.owned,world.events=[],{},[]
            world.log,world.env=log,{'PATH':'/usr/bin:/bin'}
            world.guard_stop,world.guard_thread=threading.Event(),None
            world.runtime=world.spawn([sys.executable,'-I','-S','-c','import time; time.sleep(60)'],'runtime')
            world.runtime_pid=world.runtime.pid
            child=world.runtime
            world.close()
            self.assertEqual(child.returncode,-9)
            self.assertTrue(any(e.get('event')=='emergency_writer_stopped' for e in world.events))

    def test_sigterm_of_supervisor_reaps_separate_session_writer(self):
        self.signal_case(False)

    def test_signal_between_popen_and_registration_cannot_orphan_writer(self):
        self.signal_case(True)

    def test_first_cancellation_during_normal_cleanup_is_deferred_not_lost(self):
        with fixture() as root:
            code='''import importlib.util,os,signal,sys
from pathlib import Path
s=importlib.util.spec_from_file_location('d',sys.argv[1]);d=importlib.util.module_from_spec(s);s.loader.exec_module(d)
d.install_shutdown_signals()
with d.protected_cleanup():
 os.kill(os.getpid(),signal.SIGTERM)
 Path(sys.argv[2]).write_text('cleanup finished before cancellation was delivered')
'''
            result=subprocess.run([sys.executable,'-I','-S','-B','-c',code,
                str(D.REPO/'scripts/performance/desktop.py'),str(root/'cleaned')],capture_output=True,timeout=5)
            self.assertTrue((root/'cleaned').exists())
            self.assertNotEqual(result.returncode,0,'first cancellation was silently lost during cleanup')

    def signal_case(self, during_registration):
        with fixture() as root:
            code='''import importlib.util,json,os,signal,sys,threading,time
from pathlib import Path
spec=importlib.util.spec_from_file_location('d',sys.argv[1]); d=importlib.util.module_from_spec(spec);spec.loader.exec_module(d)
root=Path(sys.argv[2]); d.install_shutdown_signals()
with (root/'child.log').open('w') as log:
 w=d.DesktopWorld.__new__(d.DesktopWorld);w.roots,w.owned,w.events=[],{},[]
 w.log,w.env=log,{'PATH':'/usr/bin:/bin'};w.guard_stop,w.guard_thread=threading.Event(),None
 w.runtime=w.runtime_pid=None
 if sys.argv[3]=='registration':
  original=d.COMMON.subprocess.Popen
  def injected(*args,**kwargs):
   child=original(*args,**kwargs);st=d.SAMPLER.parse_stat(Path('/proc',str(child.pid),'stat').read_text())
   (root/'ready.json').write_text(json.dumps([child.pid,st['start_ticks'],st['session']]))
   os.kill(os.getpid(),signal.SIGTERM)
   return child
  d.COMMON.subprocess.Popen=injected
 try:
  w.runtime=w.spawn([sys.executable,'-I','-S','-c','import time;time.sleep(60)'],'runtime');w.runtime_pid=w.runtime.pid
  child=w.runtime; start,session=w.owned[child.pid]
  (root/'ready.json').write_text(json.dumps([child.pid,start,session]))
  time.sleep(60)
 finally:
  with d.protected_cleanup():
   w.close();(root/'cleanup.json').write_text(json.dumps({'returncodes':[c.returncode for c,r in w.roots]}))
'''
            with (root/'supervisor.log').open('w') as log:
                process=subprocess.Popen([sys.executable,'-I','-S','-B','-c',code,
                    str(D.REPO/'scripts/performance/desktop.py'),str(root),
                    'registration' if during_registration else 'waiting'],stdout=log,stderr=log,
                    start_new_session=True)
                identity=None
                try:
                    deadline=time.monotonic()+5
                    while not (root/'ready.json').exists() and process.poll() is None and time.monotonic()<deadline:
                        time.sleep(.01)
                    self.assertTrue((root/'ready.json').exists())
                    identity=__import__('json').loads((root/'ready.json').read_text())
                    if not during_registration:
                        process.terminate()
                    process.wait(timeout=5)
                    self.assertTrue((root/'cleanup.json').exists(),'SIGTERM bypassed owned-child teardown')
                    self.assertEqual(__import__('json').loads((root/'cleanup.json').read_text())['returncodes'],[-9])
                    self.assertFalse(D.SAMPLER.same_owned_process(*identity))
                finally:
                    if process.poll() is None:
                        process.kill();process.wait(timeout=5)
                    if identity is not None:
                        D.SAMPLER.terminate_owned(*identity)


if __name__ == '__main__':
    unittest.main()
