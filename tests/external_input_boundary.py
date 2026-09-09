"""S6c1 composed Cutex launch + frozen U+S6 + Rust ingress client oracle.

Private owned fixtures only. Reuses S4's explicit endpoint tripwire and native
Unix framing, not its old campaign. Fake Responses is not a real model provider.
Usage: python3 tests/external_input_boundary.py s6c01 /exact/controller-test-binary
"""
import ast
from pathlib import Path

# Reuse only S4 setup/helpers preceding its Model class. No S4 lifecycle campaign.
tree = ast.parse(Path(__file__).with_name('stock_launch_boundary.py').read_text())
cut = next(i for i, n in enumerate(tree.body) if isinstance(n, ast.ClassDef) and n.name == 'Model')
exec(compile(ast.Module(body=tree.body[:cut], type_ignores=[]), 'S4-private-helpers', 'exec'), globals())
assert socket.if_nameindex() == [(1, 'lo')], 'run in private network namespace'
with socket.socket() as oracle:
    try: oracle.connect(('192.0.2.1',443))
    except OSError as error:
        import errno
        assert error.errno == errno.ENETUNREACH
    else: raise AssertionError('external network reachable')
(RUN/'network.json').write_text(json.dumps({'interfaces':socket.if_nameindex(),'external':'ENETUNREACH'}))
for key in list(env):
    if key.startswith('CUTEX_STOCK_TEST_'): del env[key]
CONTROLLER = Path(sys.argv[2]).resolve()
assert CONTROLLER.is_file() and CONTROLLER.is_relative_to(ROOT/'target/debug/deps')
PATCHED = Path('/mnt/mambo/PersonaProjects/cutex-light-core-r1/artifacts/native-c2aaceb4/bin/codex-app-server')
SCHEMA = Path('/mnt/mambo/PersonaProjects/cutex-light-core-r1/artifacts/s6c1-schema-handoff/codex_app_server_protocol.schemas.json')
assert sha(PATCHED) == 'b70d48151c9deb76a9c0ab14a820c582f2bc12a73bbb1512fee9b2f1bec9fa60'
assert sha(SCHEMA) == '00e035e34ac1034ee34473f8f68b7704d6058c5b180ff4f4b6cad9fadab3a86d'
controllers = []; peers = []; facts = []; durable = None

class Model(http.server.BaseHTTPRequestHandler):
    calls = 0
    empty = False
    requests = []
    observed = queue.Queue()
    def log_message(self, *args): pass
    def do_POST(self):
        assert self.path == '/v1/responses'
        data = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        Model.calls += 1; assert Model.calls <= 12
        Model.requests.append(data)
        events = [{'type':'response.created','response':{'id':f's6-{Model.calls}'}}]
        if not Model.empty:
            events.append({'type':'response.output_item.done','item':{'type':'message','role':'assistant',
                'id':f's6-output-{Model.calls}','content':[{'type':'output_text','text':'Private external data observed'}]}})
        events.append({'type':'response.completed','response':{'id':f's6-{Model.calls}',
            'usage':{'input_tokens':0,'output_tokens':0,'total_tokens':0}}})
        body = ''.join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events).encode()
        self.send_response(200); self.send_header('Content-Type','text/event-stream')
        self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
        Model.observed.put(Model.calls)

class Controller:
    def __init__(self, owner_id, generation, reject=False):
        e = {**env, 'S6_PRIVATE_OWNER':owner_id, 'S6_PRIVATE_GENERATION':str(generation)}
        log = open(RUN/f'controller-{len(controllers)}.stderr','wb'); logs.append(log)
        self.p = subprocess.Popen([str(CONTROLLER),'--ignored','--exact','private_external_input_controller','--nocapture'],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True, env=e, cwd=RUN, start_new_session=True)
        children.append(self.p); controllers.append(self)
        self.lines = queue.Queue()
        def read():
            for line in self.p.stdout:
                self.lines.put(line.rstrip())
            self.lines.put('EOF')
        threading.Thread(target=read,daemon=True).start()
        response = self.read()
        if reject: assert isinstance(response,dict) and 'error' in response, response
        else: assert response == 'READY', response
        self.initial = response
    def read(self):
        while True:
            line = self.lines.get(timeout=60)
            if line == 'S6_READY': return 'READY'
            if line.startswith('S6_REPLY '): return json.loads(line[9:])
            assert line != 'EOF', 'controller exited without result'
    def call(self, op, params, reject=False):
        self.p.stdin.write(json.dumps({'operation':op,'params':params})+'\n'); self.p.stdin.flush()
        result = self.read()
        facts.append({'operation':op,'error':result.get('error'),'result':result.get('result')})
        if reject: assert 'error' in result, result
        else: assert 'result' in result, result
        return result.get('result',result)
    def close(self):
        if self.p.poll() is None: self.p.stdin.close(); assert self.p.wait(timeout=10) == 0

