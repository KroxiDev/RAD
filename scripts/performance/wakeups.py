#!/usr/bin/env python3
"""Opt-in exact kernel counters for fixed owned TIDs; no GUI, XL or services.

One bounded pkexec helper batch, no installed tools or persistent kernel changes.
Default does not inspect anything. Counts have per-CPU intervals, not a global cut.
"""
import argparse
import hashlib
import importlib.util
import json
import mmap
import os
from pathlib import Path
import queue
import select
import shutil
import signal
import subprocess
import struct
import sys
import threading
import time

REPO=Path(__file__).resolve().parents[2]
SPEC=importlib.util.spec_from_file_location('wakeups_lifecycle',Path(__file__).with_name('desktop.py'))
D=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(D)
C,S=D.COMMON,D.SAMPLER
BOUND_EXEC='''import ctypes,os,signal,sys
if ctypes.CDLL(None).prctl(1,signal.SIGKILL,0,0,0)!=0: raise SystemExit(90)
if os.getppid()!=int(sys.argv[1]): raise SystemExit(91)
os.execv(sys.argv[2],sys.argv[2:])
'''
PROBE='''import mmap,struct,sys
with open(sys.argv[1],'r+b',buffering=0) as handle:
 ack=mmap.mmap(handle.fileno(),8);ack[:]=struct.pack('Q',0)
 print('ready',flush=True)
 n=0
 while sys.stdin.buffer.read(1):
  n+=1;ack[:]=struct.pack('Q',n)
'''


def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()


class World(C.World):
    def __init__(self,root,log):
        self.root,self.log=root,log
        self.roots,self.owned=[],{}
        self.runtime=self.runtime_pid=None
        self.events,self.unobserved=[],set()
        root.mkdir(mode=0o700)
        self.env=S.clean_environment(root/'env')

    def spawn(self,command,role,drain=True):
        with D.registered_spawn():
            child=super().spawn([sys.executable,'-I','-S','-B','-c',BOUND_EXEC,str(os.getpid()),*command],
                                role,drain=False)
            if drain:
                child.lines=queue.Queue(maxsize=4096)
                child.reader_error=None
                def consume():
                    try:
                        for line in child.stdout:
                            if len(line)>65536:raise ValueError('fixture output bound')
                            child.lines.put_nowait(line.strip())
                    except BaseException as error:child.reader_error=type(error).__name__
                child.reader=threading.Thread(target=consume,daemon=True);child.reader.start()
            return child


class PrivilegedBatch:
    def __init__(self,helper,log):
        self.helper,self.log,self.child=helper,log,None
        self.events=[]
        self.identity=None
        self.cleanup={}
        self.close_timeout=5

    def start(self):
        with D.registered_spawn():
            # No internal stdin authentication agent: passwords never enter protocol/chat.
            self.child=subprocess.Popen(['/usr/bin/pkexec','--disable-internal-agent',
                '/usr/bin/python3','-I','-S','-B',str(self.helper),'--capture'],
                stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=self.log,
                env={'PATH':'/usr/bin:/bin','LANG':'C.UTF-8'},text=True,bufsize=1)
            stat=S.parse_stat((Path('/proc')/str(self.child.pid)/'stat').read_text())
            self.identity=(self.child.pid,stat['start_ticks'],stat['session'])

    def send(self,value):
        self.child.stdin.write(value+'\n');self.child.stdin.flush()

    def event(self,expected,timeout=15):
        if not select.select([self.child.stdout],[],[],timeout)[0]:
            raise TimeoutError('bounded privileged helper/authentication deadline')
        line=self.child.stdout.readline(65537)
        if not line or len(line)>65536:raise RuntimeError('privileged helper ended or output exceeded bound')
        value=json.loads(line);self.events.append(value)
        if value.get('event')!=expected:raise RuntimeError('privileged counter did not complete expected step')
        return value

    def begin(self,request,first=False):
        self.send(json.dumps(request));self.event('ready',180 if first else 15)
        self.send('start');self.event('started')

    def end(self):
        self.send('stop');result=self.event('complete')
        if result.get('complete') is not True or result.get('all_counter_fds_closed') is not True:
            raise RuntimeError('incomplete kernel counter window')
        return result

    def finish(self):
        self.send('finish');self.event('finished')
        self.child.stdin.close()
        if self.child.wait(timeout=5)!=0:raise RuntimeError('counter helper did not exit cleanly')
        self.child.stdout.close()

    def close(self):
        if self.child is None:return
        if self.child.stdin and not self.child.stdin.closed:self.child.stdin.close()
        try:self.child.wait(timeout=self.close_timeout)
        except subprocess.TimeoutExpired:
            self.cleanup['eof_wait_expired']=True
            # A pkexec waiting for authentication still has the caller's REAL uid.
            # Only that exact owned launcher can receive SIGTERM through a pidfd.
            # An already-root helper is never signalled; it owns EOF/deadline cleanup.
            sent=False
            if self.identity is not None:
                pid,start,session=self.identity
                try:
                    fd=os.pidfd_open(pid)
                    try:
                        values=(Path('/proc')/str(pid)/'status').read_text().splitlines()
                        real_uid=int(next(line for line in values if line.startswith('Uid:')).split()[1])
                        if real_uid==os.getuid() and real_uid!=0 and S.same_owned_process(pid,start,session):
                            try:signal.pidfd_send_signal(fd,signal.SIGTERM);sent=True
                            except PermissionError:self.cleanup['launcher_uid_changed']=True
                    finally:os.close(fd)
                except ProcessLookupError:pass
            self.cleanup['owned_launcher_sigterm']=sent
            try:self.child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                raise RuntimeError('privileged helper/authentication failed bounded EOF cleanup') from None
        finally:
            self.cleanup['returncode']=self.child.poll()
            if self.child.stdout and not self.child.stdout.closed:self.child.stdout.close()


