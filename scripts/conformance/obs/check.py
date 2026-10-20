#!/usr/bin/env python3
"""Private D4 integration; only own processes, ephemeral TCP and temporary AF_UNIX."""
import json, os, queue, signal, socket, struct, subprocess, sys, threading, time
from pathlib import Path

def equal(label, actual, expected):
    assert actual == expected, (label, actual, expected)
    print('PASS', label, flush=True)

def until(read, predicate, timeout=12):
    end=time.monotonic()+timeout
    while True:
        value=read()
        if predicate(value): return value
        assert time.monotonic()<end, value
        time.sleep(.025) # observations only; never retries effects

class Peer:
    def __init__(self, root, endpoint='editor'):
        self.sock=socket.socket(socket.AF_UNIX); self.sock.settimeout(12)
        self.sock.connect(str(root/'runtime/rad'/f'{endpoint}.sock')); self.serial=0; self.call('hello')
    def read(self,n):
        out=b''
        while len(out)<n:
            chunk=self.sock.recv(n-len(out)); assert chunk; out+=chunk
        return out
    def request(self,op,**args):
        self.serial+=1
        payload=json.dumps(dict(v=1,id=str(self.serial),op=op,args=args)).encode()
        self.sock.sendall(struct.pack('>I',len(payload))+payload)
        size,=struct.unpack('>I',self.read(4)); assert 0<size<=65536
        response=json.loads(self.read(size)); assert response['id']==str(self.serial)
        return response
    def call(self,op,**args):
        result=self.request(op,**args); assert not result.get('error'), (op,result)
        return result['data']
    def close(self): self.sock.close()

class World:
    def __init__(self,root,repo):
        self.root,self.repo=root,repo; self.children=[]; self.peers=[]
        for name in ['runtime','home','config','cache','state','system','tmp']:
            (root/name).mkdir(mode=0o700)
        self.env=dict(PATH='/usr/bin:/bin',HOME=str(root/'home'),XDG_RUNTIME_DIR=str(root/'runtime'),
          XDG_DATA_HOME=str(root/'data'),XDG_CONFIG_HOME=str(root/'config'),XDG_CACHE_HOME=str(root/'cache'),
          XDG_STATE_HOME=str(root/'state'),XDG_DATA_DIRS=str(root/'system'),XDG_CONFIG_DIRS=str(root/'system'),
          TMPDIR=str(root/'tmp'),DBUS_SESSION_BUS_ADDRESS='unix:path='+str(root/'no-bus'))
    def spawn(self,args):
        p=subprocess.Popen(args,env=self.env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,bufsize=1)
        self.children.append(p); return p
    def fake(self):
        self.server=self.spawn([str(self.repo/'target/plugins/release/examples/obs-conformance-server')])
        self.lines=queue.Queue()
        def drain():
            for line in self.server.stdout: self.lines.put(json.loads(line))
        threading.Thread(target=drain,daemon=True).start()
        ready=self.lines.get(timeout=10); assert ready['port'] not in (0,4455)
        port=self.root/'runtime/rad-obs-fixture.port'; port.write_text(str(ready['port'])); port.chmod(0o600)
    def control(self,op,**args):
        self.server.stdin.write(json.dumps(dict(op=op,**args))+'\n');self.server.stdin.flush()
        return self.lines.get(timeout=10)
    def start(self, expected_host='absent'):
        self.runtime=self.spawn([str(self.repo/'target/release/rad-runtime')])
        until(lambda:(self.root/'runtime/rad/editor.sock').exists(),bool)
        self.e=Peer(self.root);self.peers.append(self.e)
        until(lambda:self.e.call('plugin_status'),lambda v:v['state']=='idle' and len(v['items'])==1 and v['items'][0]['host']==expected_host)
        equal('deck disabled',self.e.call('status')['deck_state'],'disabled')
    def cli(self,*args,code=0):
        r=subprocess.run([str(self.repo/'target/release/rad-cli'),*args,'--json'],env=self.env,capture_output=True,text=True,timeout=15)
        equal('CLI exit '+args[0],r.returncode,code)
        return json.loads(r.stdout)
    def close(self):
        for p in self.peers:p.close()
        for p in reversed(self.children):
            if p.poll() is None:
                p.terminate()
                try:p.wait(timeout=8)
                except subprocess.TimeoutExpired:p.kill();p.wait(timeout=3)
            error=p.stderr.read()
            assert not error, ('child stderr',error)

def parameters(**values):return dict(schema_version=1,values=values)
def target(name,kind='scene',item=None,source=None):
    return json.dumps(dict(v=1,collection='C',kind=kind,name=name,item=item,source=source),separators=(',',':'))

