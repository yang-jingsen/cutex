"""S4 private real provider/native lifecycle oracle; no live homes or paid model.

Run with one NEW evidence directory argument. Failures are retained. S2's exact
WebSocket adapter is extracted as a class, never executing its probe body.
"""
import ast, base64, fcntl, hashlib, http.client, http.server, json, os, queue, re, select, termios
import signal, socket, struct, subprocess, sys, threading, time, uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
RUN = ROOT / sys.argv[1]
assert RUN.parent == ROOT and not RUN.exists()
assert os.statvfs(ROOT).f_bavail * os.statvfs(ROOT).f_frsize > 100 * 1024**3
RUN.mkdir(mode=0o700)
HOME = RUN / "home"; HOME.mkdir()
(HOME / ".cutex-test-private-home").touch()
CONF = HOME / ".cutex"; CONF.mkdir()
NATIVE = CONF / "codex-home"; NATIVE.mkdir()
STOCK = Path('/mnt/mambo/PersonaProjects/cutex-upstream-lightweight-r1/stock/bin/codex')
CUTEX = ROOT / 'target/debug/cutex'
MCP = ROOT / 'target/debug/cutex-mcp'
env = {'PATH':'/usr/local/bin:/usr/bin:/bin','HOME':str(HOME),
       'CODEX_HOME':str(NATIVE),'CUTEX_TEST_PRIVATE_HOME':str(HOME),
       'TMPDIR':str(ROOT/'tmp'),'TERM':'xterm-256color','LANG':'C.UTF-8'}
guard=RUN/'connect-guard.so'
subprocess.run(['/usr/bin/cc','-shared','-fPIC','-Wall','-Werror',
                str(Path(__file__).with_name('stock_probe_connect_guard.c')),'-ldl','-o',str(guard)],
               env=env,check=True,capture_output=True)
env.update(LD_PRELOAD=str(guard),S4_TEST_ALLOWED_PORTS='')
env['CUTEX_STOCK_TEST_LOST_READY_ACTION']='s4-launch-alpha'
env['CUTEX_STOCK_TEST_REGISTER_DENY_ACTION']='s4-launch-alpha'
env['CUTEX_STOCK_TEST_COMMIT_FAIL_ACTION']='s4-commit-failure'
repair_mode=sys.argv[2] if len(sys.argv)>2 and sys.argv[2] in ('default-publication','commit-recovery','creator-recovery','published-recovery') else None
if repair_mode:
    for key in list(env):
        if key.startswith('CUTEX_STOCK_TEST_'): del env[key]
    key={'commit-recovery':'CUTEX_STOCK_TEST_COMMIT_FAIL_ACTION','creator-recovery':'CUTEX_STOCK_TEST_CREATOR_DEATH_ACTION','published-recovery':'CUTEX_STOCK_TEST_PUBLISHED_DEATH_ACTION'}.get(repair_mode)
    if key: env[key]='s4-launch-alpha'
# Deterministic refusal proof: no listener or live endpoint is contacted.
tripwire=subprocess.run(['/usr/bin/python3','-c',"import socket; socket.socket().connect(('127.0.0.1',1))"],
                       env=env,capture_output=True)
assert tripwire.returncode==97 and b'S4_PROBE_UNOWNED_ENDPOINT_BLOCKED' in tripwire.stderr
BUS_TOKEN = 's4-private-bus-fixture-only'
HUMAN_TOKEN = 's4-private-human-fixture-only'
children=[]; logs=[]; stock_pids=[]
source = ast.parse((Path(__file__).with_name('stock_mcp_boundary.py')).read_text())
rpc_class = next(n for n in source.body if isinstance(n, ast.ClassDef) and n.name=='RPC')
exec(compile(ast.Module(body=[rpc_class],type_ignores=[]),'S2-RPC-adapter','exec'))

def port():
    for p in range(24800,24999):
        with socket.socket() as s:
            try: s.bind(('127.0.0.1',p)); return p
            except OSError: pass
    raise AssertionError('no private port available')
def owner(args,name):
    log=open(RUN/(name+'.log'),'wb'); logs.append(log)
    p=subprocess.Popen([str(a) for a in args],env=env,cwd=RUN,
                       stdout=log,stderr=log,start_new_session=True)
    children.append(p); return p
def cli(*args):
    p=subprocess.run([str(CUTEX),*map(str,args)],env=env,cwd=RUN,
                     capture_output=True,timeout=30)
    assert p.returncode==0,(args,p.stdout.decode(),p.stderr.decode())
    return p.stdout.decode()
