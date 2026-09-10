"""One authorized corrected Job turn plus at most one natural completion turn.
No response fabrication, forced direct exposure, or Core meta injection.
The immutable prior fixture supplies private stores/owned process cleanup only.
"""
import ast,hashlib,json,os,queue,sys,time,threading
from pathlib import Path

MODEL='gpt-5.6-terra'
MODE='real'
ROOT=Path(__file__).resolve().parents[1]
RUN=ROOT/sys.argv[1]
assert RUN.parent==ROOT and not RUN.exists()
assert len(os.fsencode(RUN/'h/.cutex/runtime/app-server/000000000000/s'))<=100
staged_auth=ROOT/'staged-auth.json'
assert staged_auth.is_file() and not staged_auth.is_symlink()
observations={'boundary':'actual aemeath provider, native Core and Job; private setup/observer','model':MODEL,'effort':'low','usage':[],'toolCalls':[],'messages':[]}

# Only unchanged helpers, not their campaigns. Both source files are preserved.
helpers=ast.parse(Path(__file__).with_name('reviewed-job-v2.py').read_text())
nodes=[n for n in helpers.body if isinstance(n,ast.FunctionDef) and n.name in ('record_digest','prepared_launch')]
assert len(nodes)==2
exec(compile(ast.Module(body=nodes,type_ignores=[]),'reviewed-Job-helpers','exec'),globals())

def create_subject(g):
    project='real-aemeath-private'
    mutation={'schema':'cutex/human-management-project-mutation/v1','action_id':'real-project','project_id':project,'expected_authority_epoch':0,'expected_project_revision':0,'operation':{'kind':'create','director_cutex_session_id':g['durable'],'presentation':{'display_name':'Private real smoke','badge_label':'R','color':'cyan'}}}
    code,candidates=g['api'](g['mp'],'/v2/agent-management/durable-candidates');assert code==200
    candidate=next(c for c in candidates if c['cutex_session_id']==g['durable'])
    code,result=g['api'](g['mp'],'/v2/agent-management/durable-import',{'action_id':'real-import','candidate':candidate,'confirmed_formal_name':'Private S6 Agent 0','assignment':mutation,'detach':None})
    assert code==200 and result['complete']
    cwd=g['RUN']/'new-agent';cwd.mkdir(mode=0o700)
    (cwd/'probe-readable').write_text('private-read-success\n')
    spec={'name':'Private aemeath smoke subject','cwd':str(cwd),'profile':'aemeath','runtime_backend':'host','model':MODEL,'reasoning':'low','permissions':'read-only','approval_policy':'on-request','sandbox_mode':'read-only','groups':['private-real-smoke'],'expose_to_im':False,'pin':False}
    request={'schema':'cutex/agent-management/v1','action_id':'real-create','bootstrap_intent':'real-create','project_id':project,'operation':'create','spec':spec,'start_mode':'bootstrap_only','frozen_message':None}
    review=g['action']({'operation':'review_bootstrap','request':request,'native_home':str(g['NATIVE']),'bundle_manifest':str(g['RUN']/'bundle-0.json'),'bundle_sha256':g['sha'](g['RUN']/'bundle-0.json'),'expires_at_unix':int(time.time())+1800,'job_mcp':g['job_descriptor']})
    g['action']({'operation':'authorize_bootstrap','review':review})
    before=set(g['store']()['sessions'])
    result=g['call'](g['current'],g['thread'],'/api/agent-management/v1/actions',request)
    (g['RUN']/'real-create-result.json').write_text(json.dumps(result,indent=2))
    assert result['outcome']['status']=='complete','reviewed create did not complete; no retry'
    new=set(g['store']()['sessions'])-before;assert len(new)==1
    ident=new.pop();record=g['store']()['sessions'][ident]
    receipt=next(v['receipt'] for v in g['store']()['explicit_launch_receipts'].values() if v['kind']=='runtime' and v['receipt']['review']['subject']['cutex_session_id']==ident)
    assert receipt['stage']=='ready' and receipt['review']['configuration']['aemeath_auth']
    g['stock_pids'].append(receipt['binding']['pid'])
    g.update(current=receipt,thread=record['codex_session_id'],durable=ident,workdir=cwd)
    observations['reviewedManagementCreate']=True