def validate_phase_window(edges,phase,result):
    begins=[e.get('at_monotonic_ns') for e in edges if e.get('event')=='phase_begin' and e.get('phase')==phase]
    ends=[e.get('at_monotonic_ns') for e in edges if e.get('event')=='phase_end' and e.get('phase')==phase]
    if len(begins)!=1 or len(ends)!=1 or any(type(v) is not int for v in begins+ends):
        raise ValueError('missing unique kernel-clock phase boundaries')
    if not result.get('counters') or not 0<begins[0]<ends[0]:raise ValueError('invalid phase interval')
    if any(not begins[0]<=row['enable_before_ns']<row['disable_after_ns']<=ends[0]
           for row in result['counters']):
        raise ValueError('counter interval escaped its declared fixture phase')
    return True


def persist_metadata(out,metadata):
    pending=out/'metadata.pending'
    with pending.open('w') as handle:
        json.dump(metadata,handle,indent=2);handle.write('\n');handle.flush();os.fsync(handle.fileno())
    pending.replace(out/'metadata.json')
    fd=os.open(out,os.O_RDONLY|os.O_DIRECTORY)
    try:os.fsync(fd)
    finally:os.close(fd)


def finalize_batch(batch,out,metadata):
    error=None
    try:
        if batch is not None:batch.close()
    except BaseException as failure:
        error=failure
        metadata.update(complete=False,cleanup_error_type=type(failure).__name__,all_owned_reaped=False)
    finally:
        try:
            if batch is not None:
                metadata['privileged_cleanup']=batch.cleanup
                (out/'helper-events.json').write_text(json.dumps(batch.events,indent=2)+'\n')
        finally:persist_metadata(out,metadata)
    if error is not None:raise error


def identities(root,roles):
    targets=[]
    for pid,role in roles.items():
        stat=S.parse_stat((Path('/proc')/str(pid)/'stat').read_text())
        if stat['session']!=root.pid or stat['state']=='Z':raise ValueError('target escaped owned session')
        threads=[]
        for task in (Path('/proc')/str(pid)/'task').iterdir():
            value=S.parse_stat((task/'stat').read_text())
            threads.append(dict(tid=int(task.name),start=value['start_ticks']))
        targets.append(dict(pid=pid,start=stat['start_ticks'],session=stat['session'],role=role,
                            sha256=digest(Path('/proc')/str(pid)/'exe'),threads=threads))
    controller=S.parse_stat((Path('/proc')/str(os.getpid())/'stat').read_text())
    own=S.parse_stat((Path('/proc')/str(root.pid)/'stat').read_text())
    return dict(controller=dict(pid=os.getpid(),start=controller['start_ticks']),duration_s=20,
                roots=[dict(pid=root.pid,start=own['start_ticks'],session=own['session'])],targets=targets)


def sleeping(child):
    deadline=time.monotonic()+3
    while time.monotonic()<deadline:
        if child.poll() is not None:raise RuntimeError('owned calibration probe ended')
        if S.parse_stat((Path('/proc')/str(child.pid)/'stat').read_text())['state']=='S':
            time.sleep(.01)
            if S.parse_stat((Path('/proc')/str(child.pid)/'stat').read_text())['state']=='S':return
        time.sleep(.001)
    raise TimeoutError('probe did not block before next wake')


