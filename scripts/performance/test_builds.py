import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('builds', Path(__file__).with_name('builds.py'))
builds = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builds)


class BuildResourceWindow(unittest.TestCase):
    def test_client_exits_while_the_runtime_is_stopped(self):
        binary = builds.REPO / 'target/release/rad-runtime'
        popen = subprocess.Popen
        peer_close = builds.Peer.close
        read_process = builds.observer.read_process
        runtime = None
        client_exited_at = None
        first_sample = None

        def launch(command, **kwargs):
            nonlocal runtime
            child = popen(command, **kwargs)
            if command == [str(binary)]:
                runtime = child
            return child

        def close_client(peer):
            nonlocal client_exited_at
            if peer.child.stdin.closed:
                return peer_close(peer)
            runtime.send_signal(signal.SIGSTOP)
            os.waitid(os.P_PID, runtime.pid, os.WSTOPPED)
            try:
                peer_close(peer)
            except BaseException:
                runtime.send_signal(signal.SIGCONT)
                raise
            client_exited_at = time.monotonic()

        def sample(pid, root_pid):
            nonlocal first_sample
            started = time.monotonic()
            state = read_process(pid, root_pid)
            if pid == root_pid and first_sample is None:
                first_sample = (state['state'], started)
                runtime.send_signal(signal.SIGCONT)
            return state

        with tempfile.TemporaryDirectory(dir='/tmp') as tmp:
            root = Path(tmp)
            env = builds.observer.clean_environment(root / 'env')
            data = Path(env['XDG_DATA_HOME']) / 'rad'
            data.mkdir(mode=0o700)
            settings = data / 'machine.json'
            settings.write_text(json.dumps(dict(version=1, use_stream_deck=False, start_with_session=False)))
            settings.chmod(0o600)
            with patch.object(builds.subprocess, 'Popen', launch), \
                 patch.object(builds.Peer, 'close', close_client), \
                 patch.object(builds.observer, 'read_process', sample):
                result = builds.boot(binary, root, env, 'fresh')
            samples = [json.loads(line) for line in (root / 'fresh-samples.jsonl').read_text().splitlines()]

        state, sampled_at = first_sample
        self.assertEqual(state, 'T')
        self.assertGreaterEqual(sampled_at - client_exited_at, 0.2)
        for row in [*samples, result['resources']]:
            self.assertEqual(row['phase'], 'post_client_exit')
            self.assertEqual(row['delay_after_client_exit_ms'], 200)


if __name__ == '__main__':
    unittest.main()