def native_rpc(receipt):
    rpc = RPC(Path(receipt['binding']['endpoint'].removeprefix('unix://'))); peers.append(rpc)
    init = rpc.call('initialize',{'clientInfo':{'name':'s6-private-observer','version':'1'},'capabilities':{'experimentalApi':True}})
    rpc.notify('initialized'); return rpc, init
def envelope(generation, ident, text):
    message = {'id':ident,'source':{'kind':'service','id':'private-test-service'},'type':'private.event.v1','delivery':'after_turn','text':text}
    fields = [durable, thread, ident, 'service', 'private-test-service', 'private.event.v1', 'after_turn', text]
    digest = hashlib.sha256(b'codex:external-input:v1\0'+b''.join(struct.pack('>Q',len(s.encode()))+s.encode() for s in fields)).hexdigest()
    return {'version':1,'ownerId':durable,'threadId':thread,'runtimeGeneration':generation,'message':message,'semanticSha256':digest}
def key(e): return {'messageId':e['message']['id'],'semanticSha256':e['semanticSha256']}
def settled(controller, e, state):
    deadline = time.monotonic()+30
    while True:
        result = controller.call('status',[key(e)])['statuses'][0]
        if result['processing']['state'] == state:
            assert result['deliveryState']=='context_persisted' and result['receipt']; return result
        assert time.monotonic()<deadline, result
        # Actual status drives convergence; no sleep/persistence barrier.
def launch(ident, name, restart=False, policy=None):
    request={'operation':'review_runtime','cutex_session_id':ident,'restart':restart}
    if policy is not None: request['receiver_canonical_byte_limit']=policy
    review=action(request)
    run={'operation':'run','action_id':name,'review':review}
    receipt=action(run); assert receipt['stage']=='ready' and action(run)==receipt
    stock_pids.append(receipt['binding']['pid']); return receipt

