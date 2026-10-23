#!/usr/bin/env python3
"""Bounded privileged counters only; default does not inspect or capture anything.

No commands, output paths, raw samples, tracefs writes or persistent settings.
The operator must explicitly authorize --capture through pkexec.
"""
import argparse
import ctypes
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import select
import signal
import struct
import sys
import time

ROLES = {'runtime', 'launcher_helper', 'plugin', 'probe'}
_EMIT_DEADLINE=None
_CONTROLLER_FD=None


def target_filter(tids):
    if (not isinstance(tids,list) or not tids or len(tids)>128 or
            any(type(tid) is not int or not 0<tid<2**31 for tid in tids)):
        raise ValueError('invalid target tids')
    values=sorted(set(tids))
    if len(values)>64:raise ValueError('too many target tids')
    return '('+' || '.join(f'pid == {tid}' for tid in values)+')'


def decode_counter(raw):
    if len(raw)!=24:raise ValueError('incomplete counter read')
    count,enabled,running=struct.unpack('QQQ',raw)
    if not enabled or enabled!=running:
        raise ValueError('counter time incomplete; never scale it to claim exactness')
    return dict(count=count,enabled_ns=enabled,running_ns=running)


def validate_request(value):
    def keys(obj,expected):
        if not isinstance(obj,dict) or set(obj)!=set(expected):raise ValueError('request keys')
    def positive(number):
        if type(number) is not int or not 0<number<2**63:raise ValueError('request integer')
    keys(value,('controller','duration_s','roots','targets'))
    keys(value['controller'],('pid','start'))
    for number in value['controller'].values():positive(number)
    if type(value['duration_s']) is not int or not 1<=value['duration_s']<=20:
        raise ValueError('duration bound')
    for name in ('roots','targets'):
        bound=4 if name=='roots' else 8
        if not isinstance(value[name],list) or not 1<=len(value[name])<=bound:raise ValueError('process bound')
    root_ids=[]
    for root in value['roots']:
        keys(root,('pid','start','session'))
        for number in root.values():positive(number)
        if root['session']!=root['pid']:raise ValueError('root must own its new session')
        root_ids.append(root['pid'])
    pids,tids=[],[]
    for target in value['targets']:
        keys(target,('pid','start','session','role','sha256','threads'))
        for key in ('pid','start','session'):positive(target[key])
        if target['role'] not in ROLES or not re.fullmatch('[0-9a-f]{64}',target['sha256']):
            raise ValueError('target role or executable hash')
        threads=target['threads']
        if not isinstance(threads,list) or not 1<=len(threads)<=64:raise ValueError('thread bound')
        for thread in threads:
            keys(thread,('tid','start'))
            for number in thread.values():positive(number)
            tids.append(thread['tid'])
        if target['pid'] not in [t['tid'] for t in threads]:raise ValueError('missing main thread')
        pids.append(target['pid'])
    if len(set(root_ids))!=len(root_ids) or len(set(pids))!=len(pids) or len(set(tids))!=len(tids):
        raise ValueError('duplicate identity')
    target_filter(tids)
    if value['controller']['pid'] in root_ids+pids:raise ValueError('controller is not a target')
    return value


def proc_stat(pid,tid=None):
    path=Path('/proc')/str(pid)
    if tid is not None:path=path/'task'/str(tid)
    fields=(path/'stat').read_text().rsplit(')',1)[1].split()
    return dict(state=fields[0],ppid=int(fields[1]),session=int(fields[3]),start=int(fields[19]))


def check_uid(pid,uid):
    lines=(Path('/proc')/str(pid)/'status').read_text().splitlines()
    values=next(line.split()[1:] for line in lines if line.startswith('Uid:'))
    if len(values)!=4 or any(int(v)!=uid for v in values):raise ValueError('target uid changed')


