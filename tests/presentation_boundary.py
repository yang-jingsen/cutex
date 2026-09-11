"""Actual private Cutex Job-principal -> native display proof; fake model only.

Reuse the existing isolated setup/RPC helper, not any old probe body. Run only
inside a fresh user/PID/network namespace with task root writable. Job producer
is a fixture calling the actual dedicated authenticated completion route; this
does not rerun Job subprocess/grant/MCP proofs or claim Job execution.
"""
import ast
from pathlib import Path

tree=ast.parse(Path(__file__).with_name('stock_launch_boundary.py').read_text())
cut=next(i for i,n in enumerate(tree.body) if isinstance(n,ast.ClassDef) and n.name=='Model')
# Use a shorter owned HOME component to satisfy actual AF_UNIX byte limits.
for node in ast.walk(ast.Module(body=tree.body[:cut],type_ignores=[])):
    if isinstance(node,ast.Constant) and node.value=='home':node.value='h'
exec(compile(ast.Module(body=tree.body[:cut],type_ignores=[]),'private-owned-helpers','exec'),globals())
assert socket.if_nameindex()==[(1,'lo')]
with socket.socket() as s:
    try:s.connect(('192.0.2.1',443))
    except OSError as e:assert e.errno==101
    else:raise AssertionError('network isolation absent')
assert len(str(CONF/'runtime/app-server/000000000000/s').encode())<=100
for k in list(env):
    if k.startswith('CUTEX_STOCK_TEST_'):del env[k]
BIN=Path('/mnt/mambo/PersonaProjects/cutex-light-core-r1/artifacts/visible-only-tui-r1/final-bin')
PATCHED=BIN/'codex-app-server';STOCK=BIN/'codex'
SCHEMA=ROOT/'artifacts/presentation-job-r1/schema.json'
CONTROLLER=Path(sys.argv[2]).resolve()
assert CONTROLLER.is_relative_to(ROOT/'target/debug/deps')
mode=sys.argv[3] if len(sys.argv)>3 else 'default'
assert mode in ('default','before','after','generation')
if mode=='default':
    CUTEX=ROOT/'artifacts/presentation-job-r1/default-bin/cutex'
    MCP=ROOT/'artifacts/presentation-job-r1/default-bin/cutex-mcp'
model_tree=ast.parse(Path(__file__).with_name('external_input_boundary.py').read_text())
model_node=next(n for n in model_tree.body if isinstance(n,ast.ClassDef) and n.name=='Model')
exec(compile(ast.Module(body=[model_node],type_ignores=[]),'accepted-fake-responses','exec'),globals())
model=http.server.ThreadingHTTPServer(('127.0.0.1',0),Model)
threading.Thread(target=model.serve_forever,daemon=True).start()
peers=[];controllers=[];terminal=None;durable=None;gate=None

def wait_for(f,timeout=150):
    end=time.monotonic()+timeout
    while True:
        value=f()
        if value:return value
        assert time.monotonic()<end,'bounded observation timed out'
        threading.Event().wait(.05) # state observation, never a persistence barrier
def api(p,path,body=None,token=HUMAN_TOKEN,headers=None):
    assert p in (bp,mp) and path.startswith('/') and not path.startswith('//')
    c=http.client.HTTPConnection('127.0.0.1',p,timeout=240)
    c.request('POST' if body is not None else 'GET',path,None if body is None else json.dumps(body),
        {'Authorization':'Bearer '+token,'Content-Type':'application/json',**(headers or {})})
    r=c.getresponse();raw=r.read();code=r.status;c.close()
    try:return code,json.loads(raw)
    except ValueError:return code,raw.decode()
def native(receipt):
    r=RPC(Path(receipt['binding']['endpoint'].removeprefix('unix://')));peers.append(r)
    init=r.call('initialize',{'clientInfo':{'name':'private-presentation-oracle','version':'1'},'capabilities':{'experimentalApi':True}})
    r.notify('initialized');return r,init
def launch(name,restart=False):
    review=action({'operation':'review_runtime','cutex_session_id':durable,'restart':restart})
    r=action({'operation':'run','action_id':name,'review':review});assert r['stage']=='ready'
    stock_pids.append(r['binding']['pid']);return r
def ledger():return json.loads((CONF/'runtime/management-v2/agent-bus-message-state.json').read_text())
def snapshot(mid):return ledger()['messages'][mid]['snapshot']
def framed(domain,fields):return hashlib.sha256(domain+b''.join(struct.pack('>Q',len(s.encode()))+s.encode() for s in fields)).hexdigest()
def record(ident,body):
    p={'id':ident,'source':{'kind':'service','id':'private-observer'},'title':'Private display','body':body,'format':'plainText','references':[]}
    digest=framed(b'codex:presentation:semantic:v1\0',[durable,thread,ident,'service','private-observer',p['title'],body,'plainText'])
    receipt=framed(b'codex:presentation:receipt:v1\0',[durable,thread,ident,digest])
    return {'version':1,'ownerId':durable,'originThreadId':thread,'presentation':p,'semanticSha256':digest,'receiptId':receipt}
