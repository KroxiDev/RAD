#!/usr/bin/env python3
"""Controlled component fault profiles; never hardware, GUI or personal services."""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import time

REPO = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('rad_profile', REPO/'scripts/performance/profile.py')
PROFILE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROFILE)

TESTS = {
    'held_downloader': ('runtime_tests', 'plugin_admin::lifecycle_tests::held_download_of_one_plugin_does_not_delay_cold_start_of_another'),
    'stalled_converter': ('media_tests', 'visual::tests::a_reader_without_cover_does_not_wait_for_busy_conversion'),
    'uncertain_store': ('store_tests', 'fault_tests::failed_return_after_real_commit_reconciles_by_new_synced_write_without_rotation'),
}
SOURCES = ('scripts/performance/faults.py', 'scripts/performance/faults.sh',
    'scripts/performance/fault_probe.rs', 'scripts/performance/profile.py',
    'crates/rad-runtime/examples/host_fault_profile.rs',
    'crates/rad-editor/examples/worker_fault_profile.rs',
    'crates/rad-runtime/src/plugin_admin.rs', 'plugins/media/src/visual.rs',
    'crates/rad-store/src/fault_tests.rs', 'crates/rad-editor/src/worker.rs')


def cleanup(process, owned):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    deadline = time.monotonic()+3
    while any(PROFILE.same_owned_process(p, s, process.pid) for p, s in owned.items()) and time.monotonic() < deadline:
        time.sleep(.05)
    survivors = [p for p, s in owned.items() if PROFILE.same_owned_process(p, s, process.pid)]
    for pid in survivors:
        PROFILE.terminate_owned(pid, owned[pid], process.pid)
    if survivors:
        raise RuntimeError('owned fixture processes leaked and were terminated: '+str(survivors))