def run_job(g):
    assert g['Model'].calls==0,'unexpected fake-provider request'
    # Reuse prior real Management-create evidence; this continuation needs
    # only the fixture's already reviewed ordinary existing-thread runtime.
    g['workdir']=g['RUN']
    rpc,_=g['native_rpc'](g['current'])
    resumed=rpc.call('thread/resume',{'threadId':g['thread']})
    assert resumed['model']==MODEL,resumed.get('model')
    assert g['current']['review']['configuration']['reasoning']=='low'
    assert g['current']['review']['configuration']['aemeath_auth']['version']==1
    assert rpc.call('thread/read',{'threadId':g['thread'],'includeTurns':True})['thread']['turns']==[]
    observations['neutralModelTurns']=0
    servers=[];cursor=None;seen=set()
    for _ in range(20):
        params={'threadId':g['thread'],'detail':'toolsAndAuthOnly','limit':1}
        if cursor is not None:params['cursor']=cursor
        page=rpc.call('mcpServerStatus/list',params)
        servers.extend(s for s in page['data'] if s['name']=='cutex_job')
        cursor=page.get('nextCursor')
        if cursor is None:break
        assert cursor not in seen,'repeated inventory cursor'
        seen.add(cursor)
    else:raise RuntimeError('inventory pagination bound exceeded')
    assert len(servers)==1,'Job server missing or ambiguous'
    server=servers[0];tools=server.get('tools') or {}
    projected=[]
    for key,tool in tools.items():
        schema=tool.get('inputSchema') or tool.get('input_schema')
        projected.append({'key':key,'name':tool.get('name'),'inputSchema':schema,
            'schemaSha256':hashlib.sha256(json.dumps(schema,sort_keys=True).encode()).hexdigest()})
    info=server.get('serverInfo') or {}
    observations['jobInventory']={'runtimeStatus':server.get('runtimeStatus'),'authStatus':server.get('authStatus'),
        'serverInfo':{k:info.get(k) for k in ('name','version')},'tools':projected,'count':len(projected)}
    observations['projectionBoundary']={'resolvedModel':resumed['model'],'reviewedEffort':'low',
        'staticNativeToolMode':'code_mode_only','staticSupportsSearchTool':True,
        'effectiveToolMode':'not exposed by this diagnostic','providerNamespaceTools':'not observed',
        'finalSerializedRequest':'not captured; no TLS proxy/native patch',
        'codeModeCallableRule':'tools.mcp__cutex_job__<observed tool name>'}
    (g['RUN']/'real-observations.json').write_text(json.dumps(observations,indent=2))
    assert server.get('runtimeStatus')=='connected','Job runtime not healthy; no model turn'
    submit=next((t for t in projected if t['name']=='submit'),None)
    assert submit and submit['inputSchema'] and submit['inputSchema'].get('properties'),'submit schema absent; no model turn'
    assert any(t['name']=='read_output' and t['inputSchema'] for t in projected),'output schema absent'
    expected={'actionId':'real-aemeath-job','argv':['/bin/sh','-c','cat probe-readable; printf real-job-output'],'cwd':str(g['workdir'])}
    approvals=0;pending=list(rpc.events);rpc.events.clear();turns=set()
    def save():
        (g['RUN']/'real-observations.json').write_text(json.dumps(observations,indent=2))
    def consume(msg):
        nonlocal approvals
        method=msg.get('method');p=msg.get('params',{})
        if method=='turn/started':
            turns.add(p['turn']['id']);observations['observedTurnCount']=len(turns)
            assert len(turns)<=2,'authorized turn bound exceeded'
        if method=='thread/tokenUsage/updated':
            usage=p.get('tokenUsage',{})
            if not observations['usage'] or observations['usage'][-1]!=usage: observations['usage'].append(usage)
            if len(observations['usage'])>6:raise RuntimeError('bounded generation usage event limit reached')
        if method in ('error','turn/failed'):
            observations['providerError']={'method':method,'code':p.get('codexErrorInfo')}
            raise RuntimeError('native/provider error; no retry')
        if method=='item/completed':
            item=p.get('item',{})
            if item.get('type')=='agentMessage': observations['messages'].append(item.get('text',''))
            if item.get('type')=='mcpToolCall':
                observations['toolCalls'].append({k:item.get(k) for k in ['server','tool','status']})
                assert item.get('server')=='cutex_job' and item.get('tool') in ('submit','read_output'),'unexpected tool; stopping'
        if method=='turn/completed' and p.get('turn',{}).get('error'):
            observations['providerError']={'method':method,'code':p['turn']['error'].get('codexErrorInfo')}
            raise RuntimeError('native turn failed; no retry')
        if 'id' in msg and method:
            accepted=False
            if method=='mcpServer/elicitation/request':
                meta=p.get('_meta') or {}
                accepted=p.get('serverName')=='cutex_job' and meta.get('tool_params')==expected and approvals==0
                rpc.send(json.dumps({'id':msg['id'],'result':{'action':'accept' if accepted else 'decline','content':{},'_meta':None}}).encode())
                if accepted:approvals+=1
            if not accepted:raise RuntimeError('unapproved native tool request; stopping')
        save()
    def until_turn(ident):
        deadline=time.monotonic()+120
        while time.monotonic()<deadline:
            msg=pending.pop(0) if pending else rpc.messages.get(timeout=max(.01,deadline-time.monotonic()))
            consume(msg)
            if msg.get('method')=='turn/completed' and msg['params']['turn']['id']==ident:return
        raise RuntimeError('native turn deadline; no retry')
    prompt='Private Human-approved single Job smoke. Native runtime discovery confirms connected MCP server cutex_job with tools submit and read_output. Use tool_search if necessary to discover ONLY these Job tools. CodeMode exec is permitted ONLY as the wrapper to call the discovered Job MCP functions, whose native callable names follow tools.mcp__cutex_job__submit and tools.mcp__cutex_job__read_output. This permission does NOT allow shell/exec_command, filesystem APIs or any other business tools. Submit EXACTLY ONCE with arguments '+json.dumps(expected)+'. Follow normal approval. Do not automatically retry submit or poll. After submit returns, end with Submitted. When the normal Job completion notification arrives, use read_output once for its stdout and briefly acknowledge the exact output. No credentials/configuration/other files may be read. The shell argv is ONLY a Job argument authorized for the private Job daemon; never execute it directly.'
    second=rpc.call('turn/start',{'threadId':g['thread'],'input':[{'type':'text','text':prompt}]})
    pending.extend(rpc.events);rpc.events.clear()
    until_turn(second['turn']['id'])
    if not observations['toolCalls']:raise RuntimeError('no Job tool invocation; no paid retry')
    deadline=time.monotonic()+120
    while time.monotonic()<deadline:
        state_path=g['RUN']/'job-state/state.json'
        state=json.loads(state_path.read_text()) if state_path.exists() else {'jobs':{}}
        if state['jobs'] and all(j.get('completionDelivery',{}).get('state')=='delivered' for j in state['jobs'].values()) and 'real-job-output' in '\n'.join(observations['messages']):break
        consume(pending.pop(0) if pending else rpc.messages.get(timeout=max(.01,deadline-time.monotonic())))
    else:raise RuntimeError('Job/context/model acknowledgment deadline; no retry')
    assert len(state['jobs'])==1
    jid,job=next(iter(state['jobs'].items()))
    assert job['state']=='exited' and job['exitCode']==0
    assert job['request']['origin']['permissionProfileType']=='managed'
    assert job['request']['origin']['runtimeAgentId']==g['current']['runtime_agent_id']
    assert job['request']['origin']['nativeThreadId']==g['thread']
    matches=[v['snapshot'] for v in g['ledger']()['messages'].values() if jid in json.dumps(v) and v['snapshot']['state']=='delivered']
    assert len(matches)==1 and matches[0].get('externalInputReceipt')
    observations.update({'job':job,'receipt':matches[0]['externalInputReceipt'],'approvedExactSubmit':approvals==1,'nativeThread':g['thread'],'durable':g['durable'],'modelReplyVerified':True})
    assert g['Model'].calls==0,'unexpected fake-provider request'
    save();return observations