class Control:
    def __init__(self,generation):
        e={**env,'S6_PRIVATE_OWNER':durable,'S6_PRIVATE_GENERATION':str(generation)}
        log=open(RUN/f'controller-{len(controllers)}.log','wb');logs.append(log)
        self.p=subprocess.Popen([str(CONTROLLER),'--ignored','--exact','private_presentation_controller','--nocapture'],env=e,cwd=RUN,
            stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=log,text=True,start_new_session=True)
        children.append(self.p);controllers.append(self);self.q=queue.Queue()
        def read():
            for l in self.p.stdout:self.q.put(l.strip())
            self.q.put('EOF')
        threading.Thread(target=read,daemon=True).start();self.initial=self.read()
    def read(self):
        while True:
            s=self.q.get(timeout=90)
            if s=='P_READY':return 'ready'
            if s.startswith('P_REPLY '):return json.loads(s[8:])
            assert s!='EOF','controller unexpectedly exited'
    def call(self,op,r):
        self.p.stdin.write(json.dumps({'operation':op,'record':r})+'\n');self.p.stdin.flush();return self.read()
class Terminal:
    def __init__(self):
        self.master,self.slave=os.openpty();self.original=termios.tcgetattr(self.slave)
        fcntl.ioctl(self.slave,termios.TIOCSWINSZ,struct.pack('HHHH',36,140,0,0));self.screen=b''
        self.p=subprocess.Popen([str(CUTEX),'session','stock-attach',durable],env=env,cwd=RUN,stdin=self.slave,stdout=self.slave,stderr=self.slave,
            start_new_session=True,preexec_fn=lambda:fcntl.ioctl(0,termios.TIOCSCTTY,0));children.append(self.p)
        def read():
            with (RUN/'terminal.pty').open('wb') as out:
                while self.p.poll() is None:
                    if select.select([self.master],[],[],.1)[0]:
                        try:data=os.read(self.master,65536)
                        except OSError:break
                        self.screen+=data;out.write(data);out.flush()
                        if b'\x1b[6n' in data:os.write(self.master,b'\x1b[1;1R')
                        if b'\x1b[c' in data:os.write(self.master,b'\x1b[?1;2c')
        self.reader=threading.Thread(target=read,daemon=True);self.reader.start()
    def wait(self,text):
        wait_for(lambda:text.encode() in re.sub(rb'\x1b\[[0-9;?<>=]*[ -/]*[@-~]',b'',self.screen),60)
    def close(self):
        # Separate bracketed paste from Enter: a burst containing both may be
        # classified as pasted text by the native composer, not a command.
        os.write(self.master,b'\x1b[200~/exit\x1b[201~');self.wait('/exit')
        os.write(self.master,b'\r');assert self.p.wait(timeout=20)==0
        self.reader.join(timeout=2);assert termios.tcgetattr(self.slave)==self.original
        os.close(self.master);os.close(self.slave)