def validate_live(request,uid):
    controller=request['controller']
    controller_live=proc_stat(controller['pid'])
    if controller_live['start']!=controller['start'] or controller_live['state']=='Z':
        raise ValueError('controller identity changed')
    check_uid(controller['pid'],uid)
    roots={root['pid']:root for root in request['roots']}
    for root in roots.values():
        live=proc_stat(root['pid']);check_uid(root['pid'],uid)
        if (live['ppid']!=controller['pid'] or live['session']!=root['session'] or
                live['start']!=root['start'] or live['state']=='Z'):
            raise ValueError('root is not a live direct owned child')
    for target in request['targets']:
        pid=target['pid'];live=proc_stat(pid);check_uid(pid,uid)
        if live['start']!=target['start'] or live['session']!=target['session'] or live['state']=='Z':
            raise ValueError('target identity changed')
        ancestor,seen=pid,set()
        while ancestor not in roots:
            if ancestor in seen or len(seen)>=32:raise ValueError('invalid target ancestry')
            seen.add(ancestor);check_uid(ancestor,uid)
            ancestor=proc_stat(ancestor)['ppid']
            if ancestor<=1 or ancestor==controller['pid']:raise ValueError('target outside owned roots')
        actual={int(p.name):proc_stat(pid,int(p.name))['start'] for p in
                (Path('/proc')/str(pid)/'task').iterdir()}
        if actual!={t['tid']:t['start'] for t in target['threads']}:
            raise ValueError('fixed thread coverage changed')
        if hashlib.sha256((Path('/proc')/str(pid)/'exe').read_bytes()).hexdigest()!=target['sha256']:
            raise ValueError('target executable changed')
        if proc_stat(pid)['start']!=target['start']:raise ValueError('target reused during validation')


def online_cpus():
    values=set()
    for part in Path('/sys/devices/system/cpu/online').read_text().strip().split(','):
        bounds=part.split('-')
        if len(bounds)==1:values.add(int(bounds[0]))
        elif len(bounds)==2:values.update(range(int(bounds[0]),int(bounds[1])+1))
        else:raise ValueError('cpu list format')
    if not values or min(values)<0 or max(values)>4095 or len(values)>256:
        raise ValueError('cpu coverage bound')
    return sorted(values)


class PerfAttr(ctypes.Structure):
    _fields_=[('type',ctypes.c_uint32),('size',ctypes.c_uint32),('config',ctypes.c_uint64),
        ('period',ctypes.c_uint64),('sample_type',ctypes.c_uint64),('read_format',ctypes.c_uint64),
        ('flags',ctypes.c_uint64),('wakeup_events',ctypes.c_uint32),('bp_type',ctypes.c_uint32),
        ('config1',ctypes.c_uint64),('config2',ctypes.c_uint64),('branch_type',ctypes.c_uint64),
        ('regs_user',ctypes.c_uint64),('stack_user',ctypes.c_uint32),('clockid',ctypes.c_int32),
        ('regs_intr',ctypes.c_uint64),('aux_watermark',ctypes.c_uint32),('max_stack',ctypes.c_uint16),
        ('reserved2',ctypes.c_uint16),('aux_size',ctypes.c_uint32),('reserved3',ctypes.c_uint32),
        ('sig_data',ctypes.c_uint64)]


class Perf:
    def __init__(self):
        if os.uname().machine!='x86_64' or ctypes.sizeof(PerfAttr)!=128:
            raise ValueError('unsupported perf syscall layout')
        self.libc=ctypes.CDLL(None,use_errno=True)
        self.libc.syscall.restype=ctypes.c_long
        self.event_id=int(Path('/sys/kernel/tracing/events/sched/sched_wakeup/id').read_text())
        if not 0<self.event_id<2**32:raise ValueError('invalid tracepoint id')
        format_text=Path('/sys/kernel/tracing/events/sched/sched_wakeup/format').read_text()
        if 'name: sched_wakeup' not in format_text or not re.search(r'field:[^\n;]+\bpid;',format_text):
            raise ValueError('target pid field not verified')
        self.success_field=bool(re.search(r'field:[^\n;]+\bsuccess;',format_text))

    def open(self,cpu):
        # Tracepoint counters only: disabled, period=0, sample_type=0, no ring buffer.
        attr=PerfAttr(type=2,size=128,config=self.event_id,read_format=3,flags=1)
        fd=self.libc.syscall(ctypes.c_long(298),ctypes.byref(attr),ctypes.c_int(-1),
                             ctypes.c_int(cpu),ctypes.c_int(-1),ctypes.c_ulong(8))
        if fd<0:raise OSError(ctypes.get_errno(),'perf counter open failed')
        return fd

    def filter(self,fd,value):
        if self.success_field:value='('+value+' && success == 1)'
        fcntl.ioctl(fd,0x40082406,value.encode('ascii')+b'\0')
    @staticmethod
    def enable(fd):fcntl.ioctl(fd,0x2400,0)
    @staticmethod
    def disable(fd):fcntl.ioctl(fd,0x2401,0)
    @staticmethod
    def read(fd):return os.read(fd,24)
    @staticmethod
    def close(fd):os.close(fd)