def ready(p,child):
    until=time.monotonic()+20
    while time.monotonic()<until:
        assert child.poll() is None,'owned service exited'
        try:
            with socket.create_connection(('127.0.0.1',p),.1): return
        except OSError: threading.Event().wait(.02)
    raise AssertionError('private service did not listen')
def api(p,path,body=None,token=HUMAN_TOKEN,headers=None):
    c=http.client.HTTPConnection('127.0.0.1',p,timeout=60)
    c.request('POST' if body is not None else 'GET',path,
              None if body is None else json.dumps(body),
              {'Authorization':'Bearer '+token,'Content-Type':'application/json',**(headers or {})})
    r=c.getresponse(); status=r.status; raw=r.read(); c.close()
    try: value=json.loads(raw)
    except ValueError: value=raw.decode()
    return status,value
def action(body,ok=True):
    status,value=api(mp,'/v2/agent-management/explicit-launch',body)
    if ok: assert status==200,(status,value)
    else: assert status!=200,(status,value)
    return value
def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()
def verified(path): return {'path':str(path),'sha256':sha(path)}
def store(): return json.loads((CONF/'cutex-sessions.json').read_text())
def stop_owned(p):
    if p.poll() is None: os.killpg(p.pid,signal.SIGKILL)
    p.wait(timeout=10)
def process_identity(pid):
    try:
        fields=Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()
        return fields[19],fields[0]
    except FileNotFoundError: return None
def owned_tree(pid):
    result={}; pending=[pid]
    while pending:
        child=pending.pop(); identity=process_identity(child)
        if identity is None: continue
        result[child]=identity[0]
        try: pending += [int(p) for p in Path(f'/proc/{child}/task/{child}/children').read_text().split()]
        except FileNotFoundError: pass
    return result
class Model(http.server.BaseHTTPRequestHandler):
    calls=0
    mode='history'
    mcp_step=0
    approval_step=0
    sandbox_step=0
    sandbox_file='sandbox-must-not-exist'
    outputs=[]
    def log_message(self,*args): pass
    def do_POST(self):
        assert self.path=='/v1/responses'
        data=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        Model.calls+=1; assert Model.calls<=12
        item={'type':'message','role':'assistant','id':f's4-fixture-{Model.calls}','content':[{'type':'output_text','text':'S4 preserved native history'}]}
        if Model.mode=='approval':
            n=Model.approval_step; Model.approval_step+=1
            if n==0:
                item={'type':'function_call','call_id':'s4-approval','namespace':'functions','name':'exec_command','arguments':json.dumps({'cmd':f'printf s4-approval-proof > {RUN / "approval-must-not-exist"}','sandbox_permissions':'require_escalated','justification':'S4 private approval proof: decline this request','max_output_tokens':1000})}
            else: item['content'][0]['text']='S4 approval declined'
        if Model.mode=='sandbox':
            Model.outputs += [i for i in data.get('input',[]) if i.get('type')=='function_call_output']
            n=Model.sandbox_step; Model.sandbox_step+=1
            if n==0:
                script="import socket; from pathlib import Path\nfor label,operation in [('file',lambda:Path("+repr(Model.sandbox_file)+").write_text('probe')),('network',lambda:socket.create_connection(('127.0.0.1',"+str(model.server_port)+"),1))]:\n try: operation(); print(label+':ALLOWED')\n except OSError as e: print(label+':DENIED:'+str(e.errno))"
                import shlex
                item={'type':'function_call','call_id':'s4-'+Model.sandbox_file,'namespace':'functions','name':'exec_command','arguments':json.dumps({'cmd':'/usr/bin/python3 -c '+shlex.quote(script),'max_output_tokens':1000})}
            else: item['content'][0]['text']='S4 sandbox observation complete'
        if Model.mode=='mcp':
            Model.outputs += [i for i in data.get('input',[]) if i.get('type')=='function_call_output']
            n=Model.mcp_step; Model.mcp_step+=1
            if n==0: item={'type':'tool_search_call','call_id':'s4-search','execution':'client','arguments':{'query':'Cutex query_managed send','limit':2}}
            elif n in (1,2):
                name='query_managed' if n==1 else 'send'
                args={'action_id':'s4-mcp-query','project_id':'s4-project'} if n==1 else {'to':target,'message':'Private S4 outbound proof','external_message_id':'s4-outbound','delivery_mode':'passive'}
                item={'type':'function_call','call_id':f's4-mcp-{n}','namespace':'mcp__cutex','name':name,'arguments':json.dumps(args)}
            else: item['content'][0]['text']='S4 outbound complete'
        events=[{'type':'response.created','response':{'id':f's4-response-{Model.calls}'}},
                {'type':'response.output_item.done','item':item},
                {'type':'response.completed','response':{'id':f's4-response-{Model.calls}','usage':{'input_tokens':0,'output_tokens':0,'total_tokens':0}}}]
        body=''.join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events).encode()
        self.send_response(200); self.send_header('Content-Type','text/event-stream')
        self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)