base=Path(__file__).with_name('base-fixture.py').read_text()
assert hashlib.sha256(base.encode()).hexdigest()=='ed1ebdb57d413db2f3f382aa65c1bf014e4ccb8b8483b63dd85e774717d9728e'
base=base.replace("CUTEX = FROZEN/'package/artifacts/linux/cutex'","CUTEX = ROOT/'bin-v2/cutex'")
base=base.replace("MCP = FROZEN/'package/artifacts/linux/cutex-mcp'","MCP = ROOT/'bin-v2/cutex-mcp'")
base=base.replace('profile_id = str(uuid.uuid4())',"profile_id = 'cd6a39eb-3997-45c6-9824-5113fe36a4b8'")
base=base.replace("'alpha'","'aemeath'")
base=base.replace('unknown-private-model',MODEL)
base=base.replace("'--permission', 'full-access', '--sandbox', 'danger-full-access'","'--permission', 'read-only', '--sandbox', 'read-only'")
base=base.replace("'sandbox': 'danger-full-access'","'sandbox': 'read-only'")
base=base.replace("env=env,cwd=RUN,\n                       stdout=log", "env=({k:v for k,v in env.items() if k not in ('LD_PRELOAD','S4_TEST_ALLOWED_PORTS')} if str(args[0])==str(PATCHED) else env),cwd=RUN,\n                       stdout=log")
lines=base.splitlines()
for i,line in enumerate(lines):
    if "(folder / 'config.toml').write_text" in line:
        lines[i]="    (folder / 'config.toml').write_text(\"cutex_provider_mode='aemeath_chatgpt_v1'\\nmodel='gpt-5.6-terra'\\nmodel_provider='openai'\\nmodel_reasoning_effort='low'\\n\")"
    if line.startswith('    args = [PATCHED,'):
        lines[i]="    args = [PATCHED,'-c','model=\"gpt-5.6-terra\"','-c','model_reasoning_effort=\"low\"','-c','cli_auth_credentials_store=\"file\"','-c','default_permissions=\":read-only\"','--disable-plugin-startup-tasks-for-tests','--listen','unix://'+str(sock)]"
    if line=='    sock = RUN / \'bootstrap.sock\'':
        lines[i]="    NATIVE.chmod(0o700)\n    import shutil\n    shutil.copyfile(staged_auth,NATIVE/'auth.json')\n    (NATIVE/'auth.json').chmod(0o600)\n    staged_auth.unlink()\n"+line