def run_test(executable, test, directory):
    """Snapshots inside the selected test are self-process, not the whole tree.

    The short test's external samples are retained only as raw observations. They
    cannot establish steady-state CPU or an absolute peak during its fault.
    """
    directory.mkdir(mode=0o700)
    env = PROFILE.clean_environment(directory/'env')
    temporary = directory/'tmp'
    temporary.mkdir(mode=0o700)
    env.update(TMPDIR=str(temporary), RAD_FAULT_PROFILE_ROOT=str(directory/'probe'))
    owned, rows = {}, []
    epoch = time.monotonic()
    with (directory/'stdout.log').open('w') as out, (directory/'stderr.log').open('w') as err:
        process = subprocess.Popen([str(executable), '--exact', test, '--nocapture', '--test-threads=1'],
            cwd=REPO, env=env, stdin=subprocess.DEVNULL, stdout=out, stderr=err,
            start_new_session=True)
        try:
            while process.poll() is None:
                if time.monotonic()-epoch > 30:
                    raise TimeoutError('controlled test exceeded 30-second external deadline')
                snapshot = []
                for pid in PROFILE.descendants(process.pid):
                    try:
                        state = PROFILE.read_process(pid, process.pid)
                        owned[pid] = state['start_ticks']
                        snapshot.append(state)
                    except (FileNotFoundError, ProcessLookupError):
                        continue
                rows.append(dict(elapsed_s=time.monotonic()-epoch, processes=snapshot))
                time.sleep(.05)
            if process.wait() != 0:
                raise RuntimeError('controlled test failed; inspect '+str(directory.relative_to(REPO)))
        finally:
            cleanup(process, owned)
    if '1 passed; 0 failed' not in (directory/'stdout.log').read_text():
        raise ValueError('expected exactly one executed critical test, not an empty filter')
    (directory/'external-samples.jsonl').write_text(''.join(json.dumps(row)+'\n' for row in rows))
    events = [json.loads(line) for line in (directory/'probe/fault.jsonl').read_text().splitlines()]
    phases = [e['phase'] for e in events if e['event'] == 'phase_snapshot']
    if phases != ['before', 'during', 'after'] or not any(e['event'] == 'sample' for e in events):
        raise ValueError('incomplete passive fault instrumentation')
    return events


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    for name in ('runtime_tests', 'store_tests', 'media_tests'):
        parser.add_argument('--'+name.replace('_', '-'), required=True, type=Path)
    parser.add_argument('--seconds', type=int, default=3)
    parser.add_argument('--repetitions', type=int, default=3)
    args = parser.parse_args()
    root = args.output.resolve()
    if root.parent != REPO/'target/performance' or not 1 <= args.seconds <= 10 or not 1 <= args.repetitions <= 10:
        parser.error('use a new direct child of target/performance; seconds/repetitions in1..10')
    artifacts = {name: getattr(args, name).resolve() for name in ('runtime_tests', 'store_tests', 'media_tests')}
    for executable in artifacts.values():
        if not executable.is_relative_to(REPO/'target') or not executable.is_file():
            parser.error('test artifacts must already exist under this checkout target')
    root.mkdir(mode=0o700)
    metadata = dict(schema_version=1, repetitions=args.repetitions, seconds=args.seconds,
        source_sha256={p:hashlib.sha256((REPO/p).read_bytes()).hexdigest() for p in SOURCES},
        test_artifact_sha256={name:hashlib.sha256(p.read_bytes()).hexdigest() for name,p in artifacts.items()},
        example_artifact_sha256={name:hashlib.sha256((REPO/'target/release/examples'/name).read_bytes()).hexdigest()
                                for name in ('host_fault_profile', 'worker_fault_profile')},
        limits=['component profiles, NOT integrated Server or GUI',
                'host and worker healthy probes paced50ms; measured CPU includes probes and instrument',
                'three critical-test snapshots measure only self process, not child-tree peak',
                'critical tests use their existing clocks/polling; operation records identify wall vs component clock',
                'stalled converter is held rendezvous with no accepted decode, not a hung real codec/driver',
                'CPU ticks and context switches are bounded observations, not exact wakeups'])
    (root/'metadata.json').write_text(json.dumps(metadata, indent=2)+'\n')
    resources, timings, snapshots = [], [], []
    for repetition in range(args.repetitions):
        for scenario, package in [('host_no_reader', 'rad-runtime'), ('worker_death', 'rad-editor')]:
            name = 'host_fault_profile' if package == 'rad-runtime' else 'worker_fault_profile'
            case = root/f'{scenario}-{repetition}'
            print('Measuring',case.name,flush=True)
            rows, events = PROFILE.observe([str(REPO/'target/release/examples'/name), str(root/(case.name+'w')), str(args.seconds)],
                case, .2, args.seconds*3+25)
            if not any(e.get('event') == 'complete' and e.get('clean_shutdown') is True for e in events):
                raise ValueError('missing clean completion')
            for phase in ('before', 'during', 'after'):
                record = PROFILE.summarize_phase(rows, phase)
                record.update(scenario=scenario, repetition=repetition)
                resources.append(record)
            timings.extend(dict(e, scenario=scenario, repetition=repetition, phase=e.get('phase','transition')) for e in events if e.get('event')=='sample')
        for scenario, (artifact, test) in TESTS.items():
            print('Measuring',scenario,repetition,flush=True)
            events = run_test(artifacts[artifact], test, root/f'{scenario}-{repetition}')
            timings.extend(dict(e, repetition=repetition, phase=e.get('phase','transition')) for e in events if e['event']=='sample')
            snapshots.extend(dict(e, repetition=repetition) for e in events if e['event']=='phase_snapshot')
    (root/'resources.json').write_text(json.dumps(resources, indent=2)+'\n')
    (root/'critical-test-snapshots.json').write_text(json.dumps(snapshots, indent=2)+'\n')
    (root/'timing-samples.jsonl').write_text(''.join(json.dumps(e)+'\n' for e in timings))
    (root/'timings.json').write_text(json.dumps(PROFILE.summarize_timings(timings, ('scenario','phase','operation')), indent=2)+'\n')
    print(root.relative_to(REPO),flush=True)


if __name__ == '__main__':
    main()
