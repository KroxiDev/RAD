#!/usr/bin/env python3
"""Compare private copies of release binaries, offline and without adopting flags."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import time

REPO = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location('rad_observer', REPO/'scripts/performance/profile.py')
observer = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(observer)
NAMES = ('rad-runtime', 'rad-launcher-helper')


def timing_summary(values):
    return dict(n=len(values), p50_ns=observer.percentile(values, 50), max_ns=max(values))


def record(path):
    return dict(bytes=path.stat().st_size, sha256=hashlib.sha256(path.read_bytes()).hexdigest())


def fingerprint():
    return {p:record(REPO/p)['sha256'] for p in
            ('scripts/performance/builds.py','scripts/performance/profile.py')}


def build(output):
    output.mkdir(mode=0o700)
    env = dict(os.environ)
    for key in list(env):
        if key.startswith('CARGO_PROFILE_RELEASE_') or key in ('RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS'):
            env.pop(key)
    env.update(CARGO_HOME=str(REPO/'target/cargo-home'), CARGO_BUILD_JOBS='2', CARGO_INCREMENTAL='0')
    command = ['cargo','build','--offline','--locked','--release','--all-features',
               '-p','rad-runtime','-p','rad-launcher','--bin','rad-runtime','--bin','rad-launcher-helper']
    variants = {}
    for name, target, extra in [
        ('reference',REPO/'target',[]),
        ('thin-lto',REPO/'target/performance/build-cache/thin', ['--config','profile.release.lto="thin"'])]:
        directory = output/name
        directory.mkdir(mode=0o700)
        start = time.monotonic_ns()
        with (directory/'build.log').open('w') as log:
            subprocess.run(command+extra, cwd=REPO, env=dict(env,CARGO_TARGET_DIR=str(target)),
                           stdout=log, stderr=subprocess.STDOUT, check=True, timeout=1800)
        for binary in NAMES:
            shutil.copy2(target/'release'/binary,directory/binary)
        variants[name] = dict(flags=extra, build_wall_ns=time.monotonic_ns()-start,
                              binaries={binary:record(directory/binary) for binary in NAMES})
    directory = output/'strip-all'
    directory.mkdir(mode=0o700)
    for binary in NAMES:
        shutil.copy2(output/'reference'/binary,directory/binary)
        subprocess.run(['strip','--strip-all',str(directory/binary)],check=True,timeout=30)
    variants['strip-all'] = dict(flags=['strip --strip-all on reference copies'],
                                binaries={binary:record(directory/binary) for binary in NAMES})
    metadata = dict(variants=variants, source_sha256=fingerprint(), command=command,
        typed_client=record(REPO/'target/release/examples/conformance_client'),
        git_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=REPO,text=True).strip(),
        rustc=subprocess.check_output(['rustc','--version'],text=True).strip(),
        cargo=subprocess.check_output(['cargo','--version'],text=True).strip(),
        limits=['build wall time includes differing cache state, not a compiler speed benchmark',
                'no editor/GUI measured','no cold disk cache claim','flags are not adopted'])
    (output/'builds.json').write_text(json.dumps(metadata,indent=2)+'\n')
    print('BUILD COMPLETE '+str(output.relative_to(REPO)),flush=True)


class Peer:
    """Use the existing typed Rust client; never maintain another wire decoder."""
    def __init__(self, runtime, env, error):
        self.child = subprocess.Popen([str(REPO/'target/release/examples/conformance_client'),
            str(runtime),'editor'],env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,
            stderr=error,text=True,bufsize=1)

    def call(self, op):
        self.child.stdin.write(json.dumps([op,{}])+'\n')
        self.child.stdin.flush()
        if not select.select([self.child.stdout],[],[],10)[0]:
            raise TimeoutError('typed client readiness')
        response = json.loads(self.child.stdout.readline())
        if response.get('error'):
            raise ValueError(response)
        return response['data']

    def close(self):
        if self.child.stdin.closed:
            return
        self.child.stdin.close()
        try:
            if self.child.wait(timeout=5) != 0:
                raise RuntimeError('typed client failed')
        except subprocess.TimeoutExpired:
            self.child.kill(); self.child.wait(timeout=5)
            raise
        finally:
            self.child.stdout.close()


def boot(binary, directory, env, initialized):
    observed, peer = {}, None
    rows = []
    with (directory/(initialized+'-stderr.log')).open('wb') as error:
        start = time.monotonic_ns()
        process = subprocess.Popen([str(binary)],env=env,stdin=subprocess.DEVNULL,
                                   stdout=subprocess.DEVNULL,stderr=error,start_new_session=True)
        try:
            socket = Path(env['XDG_RUNTIME_DIR'])/'rad/editor.sock'
            deadline = time.monotonic()+15
            # Do not reuse a stale socket left by the previous session: clean exit removes it.
            while not socket.exists():
                if process.poll() is not None:
                    raise RuntimeError('private runtime failed before socket readiness')
                if time.monotonic()>deadline:
                    raise TimeoutError('private runtime readiness')
                time.sleep(.0005)
            peer = Peer(Path(env['XDG_RUNTIME_DIR']),env,error)
            status = peer.call('status')
            if status['deck_state'] != 'disabled':
                raise RuntimeError('private machine profile must have deck disabled')
            ready_ns = time.monotonic_ns()-start
            peer.close();peer=None
            time.sleep(.2)  # Let disconnect processing finish before the idle window.
            epoch = time.monotonic()
            for _ in range(6):
                snapshot = []
                for pid in observer.descendants(process.pid):
                    try:
                        state = observer.read_process(pid,process.pid)
                        observed[pid] = state['start_ticks']
                        state['role'] = 'runtime' if pid==process.pid else 'launcher_helper'
                        snapshot.append(state)
                    except (FileNotFoundError,ProcessLookupError):
                        continue
                rows.append(dict(elapsed_s=time.monotonic()-epoch,phase='idle',processes=snapshot))
                time.sleep(.2)
            process.terminate()
            if process.wait(timeout=8) != 0:
                raise RuntimeError('private runtime did not exit cleanly')
        finally:
            peer_error=None
            if peer is not None:
                try:peer.close()
                except Exception as error:peer_error=error
            if process.poll() is None:
                # Discover children even when readiness failed before the first sample.
                for pid in observer.descendants(process.pid):
                    try:
                        observed[pid] = observer.parse_stat((Path('/proc')/str(pid)/'stat').read_text())['start_ticks']
                    except FileNotFoundError:
                        pass
                process.terminate()
                try:process.wait(timeout=8)
                except subprocess.TimeoutExpired:process.kill();process.wait(timeout=5)
            deadline=time.monotonic()+3
            while any(observer.same_owned_process(p,s,process.pid) for p,s in observed.items()) and time.monotonic()<deadline:
                time.sleep(.05)
            survivors=[p for p,s in observed.items() if observer.same_owned_process(p,s,process.pid)]
            for pid in survivors:observer.terminate_owned(pid,observed[pid],process.pid)
            if survivors:raise RuntimeError('owned helper survived clean shutdown')
            if peer_error is not None:raise RuntimeError('typed client cleanup failed') from peer_error
    (directory/(initialized+'-samples.jsonl')).write_text(''.join(json.dumps(r)+'\n' for r in rows))
    result=observer.summarize_phase(rows,'idle')
    if not result['stable_process_set'] or result['processes_sampled_peak'] != 2:
        raise ValueError('expected stable runtime plus helper, no plugins')
    return dict(startup_observed_ns=ready_ns,document=initialized,resources=result)


def measure(output,repetitions):
    metadata=json.loads((output/'builds.json').read_text())
    if metadata['source_sha256'] != fingerprint():
        raise ValueError('controller source changed since variant build; use a new output directory')
    if metadata['typed_client'] != record(REPO/'target/release/examples/conformance_client'):
        raise ValueError('typed client binary changed since variant build')
    for variant,data in metadata['variants'].items():
        for binary,expected in data['binaries'].items():
            if record(output/variant/binary)!=expected:raise ValueError('variant binary changed')
    runs=output/'runs';runs.mkdir(mode=0o700)
    results=[]
    names=['reference','strip-all','thin-lto']
    for repeat in range(repetitions):
        # Rotate to avoid confounding variant with position in a warm cache sequence.
        for variant in names[repeat%3:]+names[:repeat%3]:
            root=runs/(str(repeat)+'-'+variant)
            root.mkdir(mode=0o700)
            # Short XDG paths keep AF_UNIX addresses below their Linux length limit.
            env={'PATH':'/usr/bin:/bin','LANG':'C.UTF-8'}
            for key,name in [('HOME','h'),('XDG_RUNTIME_DIR','r'),('XDG_DATA_HOME','d'),
                ('XDG_CONFIG_HOME','c'),('XDG_CACHE_HOME','k'),('XDG_STATE_HOME','s'),
                ('XDG_DATA_DIRS','a'),('XDG_CONFIG_DIRS','g')]:
                path=root/name;path.mkdir(mode=0o700);env[key]=str(path)
            env['DBUS_SESSION_BUS_ADDRESS']='unix:path='+str(root/'no-bus')
            data=Path(env['XDG_DATA_HOME'])/'rad'
            data.mkdir(mode=0o700)
            fd=os.open(data/'machine.json',os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
            with os.fdopen(fd,'w') as settings:
                json.dump(dict(version=1,use_stream_deck=False,start_with_session=False),settings)
            for document in ('fresh','existing'):
                print('MEASURE',repeat,variant,document,flush=True)
                result=boot(output/variant/'rad-runtime',root,env,document)
                result.update(repetition=repeat,variant=variant)
                results.append(result)
    (output/'results.json').write_text(json.dumps(results,indent=2)+'\n')
    summary=[]
    for variant in names:
        for document in ('fresh','existing'):
            selected=[r for r in results if r['variant']==variant and r['document']==document]
            row=dict(variant=variant,document=document,**timing_summary([r['startup_observed_ns'] for r in selected]))
            row['pss_bytes_median_of_windows']=observer.percentile([r['resources']['pss_bytes_median'] for r in selected],50)
            row['rss_bytes_median_of_windows']=observer.percentile([r['resources']['rss_bytes_median'] for r in selected],50)
            summary.append(row)
    (output/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    print('MEASURE COMPLETE '+str(output.relative_to(REPO)),flush=True)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output',type=Path)
    parser.add_argument('--stage',choices=['build','measure','all'],default='all')
    parser.add_argument('--repetitions',type=int,default=12)
    args=parser.parse_args()
    output=args.output.resolve()
    if output.parent != REPO/'target/performance' or not 3 <= args.repetitions <= 20:
        parser.error('use a direct child of target/performance and 3..20 repetitions')
    if args.stage in ('build','all'):build(output)
    if args.stage in ('measure','all'):measure(output,args.repetitions)


if __name__=='__main__':main()
