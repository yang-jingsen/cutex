"""S5a private stock MCP / actual Task provider oracle; no live homes or paid model.

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
# Deterministic refusal proof: no listener or live endpoint is contacted.
tripwire=subprocess.run(['/usr/bin/python3','-c',"import socket; socket.socket().connect(('127.0.0.1',1))"],
                       env=env,capture_output=True)
assert tripwire.returncode==97 and b'S4_PROBE_UNOWNED_ENDPOINT_BLOCKED' in tripwire.stderr
BUS_TOKEN = 's4-private-bus-fixture-only'
HUMAN_TOKEN = 's4-private-human-fixture-only'
children=[]; logs=[]; stock_pids=[]; agents={}; facts=[]
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
    calls_by_id={}
    pending=None
    output=None
    inventory=[]
    discovered=[]
    def log_message(self,*args): pass
    def do_POST(self):
        assert self.path=='/v1/responses'
        data=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        Model.calls+=1; assert Model.calls<=240
        item={'type':'message','role':'assistant','id':f's5a-fixture-{Model.calls}','content':[{'type':'output_text','text':'Private S5a fixture complete'}]}
        Model.inventory.append(data.get('tools',[]))
        Model.discovered.extend(i for i in data.get('input',[]) if i.get('type')=='tool_search_output')
        if Model.pending:
            callid,name,args=Model.pending
            outputs=[i for i in data.get('input',[]) if i.get('type')=='function_call_output' and i.get('call_id')==callid]
            if outputs:
                Model.output=outputs[-1]['output']
            else:
                n=Model.calls_by_id.get(callid,0); Model.calls_by_id[callid]=n+1
                if n==0:
                    item={'type':'tool_search_call','call_id':callid+'-search','execution':'client','arguments':{'query':name,'limit':4}}
                else:
                    item={'type':'function_call','call_id':callid,'namespace':'mcp__cutex','name':name,'arguments':json.dumps(args)}
        events=[{'type':'response.created','response':{'id':f's5a-response-{Model.calls}'}},
                {'type':'response.output_item.done','item':item},
                {'type':'response.completed','response':{'id':f's5a-response-{Model.calls}','usage':{'input_tokens':0,'output_tokens':0,'total_tokens':0}}}]
        encoded=''.join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events).encode()
        self.send_response(200); self.send_header('Content-Type','text/event-stream');self.send_header('Content-Length',str(len(encoded)));self.end_headers();self.wfile.write(encoded)

def wait_turn(rpc):
    until=time.monotonic()+45
    while not any(e.get('method')=='turn/completed' for e in rpc.events):
        assert time.monotonic()<until,'private turn timeout'
        rpc.events.append(rpc.messages.get(timeout=15))

def mcp(actor,name,args):
    a=agents[actor]; rpc=a['rpc']
    Model.pending=(f's5a-call-{len(facts)}',name,args); Model.output=None
    rpc.events.clear()
    rpc.call('turn/start',{'threadId':a['native'],'input':[{'type':'text','text':'Execute the fixed private S5a protocol fixture'}]})
    wait_turn(rpc)
    assert Model.output is not None,('missing MCP result',actor,name,args)
    output=Model.output
    if isinstance(output,str): output=json.loads(output)
    if isinstance(output,list):
        values=[json.loads(c['text']) for c in output if c.get('type')=='input_text' and c.get('text','').startswith('{')]
        assert len(values)==1,output
        output=values[0]
    # Stock wraps MCP text content, but never treat transport framing as success.
    if 'content' in output:
        output=json.loads(next(c['text'] for c in output['content'] if c.get('type')=='text'))
    assert BUS_TOKEN not in json.dumps(output) and HUMAN_TOKEN not in json.dumps(output)
    facts.append({'actor':actor,'tool':name,'arguments':args,'result':output})
    (RUN/'operations.json').write_text(json.dumps(facts,indent=2))
    Model.pending=None
    return output

def director(op,action,**kw):
    return mcp('director','cutex_task_service_director',{'operation':op,'action_id':action,**kw})
def worker(op,action,assignment='a-main',**kw):
    return mcp('worker','cutex_task_service',{'operation':op,'action_id':action,'assignment_id':assignment,**kw})
def headers(actor):
    a=agents[actor];return {'X-Cutex-Agent-Id':a['receipt']['runtime_agent_id'],'X-Cutex-Mcp-Thread-Id':a['native'],'X-Cutex-Mcp-Generation':str(a['receipt']['expected_generation'])}
def direct(actor,path,body,hs=None):
    return api(bp,path,body,token=BUS_TOKEN,headers=hs or headers(actor))

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

    # Two genuine native threads are created once, then explicitly adopted and
    # activated via existing public APIs. No store hand edits / model authority.
    sock=RUN/'bootstrap.sock'
    args=[STOCK,'-c','model="gpt-5.4"','-c','model_provider="s4-fake"']
    args += ['-c','model_providers.s4-fake={'+','.join(k+'='+json.dumps(v) for k,v in provider.items())+'}']
    bootstrap=owner([*args,'app-server','--listen','unix://'+str(sock)],'bootstrap')
    until=time.monotonic()+15
    while not sock.exists():
        assert bootstrap.poll() is None and time.monotonic()<until
        threading.Event().wait(.02)
    rpc=RPC(sock);rpc.call('initialize',{'clientInfo':{'name':'s5a-bootstrap','version':'1'},'capabilities':{'experimentalApi':True}});rpc.notify('initialized')
    for role in ['director','worker']:
        native=rpc.call('thread/start',{'cwd':str(RUN),'sandbox':'read-only','approvalPolicy':'on-request','ephemeral':False})['thread']['id']
        rpc.events.clear();rpc.call('turn/start',{'threadId':native,'input':[{'type':'text','text':'Preserve this private S5a native identity'}]});wait_turn(rpc)
        assert rpc.call('thread/read',{'threadId':native,'includeTurns':True})['thread']['turns']
        agents[role]={'native':native,'name':'S5a '+role}
    stop_owned(bootstrap)
    for role,a in agents.items():
        cli('session','adopt',a['native'],'--name',a['name'],'--cwd',RUN)
        a['durable']=next(r['cutex_session_id'] for r in store()['sessions'].values() if r.get('codex_session_id')==a['native'])
        cli('session','defaults','set',a['durable'],'--runtime-backend','host','--permission','read-only','--sandbox','read-only','--approval-policy','on-request')
    bus=owner([CUTEX,'agent','serve','--port',bp],'bus');ready(bp,bus)
    mp=port();env['S4_TEST_ALLOWED_PORTS']+=f',{mp}'
    management=owner([CUTEX,'management','serve','--port',mp],'management');ready(mp,management)
    bundle={'version':1,'upstream_commit':'3d2ee51ca2d5db578f328aa75e20aa22c0197c9a','executable':verified(STOCK),'code_mode_host':verified(STOCK.with_name('codex-code-mode-host')),'facade':verified(MCP),'schema':verified(ROOT/'s4-schema/codex_app_server_protocol.schemas.json'),'shared_config':verified(NATIVE/'config.toml')}
    manifest=RUN/'bundle.json';manifest.write_text(json.dumps(bundle))
    for role,a in agents.items():
        contract={'version':1,'native_id':a['native'],'native_home':str(NATIVE),'bundle_manifest':str(manifest),'bundle_sha256':sha(manifest)}
        review=action({'operation':'review','cutex_session_id':a['durable'],'contract':contract})
        action({'operation':'activate','action_id':'s5a-activate-'+role,'review':review})
        review=action({'operation':'review_runtime','cutex_session_id':a['durable'],'restart':False})
        receipt=action({'operation':'run','action_id':'s5a-run-'+role,'review':review})
        assert receipt['stage']=='ready',receipt
        a['receipt']=receipt;stock_pids.append(receipt['binding']['pid'])
        live=RPC(Path(receipt['binding']['endpoint'].removeprefix('unix://')))
        live.call('initialize',{'clientInfo':{'name':'s5a-observer','version':'1'},'capabilities':{'experimentalApi':True}});live.notify('initialized')
        live.call('thread/resume',{'threadId':a['native'],'excludeTurns':True});a['rpc']=live
    def mutation(action_id,kind,revision,**fields):
        return {'schema':'cutex/human-management-project-mutation/v1','action_id':action_id,'project_id':'s5a-project','expected_authority_epoch':0 if kind=='create' else 1,'expected_project_revision':revision,'operation':{'kind':kind,**fields}}
    create=mutation('s5a-project-create','create',0,director_cutex_session_id=agents['director']['durable'],presentation={'display_name':'S5a Private','badge_label':'S5','color':'cyan'})
    for role,a in agents.items():
        status,cs=api(mp,'/v2/agent-management/durable-candidates');assert status==200
        candidate=next(c for c in cs if c['cutex_session_id']==a['durable'])
        status,result=api(mp,'/v2/agent-management/durable-import',{'action_id':'s5a-import-'+role,'candidate':candidate,'confirmed_formal_name':a['name'],'assignment':create if role=='director' else None,'detach':None})
        assert status==200 and result['complete'],(status,result)
    status,result=api(mp,'/v2/agent-management/project-mutations',mutation('s5a-add','add_member',1,cutex_session_id=agents['worker']['durable']));assert status==200,(status,result)
    # Keep normal provider notifications pending; no fake inbound wake or polling.
    project='s5a-project'
    if len(sys.argv)>2 and sys.argv[2]=='schema':
        assert director('query','schema-query',selector={'kind':'all'})['status']=='current_state'
        result=mcp('worker','cutex_task_service',{'operation':'start','action_id':'schema-worker'})
        assert result['code']=='missing_assignment_id',result
        (RUN/'discovered-tools.json').write_text(json.dumps(Model.discovered,indent=2))
        assert Model.discovered,'actual Core tool-search schemas not observed'
        raw=json.dumps(Model.discovered)
        assert 'cutex_task_service_director' in raw and 'assignment_id' in raw and 'task_revision' in raw
        for forbidden in ['caller_cutex_session_id','attempt_token','expected_assignment_revision','runtime_generation',BUS_TOKEN,HUMAN_TOKEN]:
            assert forbidden not in raw,forbidden
        (RUN/'PASS.json').write_text(json.dumps({'mode':'actual Core tool-search schemas','operations':len(facts),'model_calls':Model.calls,'facade_sha256':bundle['facade']['sha256']},indent=2))
        sys.exit(0)
    base={'project_id':project,'workflow_id':'wf-main','task_id':'t-main','task_revision':1,'opaque_contract':' exact 私有任务\n','completion_policy':'director_acceptance'}
    supplement=len(sys.argv)>2 and sys.argv[2]=='supplement'
    if not supplement:
        result=director('create_revision','d-create',**base);assert result['status']=='committed',result
        assign={'project_id':project,'task_id':'t-main','task_revision':1,'assignment_id':'a-main','assignee_cutex_session_id':agents['worker']['durable'],'summary':'Private S5a work'}
        result=director('assign','d-assign',**assign);assert result['status']=='committed',result
        result=director('query','d-query',selector={'kind':'assignment','assignment_id':'a-main'});assert result['assignments'][0]['assignee_cutex_session_id']==agents['worker']['durable'],result
        start=worker('start','w-start');assert start['status']=='committed',start
        assert worker('start','w-start')==start
        assert worker('report_status','w-start',summary='changed')['status']=='conflict'
        for op,kw in [('report_status',{'summary':'progress'}),('block',{'summary':'private blocker'}),('resume',{}),('submit',{'result_sha256':'a'*64,'result_reference':'private-result-1'})]:
            result=worker(op,'w-'+op,**kw);assert result['status']=='committed',result
        result=director('request_changes','d-repair',assignment_id='a-main',decision_reference='private repair');assert result['status']=='committed',result
        # Current provider request_changes resumes the SAME attempt; no new start.
        result=worker('report_status','w-repair-status',summary='repairing');assert result['status']=='committed' and result['attempt_number']==1,result
        result=worker('submit','w-repair-submit',result_sha256='b'*64,result_reference='private-result-2');assert result['status']=='committed',result
        result=director('accept_result','d-accept',assignment_id='a-main');assert result['status']=='committed',result
        # A real existing assignment owned by the Director is foreign to Worker.
        result=director('create_and_assign','d-foreign',**{**base,'workflow_id':'wf-foreign','task_id':'t-foreign','assignment_id':'a-foreign','assignee_cutex_session_id':agents['director']['durable'],'summary':'foreign assignment'});assert result['status']=='committed',result
        result=worker('start','w-foreign',assignment='a-foreign');assert result['status']=='no_write' and result['code']=='unauthorized',result
        # Actual response loss: send a prepared public action and close without
        # reading its reply. MCP exact replay must observe/complete the same action.
        result=director('create_and_assign','d-lost',**{**base,'workflow_id':'wf-lost','task_id':'t-lost','assignment_id':'a-lost','assignee_cutex_session_id':agents['worker']['durable'],'summary':'lost response'});assert result['status']=='committed',result
        semantic={'operation':'start','body':{'schema':'cutex/task-service-action/v2','action_id':'w-lost','assignment_id':'a-lost'}}
        status,prepared=direct('worker','/api/task/v2/worker-prepare',{'schema':'cutex/task-service-worker-prepare/v2','action':semantic});assert status==200 and prepared['outcome']['kind']=='prepared',prepared
        payload=json.dumps(prepared['outcome']['body']).encode()
        with socket.create_connection(('127.0.0.1',bp)) as lost:
            head={'Host':f'127.0.0.1:{bp}','Authorization':'Bearer '+BUS_TOKEN,'Content-Type':'application/json','Content-Length':str(len(payload)),'Connection':'close',**headers('worker')}
            lost.sendall(('POST /api/task/v2/actions HTTP/1.1\r\n'+''.join(k+': '+v+'\r\n' for k,v in head.items())+'\r\n').encode()+payload)
        result=worker('start','w-lost',assignment='a-lost');assert result['status']=='committed' and result['attempt_number']==1,result
        # Concrete provider continuation: creation succeeds, target lookup fails.
        partial={**base,'workflow_id':'wf-partial','task_id':'t-partial','assignment_id':'a-partial','assignee_cutex_session_id':'cutex.00000000-0000-0000-0000-000000000099','summary':'missing private target'}
        result=director('create_and_assign','d-partial',**partial)
        assert result['status']!='committed' and result['continuation']=={'phase':'create_revision_committed','retry_action_id':'d-partial'},result
        assert director('create_and_assign','d-partial',**partial)==result
        # Mutually exclusive closures use different explicit task/assignment IDs.
        for end in ['decline','abort_attempt','fail_result','cancel']:
            ident='case-'+end
            result=director('create_and_assign','d-'+ident,**{**base,'workflow_id':'wf-'+ident,'task_id':'t-'+ident,'assignment_id':'a-'+ident,'assignee_cutex_session_id':agents['worker']['durable'],'summary':ident});assert result['status']=='committed',result
            if end in ['abort_attempt','fail_result']:
                assert worker('start','w-start-'+ident,assignment='a-'+ident)['status']=='committed'
            if end=='fail_result':
                assert worker('submit','w-submit-'+ident,assignment='a-'+ident,result_sha256='c'*64,result_reference='failed-result')['status']=='committed'
            result=worker(end,'w-end-'+ident,assignment='a-'+ident) if end in ['decline','abort_attempt'] else director(end,'d-end-'+ident,assignment_id='a-'+ident)
            assert result['status']=='committed',result
        result=mcp('worker','cutex_task_service_director',{'operation':'query','action_id':'worker-denied','selector':{'kind':'all'}});assert result['status']=='no_write',result
        result=worker('start','foreign-assignment',assignment='does-not-belong');assert result['status']!='committed',result
        result=mcp('worker','cutex_task_service',{'operation':'start','action_id':'spoof','assignment_id':'a-main','caller_cutex_session_id':agents['director']['durable']});assert result['status']=='no_write',result
    else:
        result=director('create_and_assign','supp-create',**{**base,'assignment_id':'a-main','assignee_cutex_session_id':agents['worker']['durable'],'summary':'final artifact smoke'});assert result['status']=='committed',result
        changed={**base,'opaque_contract':'changed exact action','assignment_id':'a-main','assignee_cutex_session_id':agents['worker']['durable'],'summary':'final artifact smoke'}
        result=director('create_and_assign','supp-create',**changed);assert result['status']=='conflict',result
        assert worker('start','supp-start')['status']=='committed'
        assert worker('submit','supp-submit',result_sha256='d'*64,result_reference='supp-result')['status']=='committed'
        assert director('accept_result','supp-accept',assignment_id='a-main')['status']=='committed'
    result=mcp('director','query_managed',{'action_id':'s5a-qm','project_id':project});assert project in json.dumps(result),result
    result=mcp('director','send',{'to':agents['worker']['durable'],'message':'private S5a outbound','external_message_id':'s5a-send','delivery_mode':'passive'});assert result['ok'] and result['id'] and result['to_cutex_session_id']==agents['worker']['durable'],result
    raw_query={'schema':'cutex/task-service-director-action/v2','operation':'query','action_id':'s5a-negative','selector':{'kind':'all'}}
    denials=[]
    for label,hs in [('stale',{**headers('director'),'X-Cutex-Mcp-Generation':'99'}),('foreign',{**headers('director'),'X-Cutex-Mcp-Thread-Id':agents['worker']['native']}),('missing',{k:v for k,v in headers('director').items() if k!='X-Cutex-Mcp-Thread-Id'})]:
        status,result=direct('director','/api/task/v2/director-action',raw_query,hs)
        assert result['status']=='no_write' and result['code']=='unauthorized',(label,status,result);denials.append(label)
    # Actual facade process with absent Core metadata must reject before HTTP.
    facade_env={**env,'CUTEX_AGENT_ID':agents['worker']['receipt']['runtime_agent_id'],'CUTEX_RUNTIME_GENERATION':str(agents['worker']['receipt']['expected_generation']),'CUTEX_AGENT_BUS_URL':f'http://127.0.0.1:{bp}/','CUTEX_AGENT_BUS_TOKEN':BUS_TOKEN,'S4_TEST_ALLOWED_PORTS':''}
    missing={'jsonrpc':'2.0','id':1,'method':'tools/call','params':{'name':'cutex_task_service','arguments':{'operation':'start','action_id':'missing-core','assignment_id':'a-main'}}}
    rejected=subprocess.run([str(MCP)],input=json.dumps(missing)+'\n',text=True,capture_output=True,env=facade_env,cwd=RUN,timeout=10)
    assert rejected.returncode==0 and json.loads(rejected.stdout)['error']['code']==-32602
    assert BUS_TOKEN not in rejected.stdout and HUMAN_TOKEN not in rejected.stdout
    denials.append('missing_core_before_transport')
    assert sha(NATIVE/'config.toml')==shared_sha
    # Current provider persistence, not tool transport success, is final oracle.
    snapshots=list(CONF.glob('**/task-service-provider-v2.json'));assert len(snapshots)==1,snapshots
    snapshot=json.loads(snapshots[0].read_text())
    assert snapshot['assignments']['a-main']['closure']['reason']=='completed'
    if not supplement:
        assert snapshot['assignments']['a-lost']['active_attempt']==1
        assert 'w-lost' in snapshot['receipts']
    assert snapshot['task_revisions']['t-main']['1']['contract_sha256']==hashlib.sha256(base['opaque_contract'].encode()).hexdigest()
    summary={'assignments':{k:{'state':v['state'],'active_attempt':v['active_attempt'],'closure':v['closure']} for k,v in snapshot['assignments'].items()},'receipt_ids':list(snapshot['receipts']),'task_ids':list(snapshot['task_revisions'])}
    (RUN/'task-state-summary.json').write_text(json.dumps(summary,indent=2))
    (RUN/'model-tools.json').write_text(json.dumps(Model.inventory,indent=2))
    assert HUMAN_TOKEN not in json.dumps(Model.inventory) and BUS_TOKEN not in json.dumps(Model.inventory)
    (RUN/'PASS.json').write_text(json.dumps({'operations':len(facts),'model_calls':Model.calls,'denials':denials,'actors':{k:{'durable':a['durable'],'native':a['native']} for k,a in agents.items()},'facade_sha256':bundle['facade']['sha256'],'inbound':'not implemented; harness drives assigned Worker'},indent=2))
finally:
    (RUN/'outcome-facts.json').write_text(json.dumps({'model_calls':Model.calls,'operations':len(facts)},indent=2))
    for a in agents.values():
        if a.get('durable'):
            saved=store()['sessions'].get(a['durable'],{})
            binding=saved.get('app_server_runtime')
            if binding and saved.get('explicit_launch',{}).get('bundle_manifest')==str(RUN/'bundle.json'): stock_pids.append(binding['pid'])
    for pid in set(stock_pids):
        try:
            if os.getpgid(pid)==pid and Path(f'/proc/{pid}/exe').readlink()==STOCK and Path(f'/proc/{pid}/cwd').readlink()==RUN: os.killpg(pid,signal.SIGKILL)
        except ProcessLookupError: pass
    for child in reversed(children): stop_owned(child)
    for log in logs: log.close()
    model.shutdown()
