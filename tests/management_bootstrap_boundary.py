"""S8a real private root intent -> Director create -> neutral native owner.
Reuses only accepted namespace/setup/cleanup builders, not prior campaigns.
"""
import ast
from pathlib import Path
S8_SOURCE = Path(__file__).read_text()

builder = Path(__file__).with_name('soon_task_boundary.py').read_text()
ending = "exec(compile(ast.fix_missing_locations(tree),'S6f-private-Task','exec'),globals())"
assert ending in builder
exec(compile(builder.replace(ending, ''), 'S8a-private-builder', 'exec'), globals())
terminal_source = Path('/mnt/mambo/PersonaProjects/cutex-light-core-r1/artifacts/s7a/terminal_probe_error.py').read_text()
terminal = next(n for n in ast.parse(terminal_source).body if isinstance(n, ast.ClassDef) and n.name == 'Terminal')
init = next(n for n in terminal.body if isinstance(n, ast.FunctionDef) and n.name == '__init__')
args = next(n for n in init.body if isinstance(n, ast.Assign) and any(isinstance(t, ast.Name) and t.id == 'args' for t in n.targets))
args.value = ast.parse("[str(CUTEX), 'session', 'stock-attach', thread]", mode='eval').body
tree.body.insert(tree.body.index(probe), terminal)
fault_setup = ast.parse("""
fault_mode=sys.argv[3] if len(sys.argv)>3 else None
assert fault_mode in (None,'pre-id','post-id')
if fault_mode:
    env['CUTEX_BOOTSTRAP_TEST_'+('PRE' if fault_mode=='pre-id' else 'POST')+'_ID_ACTION']='s8a-create'
""").body
bus_index = next(i for i,n in enumerate(setup) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='bus' for t in n.targets))
setup[bus_index:bus_index] = fault_setup
setup[:0] = ast.parse("""
if os.environ.get('S8_PRIVATE_BIN_DIR'):
    binaries=Path(os.environ['S8_PRIVATE_BIN_DIR']).resolve()
    assert binaries.is_relative_to(ROOT/'artifacts')
    CUTEX=binaries/'cutex';MCP=binaries/'cutex-mcp'
    assert CUTEX.is_file() and MCP.is_file()
""").body
# The private receiver cwd is reviewed/trusted in the original shared config,
# before its manifest is pinned. Do not let a later CLI trust prompt rewrite
# the shared config or pretend client approval changes receiver permissions.
for node in setup:
    if isinstance(node,ast.Expr) and isinstance(node.value,ast.Call) and 'NATIVE' in ast.unparse(node) and 'trust_level' in ast.unparse(node):
        value=node.value.args[0]
        suffix=ast.parse("'\\n[projects.'+json.dumps(str(RUN/'new-agent'))+']\\ntrust_level=\"trusted\"\\n'",mode='eval').body
        node.value.args[0]=ast.BinOp(left=value,op=ast.Add(),right=suffix)