class Counters:
    def __init__(self,backend,cpus,targets):
        self.backend,self.rows=backend,[]
        try:
            groups={role:[t['tid'] for p in targets if p['role']==role for t in p['threads']]
                    for role in sorted({p['role'] for p in targets})}
            for role,tids in groups.items():
                expression=target_filter(tids)
                for cpu in cpus:
                    fd=backend.open(cpu)
                    self.rows.append(dict(fd=fd,cpu=cpu,role=role,target_tids=tids))
                    backend.filter(fd,expression)
        except BaseException:
            self.close();raise

    def start(self):
        for row in self.rows:
            row['enable_before_ns']=time.monotonic_ns()
            self.backend.enable(row['fd'])
            row['enable_after_ns']=time.monotonic_ns()

    def stop(self):
        for row in self.rows:
            row['disable_before_ns']=time.monotonic_ns()
            self.backend.disable(row['fd'])
            row['disable_after_ns']=time.monotonic_ns()
        result=[]
        for row in self.rows:
            result.append({k:v for k,v in row.items() if k!='fd'}|decode_counter(self.backend.read(row['fd'])))
        return result

    def close(self):
        rows,self.rows=self.rows,[]
        errors=[]
        for row in rows:
            try:self.backend.close(row['fd'])
            except OSError as error:errors.append(error)
        if errors:raise errors[0]


class Protocol:
    def __init__(self):self.buffer=b''
    def read(self,timeout,lifelines=()):
        deadline=time.monotonic()+timeout
        while True:
            if lifelines and select.select(list(lifelines),[],[],0)[0]:
                raise ValueError('owned controller or target ended')
            if len(self.buffer)>16384:raise ValueError('protocol size bound')
            if b'\n' in self.buffer:
                line,self.buffer=self.buffer.split(b'\n',1)
                return line.decode('ascii')
            remaining=deadline-time.monotonic()
            if remaining<=0:raise TimeoutError('bounded control deadline')
            ready,_,_=select.select([sys.stdin.fileno(),*lifelines],[],[],remaining)
            if any(fd in ready for fd in lifelines):raise ValueError('owned controller or target ended')
            if sys.stdin.fileno() in ready:
                data=os.read(sys.stdin.fileno(),16385)
                if not data:raise EOFError('control lifeline closed')
                self.buffer+=data


def emit(value):
    payload=(json.dumps(value)+'\n').encode('ascii')
    if len(payload)>65536:raise ValueError('counter output size bound')
    fd=sys.stdout.fileno();deadline=time.monotonic()+2
    if _EMIT_DEADLINE is not None:deadline=min(deadline,_EMIT_DEADLINE)
    offset=0
    while offset<len(payload):
        remaining=deadline-time.monotonic()
        if remaining<=0:raise TimeoutError('bounded counter output')
        watch=[] if _CONTROLLER_FD is None else [_CONTROLLER_FD]
        readable,writable,_=select.select(watch,[fd],[],remaining)
        if readable:raise EOFError('controller ended during output')
        if fd not in writable:raise TimeoutError('bounded counter output')
        try:offset+=os.write(fd,payload[offset:])
        except BlockingIOError:continue