def calibrate(batch,out):
    with (out/'calibration.stderr.log').open('w') as log:
        world=World(out/'calibration',log)
        mappings=[]
        try:
            children=[]
            for name in ('positive','negative'):
                ack=world.root/(name+'.ack')
                with ack.open('xb') as handle:handle.write(b'\0'*8)
                ack.chmod(0o600)
                with ack.open('r+b',buffering=0) as handle:mappings.append(mmap.mmap(handle.fileno(),8))
                children.append(world.spawn([sys.executable,'-I','-S','-B','-c',PROBE,str(ack)],name+'_probe'))
            positive,negative=children
            for child in (positive,negative):
                if world.line(child)!='ready':raise RuntimeError('probe did not become ready')
                sleeping(child)
            request=identities(positive,{positive.pid:'probe'})
            batch.begin(request,first=True)
            for index in range(10):
                # Waker is the unselected controller; another unselected target is
                # deliberately woken too. Only positive's ten wakeups may count.
                # ACK via shared memory avoids target-generated pipe wakeups: a
                # wrong emitter/common_pid filter must not also happen to count ten.
                for child,ack in zip(children,mappings):
                    sleeping(child);child.stdin.write('x');child.stdin.flush()
                    deadline=time.monotonic()+3
                    while struct.unpack('Q',ack[:])[0]!=index+1:
                        if time.monotonic()>deadline:raise TimeoutError('shared-memory acknowledgement missing')
                        time.sleep(.001)
            sleeping(positive);result=batch.end()
            if result['count']!=10:raise RuntimeError('kernel target filter calibration count mismatch')
            (out/'calibration.json').write_text(json.dumps(dict(request=request,result=result,
                expected_target_wakeups=10,negative_target_wakeups=10,external_waker=True,
                ack_mode='shared_memory_poll_no_ack_pipe'),indent=2)+'\n')
        finally:
            with D.protected_cleanup():
                world.close()
                for ack in mappings:ack.close()


def fixture_event(world,child,raw,accept,timeout=35):
    deadline=time.monotonic()+timeout
    while time.monotonic()<deadline:
        if child.reader_error:raise RuntimeError('bounded fixture output reader failed')
        try:line=child.lines.get(timeout=min(1,max(.001,deadline-time.monotonic())))
        except queue.Empty:
            if child.poll() is not None:raise RuntimeError('fixture ended without required event')
            continue
        value=json.loads(line);raw.write(json.dumps(value)+'\n')
        if value.get('event') in ('phase_begin','phase_end'):
            if not hasattr(child,'phase_edges'):child.phase_edges=[]
            child.phase_edges.append(value)
        if accept(value):raw.flush();return value
    raise TimeoutError('fixture phase deadline')


def case(batch,out,elf,count,seconds):
    with (out/f'n{count}.stderr.log').open('w') as log, (out/f'n{count}.fixture.jsonl').open('w') as raw:
        world=World(out/f'n{count}',log)
        try:
            child=world.spawn([str(elf/'examples/runtime_profile'),str(world.root/'work'),str(count),'16'],
                              'runtime_fixture')
            windows=[]
            for phase in ('idle','load'):
                fixture_event(world,child,raw,lambda e:e.get('event')=='phase_begin' and e['phase']==phase)
                roles={}
                for pid in S.descendants(child.pid):
                    sha=digest(Path('/proc')/str(pid)/'exe')
                    if pid==child.pid and sha==digest(elf/'examples/runtime_profile'):role='runtime'
                    elif sha==digest(elf/'rad-launcher-helper'):role='launcher_helper'
                    elif sha==digest(elf/'examples/runtime_profile'):role='plugin'
                    else:raise RuntimeError('unrecognized owned executable during fixed coverage setup')
                    roles[pid]=role
                if sorted(roles.values())!=sorted(['runtime','launcher_helper']+['plugin']*count):
                    raise RuntimeError('fixture process coverage differs from declared count')
                request=identities(child,roles)
                batch.begin(request);time.sleep(seconds);result=batch.end()
                windows.append(dict(phase=phase,plugins=count,request=request,result=result))
                print(f'COUNTED plugins={count} phase={phase} wakeups={result["count"]}',flush=True)
            complete=fixture_event(world,child,raw,lambda e:e.get('event')=='complete')
            if complete.get('clean_shutdown') is not True or child.wait(timeout=5)!=0:
                raise RuntimeError('instrumental runtime did not close cleanly')
            for window in windows:validate_phase_window(child.phase_edges,window['phase'],window['result'])
            world.close()
            (out/f'n{count}.counters.json').write_text(json.dumps(windows,indent=2)+'\n')
            return windows
        finally:
            with D.protected_cleanup():world.close()