campaign = '''
current=launch(durable,'s8a-director')
(RUN/'probe-source.py').write_text(S8_SOURCE)
(RUN/'byte-manifest.json').write_text(json.dumps({'cutex':sha(CUTEX),'facade':sha(MCP),'native':sha(PATCHED),'schema':sha(SCHEMA),'cutex_path':str(CUTEX),'facade_path':str(MCP)}))
project='s8a-project'
create={'schema':'cutex/human-management-project-mutation/v1','action_id':'s8a-project','project_id':project,'expected_authority_epoch':0,'expected_project_revision':0,'operation':{'kind':'create','director_cutex_session_id':durable,'presentation':{'display_name':'Private S8a','badge_label':'S8','color':'cyan'}}}
code,candidates=api(mp,'/v2/agent-management/durable-candidates');assert code==200
candidate=next(c for c in candidates if c['cutex_session_id']==durable)
code,result=api(mp,'/v2/agent-management/durable-import',{'action_id':'s8a-import-director','candidate':candidate,'confirmed_formal_name':'Private S6 Agent 0','assignment':create,'detach':None})
assert code==200 and result['complete'],(code,result)
spec={'name':'Explicit S8a Formal Agent','cwd':str(RUN/'new-agent'),'profile':'alpha','runtime_backend':'host','model':'unknown-private-model','reasoning':'low','permissions':'read-only','approval_policy':'on-request','sandbox_mode':'read-only','groups':['private-s8a'],'expose_to_im':False,'pin':False}
request={'schema':'cutex/agent-management/v1','action_id':'s8a-create','bootstrap_intent':'s8a-create','project_id':project,'operation':'create','spec':spec,'start_mode':'bootstrap_only','frozen_message':None}
review_request={'operation':'review_bootstrap','request':request,'native_home':str(NATIVE),'bundle_manifest':str(RUN/'bundle-0.json'),'bundle_sha256':sha(RUN/'bundle-0.json'),'expires_at_unix':int(time.time())+600}
before=store()
def manage(body):return call(current,threads[0],'/api/agent-management/v1/actions',body)
def unchanged_creation():
    now=store()['sessions']
    return set(now)==set(before['sessions']) and not (RUN/'new-agent').exists() and all(
        [now[k].get(f) for f in ['cutex_session_id','codex_session_id','formal_agent_name','profile','explicit_launch']]
        ==[v.get(f) for f in ['cutex_session_id','codex_session_id','formal_agent_name','profile','explicit_launch']]
        for k,v in before['sessions'].items())
assert api(mp,'/v2/agent-management/explicit-launch',review_request,token=BUS_TOKEN)[0]==401
assert manage(request)['outcome']['status']=='no_write'
assert manage({**request,'action_id':'s8a-wrong-action'})['outcome']['status']=='no_write'
review=action(review_request)
assert unchanged_creation() and Model.calls==0
wrong_project={**review,'request':{**request,'project_id':'s8a-foreign-project'}}
action({'operation':'authorize_bootstrap','review':wrong_project},ok=False)
wrong_config={**review,'configuration':{**review['configuration'],'model':'different-model'}}
action({'operation':'authorize_bootstrap','review':wrong_config},ok=False)
for field,value in [('version',99),('director',legacy),('expires_at_unix',1)]:
    action({'operation':'authorize_bootstrap','review':{**review,field:value}},ok=False)
authorized={'operation':'authorize_bootstrap','review':review}
receipt=action(authorized);assert action(authorized)==receipt
assert unchanged_creation() and Model.calls==0
wrong={**request,'spec':{**spec,'name':'Different formal name'}}
denied=manage(wrong)
assert unchanged_creation() and Model.calls==0,denied
if fault_mode:
    try:manage(request)
    except (http.client.RemoteDisconnected,ConnectionResetError,BrokenPipeError):pass
    else:raise AssertionError('owned creator did not crash at selected cutpoint')
    assert bus.wait(timeout=10)==86
    journal=json.loads((CONF/'runtime/agent-management/v1/agent-management-v1.json').read_text())
    staged=journal['actions']['s8a-create']
    (RUN/'crash-stage.json').write_text(json.dumps({'exit':86,'cutpoint':fault_mode,'journal':staged}))
    assert staged['known_successor_cutex_session'] is None
    assert (staged['known_native_session_id'] is None)==(fault_mode=='pre-id')
    native_files=list((NATIVE/'sessions').glob('**/*.jsonl'))
    for key in list(env):
        if key.startswith('CUTEX_BOOTSTRAP_TEST_'):del env[key]
    bus=owner([CUTEX,'agent','serve','--port',bp],'bus-recovered');ready(bp,bus)
    # Only read-only authentication readiness is retried; create is replayed
    # once with its original action, never replaced by another native start.
    deadline=time.monotonic()+90
    while True:
        code,value=api(bp,'/api/agent-management/v1/actions',{'schema':'cutex/agent-management/v1','action_id':'s8a-recovery-observe','project_id':project,'operation':'query_managed'},token=BUS_TOKEN,headers=hs(current,threads[0]))
        if code==200 and value.get('outcome',{}).get('status')=='complete':break
        assert time.monotonic()<deadline,(code,value)
        threading.Event().wait(.05)
    result=manage(request)
    assert len(list((NATIVE/'sessions').glob('**/*.jsonl')))==len(native_files)
    if fault_mode=='pre-id':
        assert result['outcome']['status']=='owner_action_required',result
        assert manage(request)==result and set(store()['sessions'])==set(before['sessions']) and Model.calls==0
        (RUN/'result.json').write_text(json.dumps({'cutpoint':fault_mode,'journal':staged,'result':result,'native_start_retry':False,'model_calls':0}))
        raise SystemExit(0)
    assert staged['known_native_session_id'] in [r.get('codex_session_id') for r in store()['sessions'].values()]
else:result=manage(request)
(RUN/'create-result.json').write_text(json.dumps(result))
assert result['outcome']['status']=='complete',result
assert manage(request)==result
assert Model.calls==0,'neutral bootstrap sampled a model'
sessions=store()['sessions'];new=[r for k,r in sessions.items() if k not in before['sessions']]
assert len(new)==1,new
record=new[0];ident=record['cutex_session_id'];native=record['codex_session_id']
assert record['formal_agent_name']==spec['name'] and record['profile']=='alpha'
assert record['explicit_launch']['native_id']==native and record['explicit_launch']['version']==2
runtime=next(v['receipt'] for v in store()['explicit_launch_receipts'].values() if v['kind']=='runtime' and v['receipt']['review']['subject']['cutex_session_id']==ident)
stock_pids.append(runtime['binding']['pid'])
assert runtime['stage']=='ready' and runtime['expected_generation']==record['runtime_generation']
peer,cap=native_rpc(runtime)
assert cap['externalInputVersion']==1 and 'soon' in cap['externalInputDeliveries']
history=peer.call('thread/read',{'threadId':native,'includeTurns':True})['thread']
assert history['turns']==[] and history['id']==native
rollouts=list((NATIVE/'sessions').glob('**/*'+native+'.jsonl'));assert len(rollouts)==1
meta=json.loads(rollouts[0].read_text().splitlines()[0])
assert meta['type']=='session_meta' and meta['payload']['id']==native
assert 'cutex-top-level-session' not in json.dumps(meta)
peer.call('thread/name/set',{'threadId':native,'name':'Native title is not a formal Agent name'})
assert store()['sessions'][ident]['formal_agent_name']==spec['name']
assert sha(NATIVE/'config.toml')==shared_sha
facts.append({'operation':'reviewed_create','durable':ident,'native':native,'generation':runtime['expected_generation'],'model_calls':Model.calls,'result':result})
from types import SimpleNamespace
h=SimpleNamespace(RUN=RUN,ENV=env)
terminal=Terminal('s8a-created-attach',None,ident)
try:
    terminal.wait(b'unknown-private-model')
    assert Model.calls==0
    terminal.prompt(b'Private Human input after neutral Management create')
    terminal.wait(b'Private external data observed')
    assert Model.calls==1
    # Output can precede the terminal's turn-completed notification. Use the
    # native explicit Exit command, not two Ctrl+C bytes racing completion.
    # This exits only the client; the bound app-server remains the same owner.
    terminal.prompt(b'/exit')
    terminal.child.wait(timeout=30)
finally:terminal.close()
assert store()['sessions'][ident]['runtime_generation']==runtime['expected_generation']
(RUN/'result.json').write_text(json.dumps({'facts':facts,'neutral_model_calls':0,'cli_model_calls':Model.calls,'source':'actual root/Director/provider/native/CLI path; private actors and fake Responses'}))
'''
probe.body = setup + ast.parse(campaign).body
# Every runtime in this private fixture store was created by this harness.
# Include the newly created Agent, including failures after registration but
# before its final Management receipt. Validate exact executable/cwd before kill.
cleanup = ast.unparse(ast.Module(body=probe.finalbody, type_ignores=[]))
cleanup = cleanup.replace('for ident in ids:', "for ident in store()['sessions']:")
cleanup = cleanup.replace("Path(f'/proc/{pid}/cwd').readlink() == RUN", "Path(f'/proc/{pid}/cwd').readlink() in [RUN, RUN/'new-agent']")
probe.finalbody = ast.parse(cleanup).body
tree.body[tree.body.index(probe):tree.body.index(probe)] = helpers
exec(compile(ast.fix_missing_locations(tree), 'S8a-private-bootstrap', 'exec'), globals())