def capture(protocol,request,uid,deadline=None):
    validate_live(request,uid)
    cpus=online_cpus()
    lifelines=[];counters=None
    try:
        for pid in [request['controller']['pid'],*(p['pid'] for p in request['targets'])]:
            lifelines.append(os.pidfd_open(pid))
        validate_live(request,uid)
        backend=Perf();counters=Counters(backend,cpus,request['targets'])
        emit(dict(event='ready',event_name='sched:sched_wakeup',fixed_tids=sum(len(p['threads']) for p in request['targets']),
                  event_id=backend.event_id,success_field=backend.success_field,
                  cpus=cpus,sampling=False,root_writes=False))
        remaining=lambda maximum: min(maximum,deadline-time.monotonic()) if deadline is not None else maximum
        if remaining(10)<=0:raise TimeoutError('batch deadline')
        if protocol.read(remaining(10),lifelines)!='start':raise ValueError('expected start command')
        validate_live(request,uid)
        counters.start()
        emit(dict(event='started'))
        if remaining(request['duration_s'])<=0:raise TimeoutError('batch deadline')
        if protocol.read(remaining(request['duration_s']),lifelines)!='stop':raise ValueError('expected stop command')
        values=counters.stop()
        validate_live(request,uid)
        if online_cpus()!=cpus:raise ValueError('cpu coverage changed')
        counters.close();counters=None
        emit(dict(event='complete',complete=True,counters=values,
            count=sum(row['count'] for row in values),
            derived_rate_per_s=sum(row['count']*1e9/row['enabled_ns'] for row in values),
            scope='fixed owned TIDs; per-emitter-CPU intervals, not a simultaneous global cut',
            sampling=False,scaled=False,ring_buffer_losses='not applicable: counters without samples',
            limitations=['not CPU C-state exits, causes or wakeup-to-run latency',
                         'transient/new threads and unselected descendants are outside fixed coverage'],
            all_counter_fds_closed=True))
    finally:
        if counters is not None:counters.close()
        for fd in lifelines:os.close(fd)


def main(argv=None):
    global _EMIT_DEADLINE,_CONTROLLER_FD
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--capture',action='store_true')
    args = parser.parse_args(argv)
    if not args.capture:
        print('DRY: no privilege, process or kernel inspection')
        return
    if os.geteuid()!=0:raise PermissionError('explicit administrative authorization required')
    caller=os.environ.get('PKEXEC_UID','')
    if not caller.isdecimal() or int(caller)==0:raise PermissionError('pkexec caller uid required')
    os.set_blocking(sys.stdout.fileno(),False)
    def cancel(_signum,_frame):
        signal.signal(signal.SIGTERM,signal.SIG_IGN);signal.signal(signal.SIGINT,signal.SIG_IGN)
        raise InterruptedError('counter capture cancelled')
    signal.signal(signal.SIGTERM,cancel);signal.signal(signal.SIGINT,cancel)
    controller_fd=None
    try:
        protocol=Protocol();controller=None;deadline=time.monotonic()+180
        _EMIT_DEADLINE=deadline
        for index in range(9):
            remaining=min(15,deadline-time.monotonic())
            if remaining<=0:raise TimeoutError('batch deadline')
            line=protocol.read(remaining,() if controller_fd is None else (controller_fd,))
            if line=='finish' and controller is not None:
                emit(dict(event='finished',complete=True,windows=index,all_counter_fds_closed=True))
                return
            if index==8:raise ValueError('batch window bound')
            request=validate_request(json.loads(line))
            if os.getppid()!=request['controller']['pid']:raise ValueError('controller is not direct parent')
            if controller is None:
                controller=request['controller'];controller_fd=os.pidfd_open(controller['pid'])
                _CONTROLLER_FD=controller_fd
            elif controller!=request['controller']:raise ValueError('controller cannot change')
            capture(protocol,request,int(caller),deadline)
    except BaseException as error:
        try:emit(dict(event='failed',complete=False,error_type=type(error).__name__,errno=getattr(error,'errno',None)))
        except (OSError,TimeoutError,ValueError):pass
        raise SystemExit(1) from None
    finally:
        if controller_fd is not None:os.close(controller_fd)


if __name__ == '__main__':
    main()