def main(argv=None):
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output',type=Path)
    parser.add_argument('--run-counters',action='store_true')
    parser.add_argument('--calibration-only',action='store_true')
    parser.add_argument('--seconds',type=int,default=8)
    args=parser.parse_args(argv)
    if not args.run_counters:
        if args.calibration_only:parser.error('--calibration-only requires --run-counters')
        print('DRY: no privilege, kernel, process, service or hardware inspection');return
    out=args.output.resolve()
    if out.parent!=REPO/'target/performance' or out.exists() or out!=args.output.absolute() or len(str(out/'n4/work/runtime/editor.sock').encode())>=108:
        parser.error('new short direct child of target/performance without symlinks')
    if not 3<=args.seconds<=8:parser.error('counter windows 3..8s inside 16s fixture phases')
    D.install_shutdown_signals()
    out.mkdir(mode=0o700);elf=out/'elf';(elf/'examples').mkdir(mode=0o700,parents=True)
    inputs={'wakeup_counter.py':REPO/'scripts/performance/wakeup_counter.py'}
    if not args.calibration_only:
        inputs.update({'examples/runtime_profile':REPO/'target/release/examples/runtime_profile',
                       'rad-launcher-helper':REPO/'target/release/rad-launcher-helper'})
    input_hashes={name:digest(path) for name,path in inputs.items()}
    for name,path in inputs.items():shutil.copy2(path,elf/name)
    if any(digest(elf/name)!=sha for name,sha in input_hashes.items()):
        raise RuntimeError('input changed while retaining pre-execution copy')
    metadata=dict(base_commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=REPO,text=True).strip(),
        inputs_sha256=input_hashes,
        sources_sha256={str(path):digest(path) for path in map(Path,(
            'scripts/performance/wakeups.py','scripts/performance/wakeup_counter.py',
            'scripts/performance/desktop.py','scripts/performance/profile.py','scripts/performance/officials.py',
            'crates/rad-runtime/examples/runtime_profile.rs'))},
        privileged_interpreter_sha256=digest(Path('/usr/bin/python3')),complete=False,
        pkexec_sha256=digest(Path('/usr/bin/pkexec')),calibration_only=args.calibration_only,
        kernel=os.uname().release,architecture=os.uname().machine,
        limits=['fixed owned TIDs and per-emitter-CPU intervals, not a global simultaneous window',
                'inert echo fixtures, not real official plugin wakeups; no GUI/XL/services',
                'runtime fixture includes in-process client thread and preparation allocation history',
                'kernel event counts are not CPU C-state exits, causes or wakeup-to-run latency',
                'rates are derived sums normalized by each CPU counter enabled time',
                'no samples/ring buffers, kernel sysctl or tracefs writes, extra tools or persistent capabilities'])
    metadata['retained_sources']={}
    for name,sha in metadata['sources_sha256'].items():
        target=out/'sources'/name
        target.parent.mkdir(mode=0o700,parents=True,exist_ok=True)
        shutil.copy2(name,target)
        if digest(target)!=sha:raise RuntimeError('source changed during preexec retention')
        metadata['retained_sources'][name]=str(target.relative_to(out))
    metadata['tool_copies']={}
    for name,path in (('pkexec',Path('/usr/bin/pkexec')),('privileged-python',Path('/usr/bin/python3')),
                      ('controller-probe-python',Path(sys.executable))):
        sha=digest(path);target=elf/('audit-'+name)
        shutil.copy2(path,target);target.chmod(0o400)  # Never execute a copied setuid/interpreter artifact.
        if digest(target)!=sha:raise RuntimeError('tool changed during preexec retention')
        metadata['tool_copies'][name]=dict(sha256=sha,path=str(target.relative_to(out)),audit_copy_only=True)
    persist_metadata(out,metadata)
    batch=None
    with (out/'privileged.stderr.log').open('w') as log:
        try:
            batch=PrivilegedBatch(elf/'wakeup_counter.py',log);batch.start()
            calibrate(batch,out)
            results=[]
            if not args.calibration_only:
                for count in (0,1,4):results.extend(case(batch,out,elf,count,args.seconds))
            batch.finish()
            for name,sha in metadata['inputs_sha256'].items():
                if digest(elf/name)!=sha:raise RuntimeError('retained input changed during counters')
            for name,sha in metadata['sources_sha256'].items():
                if digest(Path(name))!=sha:raise RuntimeError('measured source changed during counters')
            if (digest(Path('/usr/bin/pkexec'))!=metadata['pkexec_sha256'] or
                digest(Path('/usr/bin/python3'))!=metadata['privileged_interpreter_sha256']):
                raise RuntimeError('privileged launcher/interpreter changed during counters')
            if digest(Path(sys.executable))!=metadata['tool_copies']['controller-probe-python']['sha256']:
                raise RuntimeError('controller/probe interpreter changed during counters')
            (out/'counters.json').write_text(json.dumps(results,indent=2)+'\n')
            metadata.update(complete=True,calibration_passed=True,all_owned_reaped=True,
                            all_counter_fds_closed=True,source_fingerprints_match=True)
            print('COMPLETE '+str(out.relative_to(REPO)),flush=True)
        except BaseException as error:
            metadata.update(complete=False,error_type=type(error).__name__)
            raise
        finally:
            with D.protected_cleanup():
                finalize_batch(batch,out,metadata)


if __name__=='__main__':main()