def scenario(w):
    w.fake();w.start();e=w.e
    folder=e.call('status')['active_folder_id']
    definitions=[('scene',dict(scene=target('Live'))),('filter',dict(source=target('Camera','input'),filter='Blur',operation='on')),
      ('item',dict(scene=target('Root'),item=target('G','group',8,'Camera'),operation='show')),
      ('record',dict(operation='start')),('stream',dict(operation='start')),('wait',dict())]
    for i,(kind,values) in enumerate(definitions):
        e.call('create_button',folder_id=folder,position=i,name=kind)
        e.call('add_action',button_id=f'b{i+1}',type='org.rad.obs:'+kind,parameters=parameters(**values))
    e.call('add_action',button_id='b6',type='org.rad.obs:scene',parameters=parameters(scene=target('Root')))
    until(lambda:e.call('plugin_status'),lambda v:v['items'][0]['host']=='ready')
    e.call('set_permissions',target='global',value=dict(own=dict(consult=True,modify=True,execute=True)))
    a=Peer(w.root,'automation');w.peers.append(a)
    for op,args in [('credential_replace',dict(plugin_id='org.rad.obs',slot='password',secret='wrong')),
      ('credential_status',dict(plugin_id='org.rad.obs',slot='password')),
      ('plugin_admin',dict(operation='uninstall',plugin_id='org.rad.obs',source=None,trusted=True))]:
        equal('Automation rejects '+op,a.request(op,**args)['error']['code'],'permission_denied')
    e.call('credential_replace',plugin_id='org.rad.obs',slot='password',secret='wrong')
    equal('wrong credentials fail',w.cli('run','a1','--wait',code=6)['data']['steps'][0]['code'],'action_failed')
    equal('wrong password no effects',w.control('snapshot')['effects'],[])
    e.call('credential_replace',plugin_id='org.rad.obs',slot='password',secret='correct')
    scenes=w.cli('options','a1','scene')['data']; equal('scene catalog count',len(scenes['items']),2)
    items=w.cli('options','a3','item')['data']['items']
    actual=[json.loads(x['value']) for x in items]
    equal('duplicate sources and group item targets',[(x['name'],x['item'],x['source']) for x in actual],[('Root',1,'Camera'),('Root',2,'Camera'),('Root',3,'G'),('G',8,'Camera')])
    equal('filter catalog scoped',w.cli('options','a2','filter')['data']['items'],[dict(value='Blur',label='Blur')])
    equal('queries never mutate',w.control('snapshot')['effects'],[])
    for i in range(1,6):equal('action succeeded',w.cli('run',f'a{i}','--wait')['data']['state'],'succeeded')
    effects=w.control('snapshot')['effects']
    equal('exact effects',effects,[dict(request='SetCurrentProgramScene',data=dict(sceneName='Live')),
      dict(request='SetSourceFilterEnabled',data=dict(sourceName='Camera',filterName='Blur',filterEnabled=True)),
      dict(request='SetSceneItemEnabled',data=dict(sceneName='G',sceneItemId=8,sceneItemEnabled=True)),
      dict(request='StartRecord',data={}),dict(request='StartStream',data={})])
    e.call('edit',changes=[dict(kind='set',id='b1',field='visual',value=dict(type='org.rad.obs#scene',parameters=parameters(scene=target('Live'))))])
    current=until(lambda:e.call('plugin_visual',id='b1'),lambda v:v['availability']=='current')
    w.control('external',scene='Root')
    until(lambda:e.call('plugin_visual',id='b1'),lambda v:v['availability']=='current' and v['text']!=current['text'])
    equal('external state not an effect',w.control('snapshot')['effects'],effects)
    w.control('offline')
    until(lambda:e.call('plugin_visual',id='b1'),lambda v:v['availability']=='unavailable')
    w.control('online')
    until(lambda:e.call('plugin_visual',id='b1'),lambda v:v['availability']=='current')
    equal('reconnect never replays prior actions',w.control('snapshot')['effects'],effects)
    w.control('offline')
    until(lambda:e.call('plugin_visual',id='b1'),lambda v:v['availability']=='unavailable')
    # Deliberately hold this controlled outage beyond the ordinary 5s budget.
    # Repeated operations below are observations, never retries of run/effects.
    admitted=e.call('run',id='b6'); started=time.monotonic()
    while time.monotonic()-started<5.25:
        equal('wait remains running during controlled outage',e.call('result',execution_id=admitted['execution_id'])['state'],'running')
        time.sleep(.25)
    equal('offline wait has zero scene effects',w.control('snapshot')['effects'],effects)
    w.control('online')
    until(lambda:e.call('plugin_visual',id='b1'),lambda v:v['availability']=='current')
    until(lambda:w.control('snapshot'),lambda v:len(v['effects'])==6)
    equal('wait execution terminal success',until(lambda:e.call('result',execution_id=admitted['execution_id']),lambda v:v['state']!='running')['state'],'succeeded')
    equal('wait then scene single effect',w.control('snapshot')['effects'],effects+[dict(request='SetCurrentProgramScene',data=dict(sceneName='Root'))])
    for peer in w.peers: peer.close()
    w.peers.clear()
    w.runtime.terminate(); equal('runtime orderly stop',w.runtime.wait(timeout=8),0)
    w.server.stdin.close(); equal('fake orderly stop',w.server.wait(timeout=8),0)
    w.fake();w.start(expected_host='ready');e=w.e
    until(lambda:e.call('plugin_visual',id='b1'),lambda v:v['availability']=='current')
    equal('runtime and fake restart never replay prior actions',w.control('snapshot')['effects'],[])
    equal('persisted credential works after restart',w.cli('options','a1','scene')['data']['availability'],'available')
    print('PASS OBS runtime/Manager/IPC/CLI conformance',flush=True)

if __name__=='__main__':
    root,repo=map(lambda s:Path(s).resolve(),sys.argv[1:]);assert root.parent==Path('/tmp') and root.name.startswith('rad-d4-')
    world=World(root,repo)
    try:scenario(world)
    finally:world.close()