model=http.server.ThreadingHTTPServer(('127.0.0.1',0),Model)
threading.Thread(target=model.serve_forever,daemon=True).start()
try:
    bp=port(); env['S4_TEST_ALLOWED_PORTS']=f'{bp},{model.server_port}'
    (CONF/'config.json').write_text(json.dumps({'agent_bus_enabled':True,'agent_bus_port':bp,'agent_bus_token':BUS_TOKEN,
        'management_api_token':HUMAN_TOKEN,'default_profile':'alpha'}))
    profile_id=str(uuid.uuid4()); folder=CONF/'profiles'/profile_id; folder.mkdir(parents=True)
    provider={'name':'s6-private','base_url':f'http://127.0.0.1:{model.server_port}/v1','wire_api':'responses',
        'requires_openai_auth':False,'supports_websockets':False}
    (folder/'config.toml').write_text('model="unknown-private-model"\nmodel_provider="s6-private"\n[model_providers.s6-private]\n'+
        '\n'.join(k+'='+json.dumps(v) for k,v in provider.items())+'\n')
    (CONF/'accounts.json').write_text(json.dumps({'version':3,'accounts':[{'id':profile_id,'name':'alpha','email':None,'plan_type':None,'last_used_at':None}],'active_account_id':None}))
    (NATIVE/'config.toml').write_text(f'[projects.{json.dumps(str(RUN))}]\ntrust_level="trusted"\n[analytics]\nenabled=false\n')
    shared_sha=sha(NATIVE/'config.toml')
    sock=RUN/'bootstrap.sock'
    args=[PATCHED,'-c','model="unknown-private-model"','-c','model_provider="s6-private"',
        '-c','model_providers.s6-private={'+','.join(k+'='+json.dumps(v) for k,v in provider.items())+'}',
        '--disable-plugin-startup-tasks-for-tests','--listen','unix://'+str(sock)]
    bootstrap=owner(args,'bootstrap')
    until=time.monotonic()+15
    while not sock.exists():
        assert bootstrap.poll() is None and time.monotonic()<until
        threading.Event().wait(.02)
    rpc=RPC(sock); peers.append(rpc)
    rpc.call('initialize',{'clientInfo':{'name':'s6-neutral-fixture','version':'1'},'capabilities':{'experimentalApi':True}}); rpc.notify('initialized')
    threads=[]
    for _ in range(2):
        t=rpc.call('thread/start',{'cwd':str(RUN),'sandbox':'read-only','approvalPolicy':'on-request','ephemeral':False,'historyMode':'paginated'})['thread']['id']
        assert rpc.call('thread/read',{'threadId':t,'includeTurns':True})['thread']['turns']==[]
        threads.append(t)
    stop_owned(bootstrap); assert Model.calls==0
    ids=[]
    for n,t in enumerate(threads):
        cli('session','adopt',t,'--name',f'Private S6 Agent {n}','--cwd',RUN)
        ident=next(r['cutex_session_id'] for r in store()['sessions'].values() if r.get('codex_session_id')==t)
        cli('session','defaults','set',ident,'--runtime-backend','host','--permission','read-only','--sandbox','read-only','--approval-policy','on-request')
        ids.append(ident)
    durable, legacy=ids; thread=threads[0]
    bus=owner([CUTEX,'agent','serve','--port',bp],'bus'); ready(bp,bus)
    mp=port(); env['S4_TEST_ALLOWED_PORTS']+=f',{mp}'
    management=owner([CUTEX,'management','serve','--port',mp],'management'); ready(mp,management)
    for n,(ident,t,binary) in enumerate(zip(ids,threads,[PATCHED,STOCK])):
        bundle={'version':2 if n==0 else 1,'upstream_commit':'3d2ee51ca2d5db578f328aa75e20aa22c0197c9a',
            'executable':verified(binary),'code_mode_host':verified(binary.with_name('codex-code-mode-host')),
            'facade':verified(MCP),'schema':verified(SCHEMA if n==0 else ROOT/'s4-schema/codex_app_server_protocol.schemas.json'),
            'shared_config':verified(NATIVE/'config.toml')}
        if n==0: bundle['native_patch_commit']='c2aaceb411b7851806c62435b97895a63a7d34cd'
        manifest=RUN/f'bundle-{n}.json'; manifest.write_text(json.dumps(bundle))
        contract={'version':1,'native_id':t,'native_home':str(NATIVE),'bundle_manifest':str(manifest),'bundle_sha256':sha(manifest)}
        review=action({'operation':'review','cutex_session_id':ident,'contract':contract})
        assert api(mp,'/v2/agent-management/explicit-launch',{'operation':'activate','action_id':f's6-deny-{n}','review':review},token=BUS_TOKEN)[0]==401
        activation={'operation':'activate','action_id':f's6-activate-{n}','review':review}; receipt=action(activation); assert action(activation)==receipt
    stock=launch(legacy,'s6-stock')
    old_rpc,old_init=native_rpc(stock); assert old_init.get('externalInputVersion') is None
    denied=Controller(legacy,stock['expected_generation'],reject=True)
    assert 'registration-only' in denied.initial['error']
    first=launch(durable,'s6-default'); gen=first['expected_generation']
    live,init=native_rpc(first); assert init['externalInputVersion']==1
    controller=Controller(durable,gen)
    binding=json.loads((Path(first['binding']['runtime_dir'])/'external-input.json').read_text())
    assert binding=={'version':1,'ownerId':durable,'threadId':thread,'runtimeGeneration':gen}
    for policy in [None,0,False,-1,1.5,'unknown',4294967296]:
        action({'operation':'review_runtime','cutex_session_id':durable,'restart':True,'receiver_canonical_byte_limit':policy},ok=False)
    e=envelope(gen,'s6-default-event','A concise private external event.')
    for field,value in [('ownerId',legacy),('threadId',threads[1]),('runtimeGeneration',gen+1)]:
        wrong={**e,field:value}; controller.call('submit',wrong,reject=True)
        try: live.call('thread/externalInput/submit',wrong)
        except AssertionError: pass
        else: raise AssertionError('native mismatch accepted')
    controller.call('submit',e)
    status=settled(controller,e,'output_observed'); original_receipt=status['receipt']
    assert controller.call('submit',e)['statuses'][0]['receipt']==original_receipt
    assert controller.call('status',[key(e)])['statuses'][0]['receipt']==original_receipt
    hints=[]
    for _ in range(12):
        hint=controller.call('hint',None)
        if hint:
            assert hint=={'threadId':thread,'messageId':e['message']['id']}; hints.append(hint); break
    assert hints, 'real statusChanged was not parsed'
    controller.call('submit',envelope(gen,'s6-too-large','x'*12000),reject=True)
    # Marked generic restart must refuse without stopping this owner.
    generic=subprocess.run([str(CUTEX),'session','online',durable],env=env,cwd=RUN,capture_output=True,timeout=30)
    assert generic.returncode!=0 and b'explicit_stock_launch_required' in generic.stderr and process_identity(first['binding']['pid']) is not None
    second=launch(durable,'s6-off',True,'off'); gen2=second['expected_generation']
    controller.call('status',[key(e)],reject=True); controller.close()
    controller=Controller(durable,gen2)
    e2={**e,'runtimeGeneration':gen2}
    assert controller.call('submit',e2)['statuses'][0]['receipt']==original_receipt
    assert controller.call('status',[key(e2)])['statuses'][0]['receipt']==original_receipt
    # Raw native controller sends once and deliberately abandons the response.
    # Rust client subsequently reconciles exact key; it never invents a new ID.
    lost=envelope(gen2,'s6-lost-reply','x'*12000)
    lost_rpc,_=native_rpc(second)
    lost_rpc.send(json.dumps({'jsonrpc':'2.0','id':900,'method':'thread/externalInput/submit','params':lost}).encode())
    lost_rpc.sock.shutdown(socket.SHUT_RDWR)
    lost_rpc.sock.close()
    observed=controller.call('status',[key(lost)])['statuses'][0]
    if observed['deliveryState']=='unknown': controller.call('submit',lost)
    lost_status=settled(controller,lost,'output_observed')
    assert controller.call('submit',lost)['statuses'][0]['receipt']==lost_status['receipt']
    third=launch(durable,'s6-raised',True,20000); gen3=third['expected_generation']
    controller.close(); controller=Controller(durable,gen3)
    Model.empty=True
    held=envelope(gen3,'s6-held','y'*12000); controller.call('submit',held)
    held_status=settled(controller,held,'held'); assert held_status['processing']['reason']=='no_output'
    calls=Model.calls
    assert controller.call('submit',held)['statuses'][0]['processing']==held_status['processing']
    assert Model.calls==calls
    retry={'messageId':held['message']['id'],'semanticSha256':held['semanticSha256'],
        'expectedAttemptId':held_status['processing']['attemptId'],'retryId':'s6-explicit-retry'}
    Model.empty=False
    released=controller.call('retry',retry); assert released['disposition']=='released'
    final=settled(controller,held,'output_observed')
    assert controller.call('retry',retry)==released
    assert final['receipt']==held_status['receipt'] and Model.calls==calls+1
    controller.call('retry',{**retry,'expectedAttemptId':None},reject=True)
    assert sha(NATIVE/'config.toml')==shared_sha
    # Independent raw history oracle: exactly one Commit+canonical item per ID.
    paths=list((NATIVE/'sessions').glob(f'**/*{thread}.jsonl')); assert len(paths)==1
    history=[json.loads(line) for line in paths[0].read_text().splitlines()]
    (RUN/'history-types.json').write_text(json.dumps([r['type'] for r in history]))
    for event in [e,lost,held]:
        items=[r for r in history if r['type']=='response_item' and r['payload'].get('id')==event['message']['id']]
        assert len(items)==1, (event['message']['id'],len(items))
        item=items[0]['payload']; assert item['name']=='external_event' and item['namespace']=='external' and item.get('call_id') is None
        body=json.loads(item['output']); assert body=={'source':event['message']['source'],'type':event['message']['type'],'text':event['message']['text']}
    for request in Model.requests:
        assert not any(t.get('name','').startswith('thread/externalInput') for t in request.get('tools',[]))
    (RUN/'PASS.json').write_text(json.dumps({'durable':durable,'thread':thread,'generations':[gen,gen2,gen3],
        'receipt':original_receipt,'model_requests':Model.calls,'policies':['default10000','off',20000],
        'pair_ids':[e['message']['id'],lost['message']['id'],held['message']['id']],
        'stock':'registration-only rejection','bus':'registration only, no delivery integration',
        'native_sha256':sha(PATCHED),'schema_sha256':sha(SCHEMA),'facade_sha256':sha(MCP)},indent=2))
finally:
    (RUN/'facts.json').write_text(json.dumps(facts,indent=2))
    (RUN/'model-requests.json').write_text(json.dumps(Model.requests,indent=2))
    for c in controllers:
        try: c.close()
        except Exception: pass
    if durable:
        for ident in ids:
            r=store()['sessions'].get(ident,{})
            if r.get('app_server_runtime'): stock_pids.append(r['app_server_runtime']['pid'])
    for pid in set(stock_pids):
        try:
            if os.getpgid(pid)==pid and Path(f'/proc/{pid}/exe').readlink() in [PATCHED,STOCK] and Path(f'/proc/{pid}/cwd').readlink()==RUN:
                os.killpg(pid,signal.SIGKILL)
        except (ProcessLookupError,FileNotFoundError): pass
    for child in reversed(children): stop_owned(child)
    for log in logs: log.close()
    model.shutdown()