base='\n'.join(lines)+'\n'
base=base.replace("    current=launch(durable,'vm-job-subscriber')\n    from job_mcp_helper_r2 import run_job","    current=prepared_launch(globals())")
base=base.replace('actual Job daemon/process/completion; actual separate Job MCP adapter issuer, harness-driven trusted metadata','actual aemeath provider and Core-configured Job MCP; observer only, no fake responses or metadata injection')
context=dict(globals())
try:
    exec(compile(base,str(Path(__file__).with_name('base-fixture.py')),'exec'),context)
except Exception as e:
    observations['failureType']=type(e).__name__
    import traceback
    observations['failureFrames']=[{'file':Path(f.filename).name,'line':f.lineno,'function':f.name} for f in traceback.extract_tb(e.__traceback__)]
    if RUN.exists():(RUN/'real-observations.json').write_text(json.dumps(observations,indent=2))
    print(json.dumps({'failed':type(e).__name__,'providerError':observations.get('providerError'),'noRetry':True}))
finally:
    for credential in [staged_auth,RUN/'h/.cutex/codex-home/auth.json']:
        if credential.exists():credential.unlink()
    if RUN.exists():
        (RUN/'credential-cleanup.json').write_text(json.dumps({'exactGuestAuthRemoved':True,'hostAuthUntouched':True}))