model=http.server.ThreadingHTTPServer(('127.0.0.1',0),Model)
threading.Thread(target=model.serve_forever,daemon=True).start()
try:
    bp=port()
    env['S4_TEST_ALLOWED_PORTS']=f'{bp},{model.server_port}'
    config={'agent_bus_enabled':True,'agent_bus_port':bp,'agent_bus_token':BUS_TOKEN,
            'management_api_token':HUMAN_TOKEN,'default_profile':'alpha'}
    (CONF/'config.json').write_text(json.dumps(config))
    accounts=[]
    provider={'name':'s4-fake','base_url':f'http://127.0.0.1:{model.server_port}/v1',
              'wire_api':'responses','requires_openai_auth':False,'supports_websockets':False}
    for name,effort in [('alpha','low'),('beta','high')]:
        ident=str(uuid.uuid4()); folder=CONF/'profiles'/ident; folder.mkdir(parents=True)
        accounts.append({'id':ident,'name':name,'email':None,'plan_type':None,'last_used_at':None})
        raw=f'model="gpt-5.4"\nmodel_provider="s4-fake"\nmodel_reasoning_effort="{effort}"\n[model_providers.s4-fake]\n'
        raw+='\n'.join(k+'='+json.dumps(v) for k,v in provider.items())+'\n'
        (folder/'config.toml').write_text(raw)
    (CONF/'accounts.json').write_text(json.dumps({'version':3,'accounts':accounts,'active_account_id':None}))
    # Explicit dummy-fixture trust and model-notice acknowledgement, matching
    # accepted S2 startup; launch itself must never write either preference.
    shared=f'[projects.{json.dumps(str(RUN))}]\ntrust_level="trusted"\n[analytics]\nenabled=false\n[notice.model_migrations]\n"gpt-5.4"="gpt-5.6-terra"\n'
    (NATIVE/'config.toml').write_text(shared)
    shared_sha=sha(NATIVE/'config.toml')
    sock=RUN/'bootstrap.sock'
    args=[STOCK,'-c','model="gpt-5.4"','-c','model_provider="s4-fake"']
    args += ['-c','model_providers.s4-fake={'+','.join(k+'='+json.dumps(v) for k,v in provider.items())+'}']
    bootstrap=owner([*args,'app-server','--listen','unix://'+str(sock)],'bootstrap')
    until=time.monotonic()+15
    while not sock.exists():
        assert bootstrap.poll() is None and time.monotonic()<until
        threading.Event().wait(.02)
    rpc=RPC(sock); rpc.call('initialize',{'clientInfo':{'name':'s4-bootstrap','version':'1'},'capabilities':{'experimentalApi':True}}); rpc.notify('initialized')
    thread=rpc.call('thread/start',{'cwd':str(RUN),'sandbox':'read-only','approvalPolicy':'on-request','ephemeral':False})['thread']['id']
    rpc.call('turn/start',{'threadId':thread,'input':[{'type':'text','text':'Preserve this original private history'}]})
    until=time.monotonic()+30
    while not any(e.get('method')=='turn/completed' for e in rpc.events):
        assert time.monotonic()<until
        rpc.events.append(rpc.messages.get(timeout=10))
    # thread/read is the persisted-history oracle, not a timing barrier.
    before=rpc.call('thread/read',{'threadId':thread,'includeTurns':True})
    assert before['thread']['turns']
    stop_owned(bootstrap)
    cli('session','adopt',thread,'--name','S4 Formal Agent','--cwd',RUN)
    durable=next(r['cutex_session_id'] for r in store()['sessions'].values() if r.get('codex_session_id')==thread)
    cli('session','defaults','set',durable,'--runtime-backend','host','--permission','read-only','--sandbox','read-only','--approval-policy','on-request')
    bus=owner([CUTEX,'agent','serve','--port',bp],'bus'); ready(bp,bus)
    mp=port()
    env['S4_TEST_ALLOWED_PORTS']+=f',{mp}'
    management=owner([CUTEX,'management','serve','--port',mp],'management'); ready(mp,management)
    bundle={'version':1,'upstream_commit':'3d2ee51ca2d5db578f328aa75e20aa22c0197c9a',
            'executable':verified(STOCK),'code_mode_host':verified(STOCK.with_name('codex-code-mode-host')),
            'facade':verified(MCP),'schema':verified(ROOT/'s4-schema/codex_app_server_protocol.schemas.json'),
            'shared_config':verified(NATIVE/'config.toml')}
    manifest=RUN/'bundle.json'; manifest.write_text(json.dumps(bundle))
    contract={'version':1,'native_id':thread,'native_home':str(NATIVE),'bundle_manifest':str(manifest),'bundle_sha256':sha(manifest)}
    request={'operation':'review','cutex_session_id':durable,'contract':contract}
    assert api(mp,'/v2/agent-management/explicit-launch',request,token=BUS_TOKEN)[0]==401
    review=action(request)
    for label,change in [('version',{'version':2}),('foreign-native',{'native_id':str(uuid.uuid4())}),('wrong-home',{'native_home':str(RUN)})]:
        action({**request,'contract':{**contract,**change}},ok=False)
    activation={'operation':'activate','action_id':'s4-activate','review':review}
    receipt=action(activation); assert action(activation)==receipt
    changed=json.loads(json.dumps(activation)); changed['review']['subject']['formal_name']='wrong'
    action(changed,ok=False)
    first=action({'operation':'review_runtime','cutex_session_id':durable,'restart':False})
    launch={'operation':'run','action_id':'s4-launch-alpha','review':first}
    if repair_mode:
        unrelated=owner(['/usr/bin/python3','-c','import signal; signal.pause()'],'unrelated')
        if repair_mode=='commit-recovery':
            failure=action(launch,ok=False)
            pid=int(re.search(r'owned child PID (\d+)',json.dumps(failure)).group(1))
            assert process_identity(pid) is None
        elif repair_mode in ('creator-recovery','published-recovery'):
            try: action(launch)
            except (http.client.RemoteDisconnected, ConnectionResetError): pass
            else: raise AssertionError('creator did not die at selected boundary')
            assert management.wait(timeout=10)==(86 if repair_mode=='creator-recovery' else 87)
        if repair_mode!='default-publication':
            prior=store()['explicit_launch_receipts']['s4-launch-alpha']['receipt']
            assert prior['stage']==('spawned' if repair_mode=='published-recovery' else 'claimed')
            publication=prior['publication']
            # Kernel lock acquisition observes gate EOF/child exit, never a sleep barrier.
            with open(publication['path'],'r+') as witness:
                fcntl.flock(witness,fcntl.LOCK_EX)
                if repair_mode=='commit-recovery':
                    denied=action(launch,ok=False)
                    assert 'publication_busy' in json.dumps(denied)
                fcntl.flock(witness,fcntl.LOCK_UN)
            if repair_mode=='commit-recovery':
                path=Path(publication['path']); retained=path.with_suffix('.retained')
                path.rename(retained)
                try:
                    assert 'publication evidence unavailable' in json.dumps(action(launch,ok=False))
                    path.touch(mode=0o600)
                    assert 'publication evidence replaced' in json.dumps(action(launch,ok=False))
                finally: retained.replace(path)
                assert store()['explicit_launch_receipts']['s4-launch-alpha']['receipt']==prior
            if repair_mode in ('creator-recovery','published-recovery'):
                env.pop(key)
                management=owner([CUTEX,'management','serve','--port',mp],'management-recovered'); ready(mp,management)
        recovered=action(launch)
        assert recovered['stage']=='ready' and action(launch)==recovered
        if repair_mode!='default-publication':
            assert recovered['claim_id']==prior['claim_id'] and recovered['runtime_agent_id']==prior['runtime_agent_id'] and recovered['publication']==publication
        stock_pids.append(recovered['binding']['pid'])
        observer=RPC(Path(recovered['binding']['endpoint'].removeprefix('unix://')))
        observer.call('initialize',{'clientInfo':{'name':'s4-recovery-observer','version':'1'},'capabilities':{'experimentalApi':True}}); observer.notify('initialized')
        assert observer.call('server/diagnostics',{})['process']['id']==recovered['binding']['pid']
        assert observer.call('thread/read',{'threadId':thread,'includeTurns':True})['thread']['turns']==before['thread']['turns']
        record=store()['sessions'][durable]
        assert record['explicit_launch']==contract and record['codex_session_id']==thread and record['runtime_generation']==1 and record.get('app_server_launch_claim_id') is None
        assert unrelated.poll() is None and sha(NATIVE/'config.toml')==shared_sha
        (RUN/'PASS.json').write_text(json.dumps({'mode':repair_mode,'same_claim':recovered['claim_id'],'durable':durable,'native':thread,'stage':recovered['stage'],'generation':record['runtime_generation'],'model_calls':Model.calls,'unrelated_alive':True},indent=2))
        sys.exit(0)
    ready_receipt=action(launch)
    (RUN/'first-receipt.json').write_text(json.dumps(ready_receipt,indent=2))
    if ready_receipt.get('binding'): stock_pids.append(ready_receipt['binding']['pid'])
    assert ready_receipt['stage']=='spawned' and 'Unauthorized Agent Bus request' in ready_receipt['error'],ready_receipt
    denied_binding=ready_receipt['binding']
    ready_receipt=action(launch)
    assert ready_receipt['stage']=='spawned' and 'lost readiness' in ready_receipt['error'],ready_receipt
    assert ready_receipt['binding']==denied_binding
    pending=ready_receipt
    # Registered is not Ready: real provider rejects trusted MCP binding until
    # the original action commits its exact owner readback.
    hs={'X-Cutex-Agent-Id':pending['runtime_agent_id'],'X-Cutex-Mcp-Thread-Id':thread,'X-Cutex-Mcp-Generation':str(pending['expected_generation'])}
    status,denied=api(bp,'/api/agent-management/v1/actions',{'schema':'cutex/agent-management/v1','action_id':'s4-not-ready','operation':'query_managed','project_id':'s4-project'},token=BUS_TOKEN,headers=hs)
    assert 'unauthorized' in json.dumps(denied).lower(),(status,denied)
    action({'operation':'review_runtime','cutex_session_id':durable,'restart':False},ok=False)
    ready_receipt=action(launch)
    assert ready_receipt['binding']==pending['binding']
    assert ready_receipt['stage']=='ready',ready_receipt
    assert action(launch)==ready_receipt
    record=store()['sessions'][durable]
    assert record['runtime_generation']==ready_receipt['expected_generation']
    assert record['codex_session_id']==thread and record['explicit_launch']==contract
    assert record.get('app_server_launch_claim_id') is None
    assert sha(NATIVE/'config.toml')==shared_sha
    def read_owner(receipt):
        live=RPC(Path(receipt['binding']['endpoint'].removeprefix('unix://')))
        live.call('initialize',{'clientInfo':{'name':'s4-observer','version':'1'},'capabilities':{'experimentalApi':True}}); live.notify('initialized')
        assert live.call('server/diagnostics',{})['process']['id']==receipt['binding']['pid']
        history=live.call('thread/read',{'threadId':thread,'includeTurns':True})
        assert history['thread']['turns'][:len(before['thread']['turns'])]==before['thread']['turns']
        return live
    read_owner(ready_receipt)
    unrelated=owner(['/usr/bin/python3','-c','import signal; signal.pause()'],'unrelated')
    read_owner(ready_receipt).call('mcpServerStatus/list',{'threadId':thread,'detail':'toolsAndAuthOnly'})
    old_tree=owned_tree(ready_receipt['binding']['pid'])
    assert old_tree and unrelated.poll() is None
    # Generic launch is rejected without replacing or stopping this owner.
    generic={'requestId':'s4-generic-online','method':'cutex/runtime/online','params':{'expectedRuntimeGeneration':record['runtime_generation'],'openVisibleTerminal':False}}
    status,denied=api(mp,f'/v2/sessions/{durable}/cutex/requests',generic)
    # The legacy owner-visible v2 catalog excludes this truthful native source;
    # retain that limitation rather than fabricating a top-level source label.
    assert status==404 and denied['error']['code']=='session_not_found',(status,denied)
    rejected=subprocess.run([str(CUTEX),'session','online',durable],env=env,cwd=RUN,capture_output=True,timeout=20)
    assert rejected.returncode!=0 and b'explicit_stock_launch_required' in rejected.stderr,(rejected.stdout,rejected.stderr)
    read_owner(ready_receipt)
    pre_profile=action({'operation':'review_runtime','cutex_session_id':durable,'restart':True})
    cli('session','profile','set',durable,'beta')
    action({'operation':'run','action_id':'s4-stale-profile','review':pre_profile},ok=False)
    read_owner(ready_receipt)
    second=action({'operation':'review_runtime','cutex_session_id':durable,'restart':True})
    assert second['configuration']['profile_name']=='beta'
    assert second['configuration']['reasoning']=='high' and first['configuration']['reasoning']=='low'
    second_launch={'operation':'run','action_id':'s4-restart-beta','review':second}
    from concurrent.futures import ThreadPoolExecutor
    with ThreadPoolExecutor(max_workers=2) as concurrent:
        concurrent_results=list(concurrent.map(action,[second_launch,second_launch]))
    assert concurrent_results[0]==concurrent_results[1]
    second_receipt=concurrent_results[0]
    (RUN/'second-receipt.json').write_text(json.dumps(second_receipt,indent=2))
    if second_receipt.get('binding'): stock_pids.append(second_receipt['binding']['pid'])
    assert second_receipt['stage']=='ready',second_receipt
    assert action(second_launch)==second_receipt
    assert second_receipt['expected_generation']==ready_receipt['expected_generation']+1
    assert second_receipt['binding']['pid']!=ready_receipt['binding']['pid']
    assert unrelated.poll() is None
    for pid,start in old_tree.items():
        current=process_identity(pid)
        assert current is None or current[0]!=start or current[1]=='Z',('old owned process still executing',pid)
    live=read_owner(second_receipt)
    assert sha(NATIVE/'config.toml')==shared_sha
    assert json.loads((CONF/'config.json').read_text())['default_profile']=='alpha'
    assert store()['sessions'][durable]['explicit_launch']==contract
    # Real stock CLI attaches to that exact server; no second app-server writer.
    live.call('mcpServerStatus/list',{'threadId':thread,'detail':'toolsAndAuthOnly'})
    account=live.call('account/read',{})
    (RUN/'account-flags.json').write_text(json.dumps({'requiresOpenaiAuth':account.get('requiresOpenaiAuth'),'accountPresent':account.get('account') is not None}))
    resumed=live.call('thread/resume',{'threadId':thread,'excludeTurns':True})
    assert resumed['thread']['id']==thread
    master,slave=os.openpty(); fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',30,120,0,0))
    original=termios.tcgetattr(slave)
    Model.mode='approval'
    attach_args=[str(CUTEX),'session','stock-attach',durable]
    skip_pty=len(sys.argv)>2 and sys.argv[2]=='skip-pty'
    if skip_pty: attach_args=['/usr/bin/true']
    if len(sys.argv)>2 and sys.argv[2]=='trace-direct':
        from stock_rpc_trace import relay
        evidence=open(RUN/'attach-rpc.jsonl','w'); logs.append(evidence)
        proxy=RUN/'trace.sock'
        listener=relay(proxy,Path(second_receipt['binding']['endpoint'].removeprefix('unix://')),evidence)
        # Diagnostic stock CLI, explicitly NOT the production adapter oracle.
        attach_args=[str(x) for x in [STOCK,'resume','--remote','unix://'+str(proxy),thread,'--no-alt-screen','--cd',RUN,*args[1:],'-c','tui.resume_cwd="current"']]
    terminal=subprocess.Popen(attach_args,env=env,cwd=RUN,
        stdin=slave,stdout=slave,stderr=slave,start_new_session=True,
        preexec_fn=lambda:fcntl.ioctl(0,termios.TIOCSCTTY,0))
    children.append(terminal)
    screen=b''; exited=False; prompted=False; submitted=False; declined=False; approval_complete=False; until=time.monotonic()+40
    while time.monotonic()<until and terminal.poll() is None:
        if select.select([master],[],[],.1)[0]:
            chunk=os.read(master,65536); screen+=chunk
            if b'\x1b[6n' in chunk: os.write(master,b'\x1b[1;1R')
            if b'\x1b[c' in chunk: os.write(master,b'\x1b[?1;2c')
        plain=re.sub(rb'\x1b\[[0-9;?<>=]*[ -/]*[@-~]',b'',screen).replace(b' ',b'')
        if b'S4preservednativehistory' in plain and not prompted and not skip_pty:
            os.write(master,b'\x1b[200~S4 private approval probe\x1b[201~'); prompted=True
        if prompted and b'S4privateapprovalprobe' in plain and not submitted:
            os.write(master,b'\r'); submitted=True
        if prompted and b'Wouldyouliketorun' in plain and not declined:
            os.write(master,b'\x1b'); declined=True
        while True:
            try: event=live.messages.get_nowait()
            except queue.Empty: break
            if declined and event.get('method')=='turn/completed': approval_complete=True
        if approval_complete and not exited:
            os.write(master,b'\x03\x03'); exited=True
    (RUN/'attach.bin').write_bytes(screen)
    if not skip_pty:
        assert declined and approval_complete and exited and terminal.poll()==0 and termios.tcgetattr(slave)==original
        assert not (RUN/'approval-must-not-exist').exists()
    os.close(master); os.close(slave)
    read_owner(second_receipt)
    Model.mode='sandbox'; live.events.clear()
    live.call('turn/start',{'threadId':thread,'input':[{'type':'text','text':'S4 fixed read-only sandbox fixture'}]})
    until=time.monotonic()+30
    while not any(e.get('method')=='turn/completed' for e in live.events):
        assert time.monotonic()<until
        live.events.append(live.messages.get(timeout=10))
    sandbox_output=next(o['output'] for o in Model.outputs if o['call_id']=='s4-'+Model.sandbox_file)
    (RUN/'sandbox-output.json').write_text(json.dumps(sandbox_output,indent=2))
    assert 'file:DENIED:' in str(sandbox_output) and 'network:DENIED:' in str(sandbox_output),sandbox_output
    assert not (RUN/'sandbox-must-not-exist').exists()
    for permission,sandbox,network in [('workspace','workspace-write','DENIED:'),('full-access','danger-full-access','ALLOWED')]:
        cli('session','defaults','set',durable,'--permission',permission,'--sandbox',sandbox,'--approval-policy','on-request')
        policy_review=action({'operation':'review_runtime','cutex_session_id':durable,'restart':True})
        assert policy_review['configuration']['sandbox']==sandbox and policy_review['configuration']['approval']=='on-request'
        second_receipt=action({'operation':'run','action_id':'s4-policy-'+permission,'review':policy_review})
        assert second_receipt['stage']=='ready'
        stock_pids.append(second_receipt['binding']['pid'])
        live=read_owner(second_receipt)
        live.call('thread/resume',{'threadId':thread,'excludeTurns':True})
        Model.sandbox_step=0; Model.outputs=[]; Model.sandbox_file='sandbox-'+permission
        live.events.clear()
        live.call('turn/start',{'threadId':thread,'input':[{'type':'text','text':'S4 fixed sandbox policy fixture'}]})
        until=time.monotonic()+30
        while not any(e.get('method')=='turn/completed' for e in live.events):
            assert time.monotonic()<until
            live.events.append(live.messages.get(timeout=10))
        output=next(o['output'] for o in Model.outputs if o['call_id']=='s4-'+Model.sandbox_file)
        (RUN/('sandbox-'+permission+'-output.json')).write_text(json.dumps(output,indent=2))
        assert 'file:ALLOWED' in str(output) and 'network:'+network in str(output),output
        assert (RUN/Model.sandbox_file).read_text()=='probe'
        assert sha(NATIVE/'config.toml')==shared_sha and store()['sessions'][durable]['explicit_launch']==contract
    # Private Human APIs assign the already-running ordinary Agent explicitly.
    # No implicit grant/Project ownership was created by stock activation.
    recipient=live.call('thread/start',{'cwd':str(RUN),'sandbox':'read-only','approvalPolicy':'on-request','ephemeral':False})['thread']['id']
    status,registered=api(bp,'/api/agents/register',{'id':'s4-recipient','name':'S4 Recipient','baseName':'S4 Recipient','sessionId':recipient,'profile':'beta','cwd':str(RUN),'pid':second_receipt['binding']['pid'],'groups':[],'registrationClass':'persistent'},token=BUS_TOKEN)
    assert status==200 and registered['ok'],(status,registered)
    target=next(r['cutex_session_id'] for r in store()['sessions'].values() if r.get('codex_session_id')==recipient)
    def mutation(action,kind,revision,**fields):
        return {'schema':'cutex/human-management-project-mutation/v1','action_id':action,'project_id':'s4-project','expected_authority_epoch':0 if kind=='create' else 1,'expected_project_revision':revision,'operation':{'kind':kind,**fields}}
    create=mutation('s4-create','create',0,director_cutex_session_id=durable,presentation={'display_name':'S4 Private','badge_label':'S4','color':'cyan'})
    for ident,name,assignment in [(durable,'S4 Formal Agent',create),(target,'S4 Recipient',None)]:
        status,cs=api(mp,'/v2/agent-management/durable-candidates'); assert status==200
        candidate=next(c for c in cs if c['cutex_session_id']==ident)
        status,result=api(mp,'/v2/agent-management/durable-import',{'action_id':'s4-import-'+ident,'candidate':candidate,'confirmed_formal_name':name,'assignment':assignment,'detach':None})
        assert status==200 and result['complete'],(status,result)
    status,result=api(mp,'/v2/agent-management/project-mutations',mutation('s4-add','add_member',1,cutex_session_id=target)); assert status==200,(status,result)
    protected=action({'operation':'review_runtime','cutex_session_id':durable,'restart':True},ok=False)
    assert 'director' in json.dumps(protected).lower(),protected
    headers={'X-Cutex-Agent-Id':second_receipt['runtime_agent_id'],'X-Cutex-Mcp-Thread-Id':thread,'X-Cutex-Mcp-Generation':str(second_receipt['expected_generation'])}
    query={'schema':'cutex/agent-management/v1','action_id':'s4-direct-query','operation':'query_managed','project_id':'s4-project'}
    status,result=api(bp,'/api/agent-management/v1/actions',query,token=BUS_TOKEN,headers=headers)
    assert status==200 and 'unauthorized' not in json.dumps(result).lower(),(status,result)
    denials=[]
    for label,hs in [('stale',{**headers,'X-Cutex-Mcp-Generation':'1'}),('foreign',{**headers,'X-Cutex-Mcp-Thread-Id':recipient}),('missing',{k:v for k,v in headers.items() if k!='X-Cutex-Mcp-Generation'})]:
        status,result=api(bp,'/api/agent-management/v1/actions',query,token=BUS_TOKEN,headers=hs)
        assert 'unauthorized' in json.dumps(result).lower(),(label,status,result); denials.append(label)
    inventory=live.call('mcpServerStatus/list',{'threadId':thread,'detail':'toolsAndAuthOnly'})
    assert 'caller_cutex_session_id' not in json.dumps(inventory)
    (RUN/'mcp-inventory.json').write_text(json.dumps(inventory,indent=2))
    Model.mode='mcp'; live.events.clear()
    live.call('turn/start',{'threadId':thread,'input':[{'type':'text','text':'S4 fixed outbound fixture'}]})
    until=time.monotonic()+40
    while not any(e.get('method')=='turn/completed' for e in live.events):
        assert time.monotonic()<until
        live.events.append(live.messages.get(timeout=10))
    outputs={o['call_id']:o['output'] for o in Model.outputs}
    (RUN/'mcp-outputs.json').write_text(json.dumps(outputs,indent=2))
    assert 's4-project' in str(outputs['s4-mcp-1']),outputs
    assert 'message_id' in str(outputs['s4-mcp-2']),outputs
    assert BUS_TOKEN not in json.dumps(outputs) and HUMAN_TOKEN not in json.dumps(outputs)
    (RUN/'PASS.json').write_text(json.dumps({'durable':durable,'native':thread,'stage':'four-generations-outbound','sandbox_modes':['read-only','workspace-write','danger-full-access'],'pty':'omitted' if skip_pty else ('diagnostic relay' if len(sys.argv)>2 else 'production stock-attach approval declined'),'denials':denials,'model_calls':Model.calls},indent=2))
finally:
    (RUN/'outcome-facts.json').write_text(json.dumps({'model_calls':Model.calls,'approval_step':Model.approval_step,'mcp_step':Model.mcp_step,'approval_ui_seen':globals().get('declined',False),'approval_turn_complete':globals().get('approval_complete',False),'approval_file_exists':(RUN/'approval-must-not-exist').exists()},indent=2))
    # An HTTP error may still have committed a child binding; reconcile only
    # this probe's exact activated record, without guessing by name or cwd.
    if 'durable' in globals():
        saved=store()['sessions'].get(durable,{})
        binding=saved.get('app_server_runtime')
        if binding and saved.get('explicit_launch',{}).get('bundle_manifest')==str(RUN/'bundle.json'):
            stock_pids.append(binding['pid'])
    # Only private PIDs returned by this probe's actual operation are candidates.
    for pid in set(stock_pids):
        try:
            if (os.getpgid(pid)==pid and Path(f'/proc/{pid}/exe').readlink()==STOCK
                    and Path(f'/proc/{pid}/cwd').readlink()==RUN):
                os.killpg(pid,signal.SIGKILL)
        except ProcessLookupError: pass
    for child in reversed(children): stop_owned(child)
    for log in logs: log.close()
    model.shutdown()