try:
    bp=port();env['S4_TEST_ALLOWED_PORTS']=f'{bp},{model.server_port}'
    config={'agent_bus_enabled':True,'agent_bus_port':bp,'agent_bus_token':BUS_TOKEN,'management_api_token':HUMAN_TOKEN,'default_profile':'alpha'}
    (CONF/'config.json').write_text(json.dumps(config))
    profile=str(uuid.uuid4());folder=CONF/'profiles'/profile;folder.mkdir(parents=True)
    provider={'name':'s6-private','base_url':f'http://127.0.0.1:{model.server_port}/v1','wire_api':'responses','requires_openai_auth':False,'supports_websockets':False}
    (folder/'config.toml').write_text('model="unknown-private-model"\nmodel_provider="s6-private"\n[model_providers.s6-private]\n'+'\n'.join(k+'='+json.dumps(v) for k,v in provider.items())+'\n')
    (CONF/'accounts.json').write_text(json.dumps({'version':3,'accounts':[{'id':profile,'name':'alpha','email':None,'plan_type':None,'last_used_at':None}],'active_account_id':None}))
    (NATIVE/'config.toml').write_text(f'[projects.{json.dumps(str(RUN))}]\ntrust_level="trusted"\n[analytics]\nenabled=false\n')
    sock=RUN/'bootstrap.sock'
    args=[PATCHED,'-c','default_permissions=":read-only"','-c','model="unknown-private-model"','-c','model_provider="s6-private"','-c','model_providers.s6-private={'+','.join(k+'='+json.dumps(v) for k,v in provider.items())+'}', '--disable-plugin-startup-tasks-for-tests','--listen','unix://'+str(sock)]
    bootstrap=owner(args,'bootstrap');wait_for(lambda:sock.exists(),20)
    r=RPC(sock);peers.append(r);r.call('initialize',{'clientInfo':{'name':'neutral','version':'1'},'capabilities':{'experimentalApi':True}});r.notify('initialized')
    thread=r.call('thread/start',{'cwd':str(RUN),'sandbox':'read-only','approvalPolicy':'on-request','ephemeral':False,'historyMode':'paginated'})['thread']['id']
    # Accepted 3d8 README: this exact loaded paginated read materializes the
    # writer and publishes metadata before success; metadata-only read does not.
    persisted=r.call('thread/read',{'threadId':thread,'includeTurns':True})['thread']
    assert persisted['id']==thread and persisted['turns']==[]
    stop_owned(bootstrap);assert Model.calls==0
    cli('session','adopt',thread,'--name','Private Presentation Agent','--cwd',RUN)
    durable=next(x['cutex_session_id'] for x in store()['sessions'].values() if x.get('codex_session_id')==thread)
    cli('session','defaults','set',durable,'--runtime-backend','host','--permission','read-only','--sandbox','read-only','--approval-policy','on-request')
    config['private_job_presentation']={'version':1,'recipients':[durable]};(CONF/'config.json').write_text(json.dumps(config))
    bus=owner([CUTEX,'agent','serve','--port',bp],'bus');ready(bp,bus)
    mp=port();env['S4_TEST_ALLOWED_PORTS']+=f',{mp}'
    if mode!='default':
        gate=socket.socket(socket.AF_UNIX);gatepath=HOME/'display-gate';gate.bind(str(gatepath));gate.listen();gate.settimeout(150)
        env.update(CUTEX_NATIVE_DELIVERY_TEST_GATE=str(gatepath),CUTEX_NATIVE_DELIVERY_TEST_MESSAGE='private-event',
            CUTEX_NATIVE_DELIVERY_TEST_STAGE='before_presentation_append' if mode=='before' else 'after_presentation_append')
    management=owner([CUTEX,'management','serve','--port',mp],'management');ready(mp,management)
    bundle={'version':3,'upstream_commit':'3d2ee51ca2d5db578f328aa75e20aa22c0197c9a','native_patch_commit':'3d8a73a747cf5b957a7ca0491c28d1517f6d7722',
        'executable':verified(PATCHED),'cli':verified(STOCK),'code_mode_host':verified(BIN/'codex-code-mode-host'),'facade':verified(MCP),'schema':verified(SCHEMA),'shared_config':verified(NATIVE/'config.toml')}
    manifest=RUN/'bundle.json';manifest.write_text(json.dumps(bundle));contract={'version':2,'native_id':thread,'native_home':str(NATIVE),'bundle_manifest':str(manifest),'bundle_sha256':sha(manifest)}
    review=action({'operation':'review','cutex_session_id':durable,'contract':contract})
    assert api(mp,'/v2/agent-management/explicit-launch',{'operation':'activate','action_id':'deny','review':review},token=BUS_TOKEN)[0]==401
    activation={'operation':'activate','action_id':'presentation-activate','review':review};activated=action(activation);assert action(activation)==activated
    current=launch('presentation-launch');r,init=native(current);assert init['presentationVersion']==1
    control=Control(1);assert control.initial=='ready';assert 'error' in Control(2).initial
    standalone=record('private-idle','PRIVATE_VISIBLE_SENTINEL only display')
    assert control.call('status',standalone)=={'result':None}
    assert control.call('append',standalone)=={'result':standalone}
    assert control.call('append',standalone)=={'result':standalone}
    assert Model.calls==0 and r.call('thread/read',{'threadId':thread,'includeTurns':True})['thread']['turns']==[]
    if mode=='default':terminal=Terminal();terminal.wait('PRIVATE_VISIBLE_SENTINEL')
    request={'schema':'cutex.job_service.completion.v1','eventId':'private-event','jobId':'private-job','jobRevision':1,'terminalStatus':'exited','resultSha256':'a'*64,'targetCutexSessionId':durable,'summary':'SERVICE_DISPLAY_BODY'}
    token=(CONF/'runtime/task-service/job-service-completion.token').read_text().strip()
    assert api(bp,'/api/job-service/v1/completions',request,token=BUS_TOKEN)[0]==401
    code,response=api(bp,'/api/job-service/v1/completions',request,token=token);assert code==200 and response['status']=='committed',response
    mid=response['messageId']
    if mode!='default':
        connection,_=gate.accept();assert connection.makefile('rb').readline().decode().strip()==mid
        before=snapshot(mid);assert before['state']=='delivered' and before.get('externalInputReceipt') and not before['presentation']['receipt']
        code,job_state=api(bp,'/api/job-service/v1/completions/query',{'schema':request['schema'],'eventId':request['eventId']},token=token)
        assert code==200 and job_state['disposition']=='delivered'
        original=before['presentation']['frozen'];assert original
        (RUN/'cutpoint.json').write_text(json.dumps({'mode':mode,'inputState':before['state'],
            'jobQuery':job_state,'inputReceipt':before['externalInputReceipt'],
            'frozenPresentation':original,'localDisplayReceipt':before['presentation']['receipt']},indent=2))
        for key in list(env):
            if key.startswith('CUTEX_NATIVE_DELIVERY_TEST_'):del env[key]
        if mode=='generation':
            results=queue.Queue()
            def restart():
                try:results.put(launch('generation-fence',True))
                except BaseException as e:results.put(e)
            threading.Thread(target=restart,daemon=True).start()
            wait_for(lambda: 'generation-fence' in store().get('explicit_launch_receipts',{}),180)
            connection.sendall(b'continue\n');connection.close()
            current=results.get(timeout=240);assert isinstance(current,dict),str(current)
        else:
            stop_owned(management);connection.close()
            management=owner([CUTEX,'management','serve','--port',mp],'management-recovered');ready(mp,management)
            current=launch('presentation-recovery',True)
        gate.close();gate=None
    completed=wait_for(lambda:snapshot(mid) if snapshot(mid)['presentation'].get('receipt') else None)
    assert completed['state']=='delivered'
    if mode!='default':assert completed['presentation']['receipt']==original and completed['presentation']['commitGeneration']==2
    receipt=completed['presentation']['receipt'];assert receipt['presentation']['references']==[{'kind':'externalInput','id':mid}]
    assert receipt['presentation']['body']!=completed['externalInput']['message']['text']
    code,replay=api(bp,'/api/job-service/v1/completions',request,token=token);assert code==200 and replay['deduplicated']
    r,init=native(current);timeline=r.call('thread/timeline/list',{'threadId':thread,'limit':100})
    displays=[x for x in timeline['data'] if x['type']=='presentation'];assert len(displays)==2
    if terminal:terminal.wait('输出读取状态');terminal.close();terminal=None
    if mode=='default':
        # Default-off changes apply to NEW events, not this frozen one.
        del config['private_job_presentation'];(CONF/'config.json').write_text(json.dumps(config))
        second={**request,'eventId':'default-off'};code,off=api(bp,'/api/job-service/v1/completions',second,token=token);assert code==200
        assert 'presentation' not in snapshot(off['messageId'])
    assert all('PRIVATE_VISIBLE_SENTINEL' not in json.dumps(x) and '输出读取状态' not in json.dumps(x,ensure_ascii=False) for x in Model.requests)
    assert len(ledger()['messages'])==(2 if mode=='default' else 1)
    (RUN/'PASS.json').write_text(json.dumps({'mode':mode,'durable':durable,'native':thread,'generation':current['expected_generation'],'jobMessageId':mid,
        'displayReceipt':receipt,'inputReceipt':completed['externalInputReceipt'],'standalone':standalone,'modelRequests':Model.calls,
        'cutex':sha(CUTEX),'facade':sha(MCP),'nativeManifest':'28cbc9a64460db504fe842b787695144e14a90b7f939b7782dd0133d6e45d54c',
        'producer':'authenticated Job-completion fixture, not new Job process/MCP proof','pty':mode=='default','network':'private namespace + owned port connect tripwire'},ensure_ascii=False,indent=2))
finally:
    (RUN/'model-requests.json').write_text(json.dumps(Model.requests,ensure_ascii=False))
    if terminal and terminal.p.poll() is None:stop_owned(terminal.p)
    if gate:gate.close()
    if durable:
        runtime=store()['sessions'].get(durable,{}).get('app_server_runtime')
        if runtime:stock_pids.append(runtime['pid'])
    for pid in set(stock_pids):
        try:
            if os.getpgid(pid)==pid and Path(f'/proc/{pid}/exe').readlink()==PATCHED and Path(f'/proc/{pid}/cwd').readlink()==RUN:os.killpg(pid,signal.SIGKILL)
        except (ProcessLookupError,FileNotFoundError):pass
    for p in reversed(children):stop_owned(p)
    for log in logs:log.close()
    model.shutdown()
