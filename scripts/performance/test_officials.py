import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location('officials_under_test', Path(__file__).with_name('officials.py'))
O = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(O)


class FixtureLifecycle(unittest.TestCase):
    def test_lost_control_ack_reports_overflow(self):
        world = O.World()
        world.env, world.log = {'PATH':'/usr/bin:/bin'}, subprocess.DEVNULL
        self.addCleanup(world.close)
        command = "print('progress\\n' * 16384 + 'pong configured')"
        child = world.spawn([sys.executable, '-I', '-S', '-c', command], 'backend')
        self.assertEqual(child.wait(timeout=5), 0)
        child.reader.join(timeout=5)
        self.assertFalse(child.reader.is_alive())
        with self.assertRaisesRegex(RuntimeError, 'overflow'):
            while world.line(child) != 'pong configured':
                pass

    def test_failed_backend_setup_reaps_all_children_and_preserves_the_error(self):
        for failure in ('reader', 'registration'):
            with self.subTest(failure=failure):
                self.check_backend_failure(failure)

    def check_backend_failure(self, failure):
        children = []
        popen = subprocess.Popen

        def record_child(*args, **kwargs):
            child = popen(*args, **kwargs)
            children.append(child)
            return child

        def seed(command, **kwargs):
            (Path(command[1])/'data/rad').mkdir(parents=True)
            return subprocess.CompletedProcess(command, 0, stdout='{}')

        def start_backend(world):
            command = [sys.executable, '-I', '-S', '-c', 'import time; time.sleep(60)']
            world.spawn(command, 'first')
            if failure == 'reader':
                with patch.object(O.threading.Thread, 'start', side_effect=RuntimeError(failure)):
                    world.spawn(command, 'second')
            else:
                world.spawn(command, 'second')

        remember = O.World.remember

        def register(world, pid, session):
            if failure == 'registration' and len(children) == 2 and pid == children[1].pid:
                raise RuntimeError(failure)
            return remember(world, pid, session)

        try:
            with tempfile.TemporaryDirectory() as out, \
                 patch.object(O.subprocess, 'run', side_effect=seed), \
                 patch.object(O.subprocess, 'Popen', side_effect=record_child), \
                 patch.object(O.World, 'remember', register), \
                 patch.object(O.World, 'prepare_backend', start_backend):
                with self.assertRaisesRegex(RuntimeError, '^'+failure+'$'):
                    O.run_case(Path(out), 'media', 0, 5, .2)
            self.assertEqual(len(children), 2)
            for child in children:
                self.assertIsNotNone(child.returncode)
                self.assertTrue(child.stdin.closed and child.stdout.closed)
        finally:
            for child in children:
                child.kill()
                child.wait(timeout=3)
                child.stdin.close()
                child.stdout.close()
