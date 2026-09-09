"""S6g default-byte Release workflow oracle. Private namespaces only.

Reuse accepted launch/cleanup builders, not their campaigns. Model scheduling
is thread-scoped; authentication remains actual Core metadata and Cutex state.
"""
import ast
from pathlib import Path

builder = Path(__file__).with_name('soon_cli_boundary.py').read_text()
ending = "exec(compile(ast.fix_missing_locations(tree), 'S6f-private-CLI', 'exec'), globals())"
assert ending in builder
exec(compile(builder.replace(ending, ''), 'S6g-builder', 'exec'), globals())
probe = next(n for n in tree.body if isinstance(n, ast.Try))
cut = next(i for i, n in enumerate(probe.body) if isinstance(n, ast.Assign)
           and any(isinstance(t, ast.Name) and t.id == 'stock' for t in n.targets))
setup = ast.unparse(ast.Module(body=probe.body[:cut], type_ignores=[]))
for old, new in [('range(2)', 'range(4)'),
                 ('durable, legacy = ids', 'durable, legacy, release_id, offline_id = ids'),
                 ('[PATCHED, PATCHED]', '[PATCHED, PATCHED, PATCHED, PATCHED]')]:
    assert old in setup, old
    setup = setup.replace(old, new)
model = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == 'Model')
model_text = ast.unparse(model)
assert 'if Model.pending:' in model_text
model_text = model_text.replace('if Model.pending:',
    "if Model.pending and data.get('prompt_cache_key') == Model.target:")
# A unique new-turn reply prevents old resumed history satisfying the PTY
# assertion before the genuine Human turn has actually reached Responses.
model_text = model_text.replace("'Private S5a fixture complete'", "getattr(Model, 'reply', 'Private S5a fixture complete')")
tree.body[tree.body.index(model)] = ast.parse(model_text).body[0]
campaign = '''
stock=launch(legacy,'s6g-worker')
current=launch(durable,'s6g-director')
reviewer=launch(release_id,'s6g-release')
actors=[current,stock,reviewer]
rpcs=[native_rpc(actor)[0] for actor in actors]
def mutation(action_id,kind,revision,**fields):
    return {'schema':'cutex/human-management-project-mutation/v1','action_id':action_id,'project_id':'s6g-project','expected_authority_epoch':0 if kind=='create' else 1,'expected_project_revision':revision,'operation':{'kind':kind,**fields}}
create=mutation('s6g-project','create',0,director_cutex_session_id=durable,presentation={'display_name':'Private S6g','badge_label':'SG','color':'cyan'})
for n,ident in enumerate(ids):
    code,candidates=api(mp,'/v2/agent-management/durable-candidates');assert code==200
    candidate=next(c for c in candidates if c['cutex_session_id']==ident)
    code,result=api(mp,'/v2/agent-management/durable-import',{'action_id':f's6g-import-{n}','candidate':candidate,'confirmed_formal_name':f'Private S6 Agent {n}','assignment':create if n==0 else None,'detach':None})
    assert code==200 and result['complete'],(code,result)
for n,ident in enumerate(ids[1:]):
    code,result=api(mp,'/v2/agent-management/project-mutations',mutation(f's6g-member-{n}','add_member',n+1,cutex_session_id=ident));assert code==200,(code,result)
seat_token=hashlib.sha256(b'cutex/task-service-seat-management/v1\\0'+HUMAN_TOKEN.encode()).hexdigest()
def bind(ident,action_id):
    code,result=api(mp,'/v2/task-service/seats/bind',{'schema':'cutex/seat-occupancy-command/v1','action_id':action_id,'seat_id':'cutex-release','occupant_cutex_session':ident},token=seat_token)
    assert code==200,(code,result)
bind(release_id,'s6g-release-seat')
def idle(n):
    deadline=time.monotonic()+90
    while True:
        state=rpcs[n].call('thread/read',{'threadId':threads[n],'includeTurns':False})['thread']['status']
        if state['type']=='idle':return
        assert time.monotonic()<deadline,state
        threading.Event().wait(.02)
def mcp(n,name,args):
    idle(n)
    Model.target=threads[n];Model.output=None
    Model.pending=(f's6g-call-{len(facts)}',name,args)
    rpcs[n].call('turn/start',{'threadId':threads[n],'input':[{'type':'text','text':'Execute the fixed private Task protocol fixture','text_elements':[]}]})
    deadline=time.monotonic()+90
    while Model.output is None:
        assert time.monotonic()<deadline,(name,args)
        threading.Event().wait(.02)
    output=Model.output
    if isinstance(output,str):output=json.loads(output)
    if isinstance(output,list):
        values=[json.loads(c['text']) for c in output if c.get('type')=='input_text' and c.get('text','').startswith('{')]
        assert len(values)==1,output
        output=values[0]
    if 'content' in output:output=json.loads(next(c['text'] for c in output['content'] if c.get('type')=='text'))
    assert BUS_TOKEN not in json.dumps(output) and HUMAN_TOKEN not in json.dumps(output)
    Model.pending=None;idle(n)
    facts.append({'actor':n,'tool':name,'arguments':args,'result':output})
    return output
def d(op,aid,**kw):
    return mcp(0,'cutex_task_service_director',{'operation':op,'action_id':aid,**kw})
def w(op,aid,assignment,**kw):
    return mcp(1,'cutex_task_service',{'operation':op,'action_id':aid,'assignment_id':assignment,**kw})
def family(prefix,count):
    deadline=time.monotonic()+120
    while True:
        try: found=[(k,v['snapshot']) for k,v in ledger()['messages'].items() if k.startswith(prefix)]
        except FileNotFoundError:found=[]
        if len(found)>=count and all(v['state']=='delivered' for _,v in found):return found
        assert time.monotonic()<deadline,found
        threading.Event().wait(.02)
def snapshot(n=2):
    value=call(actors[n],threads[n],'/api/task/v2/query',{'schema':'cutex/task-service-query/v2','query':{'operation':'snapshot'}})
    # Fixed public query envelope, never provider-file mutation.
    return value
def terminal(op,aid,assignment):
    observed=snapshot()
    (RUN/'query-shape.json').write_text(json.dumps(observed))
    state=observed['outcome']['body']
    row=state['assignments'][assignment]
    attempt=list(state['attempts'][assignment].values())[-1]
    request={'schema':'cutex/task-service-terminal/v2','command':{'operation':op,'body':{'schema':'cutex/task-service-action/v2','action_id':aid,'assignment_id':assignment,'decision_reference':'Private explicit Release decision'}},'context':{'expected_assignment_revision':row['local_revision'],'attempt':{'attempt_number':attempt['attempt_number'],'attempt_token':attempt['attempt_token'],'expected_attempt_revision':attempt['local_revision']}}}
    return request,call(reviewer,threads[2],'/api/task/v2/terminal',request)
for case in ['accept','fail']:
    tid='t-s6g-'+case;assignment='a-s6g-'+case
    assert d('create_revision','s6g-create-'+case,project_id='s6g-project',workflow_id='wf-'+case,task_id=tid,task_revision=1,opaque_contract='Private Release review: inspect the bounded result and decide explicitly.',completion_policy='release_review',completion_authority_cutex_session_id=release_id)['status']=='committed'
    assert d('assign','s6g-assign-'+case,project_id='s6g-project',task_id=tid,task_revision=1,assignment_id=assignment,assignee_cutex_session_id=legacy,summary='Private Release workflow')['status']=='committed'
    family('tsa_',1 if case=='accept' else 2)
    assert w('start','s6g-start-'+case,assignment)['status']=='committed'
    assert w('submit','s6g-submit-'+case,assignment,result_sha256='a'*64,result_reference='private:bounded-result')['status']=='committed'
    notifications=family('tsc_',1 if case=='accept' else 3)
    before=json.loads(json.dumps(notifications))
    if case=='accept':
        # Release is deliberately not the project Director. The Director-only
        # semantic endpoint must retain its original authorization refusal.
        denied=call(reviewer,threads[2],'/api/task/v2/director-action',{'schema':'cutex/task-service-director-action/v2','operation':'accept_result','action_id':'s6g-wrong-endpoint','assignment_id':assignment})
        assert denied['code']=='project_authority_absent',denied
        original=snapshot()
        bind(legacy,'s6g-change-release')
        denied=call(reviewer,threads[2],'/api/task/v2/terminal',{'schema':'cutex/task-service-terminal/v2','command':{'operation':'accept_result','body':{'schema':'cutex/task-service-action/v2','action_id':'s6g-old-release','assignment_id':assignment}},'context':{'expected_assignment_revision':original['outcome']['body']['assignments'][assignment]['local_revision'],'attempt':None}})
        assert denied['outcome']['kind']=='no_write',denied
        bind(release_id,'s6g-restore-release')
        assert snapshot()['outcome']['body']==original['outcome']['body']
    request,result=terminal('accept_result' if case=='accept' else 'fail_result','s6g-decision-'+case,assignment)
    assert result['outcome']['kind']=='committed',result
    assert call(reviewer,threads[2],'/api/task/v2/terminal',request)==result
    notifications=family('tsc_',2 if case=='accept' else 4)
    for key,value in before:assert ledger()['messages'][key]['snapshot']==value
    if case=='fail':
        assert d('cancel','s6g-cancel',assignment_id=assignment)['status']=='committed'
        family('tsc_',5)
assert d('create_revision','s6g-create-exhausted',project_id='s6g-project',workflow_id='wf-exhausted',task_id='t-s6g-exhausted',task_revision=1,opaque_contract='Private offline delivery exhaustion fixture.',completion_policy='director_acceptance')['status']=='committed'
failed=d('assign','s6g-assign-exhausted',project_id='s6g-project',task_id='t-s6g-exhausted',task_revision=1,assignment_id='a-s6g-exhausted',assignee_cutex_session_id=offline_id,summary='Private unavailable recipient')
assert failed['status']!='committed',failed
helper=Path(sys.argv[3]).resolve()
assert helper.is_file() and helper.is_relative_to(ROOT/'target/debug/deps')
controller=subprocess.run([str(helper),'task_service::provider::tests::s6g_private_retries_exhausted_controller','--exact','--ignored','--nocapture'],env=env,cwd=RUN,capture_output=True,timeout=30)
(RUN/'exhaustion-controller.log').write_bytes(controller.stdout+controller.stderr)
assert controller.returncode==0,(controller.stdout,controller.stderr)
assert b'1 passed' in controller.stdout
# The test-only out-of-process system controller has no production producer.
# The real terminal HTTP handler explicitly requests a drain on Committed,
# including exact replay. This is a named scheduling edge, not an incidental
# roundtrip, new transition, sleep, or provider-state persistence barrier.
assert call(reviewer,threads[2],'/api/task/v2/terminal',request)==result
family('tsc_',6)
from types import SimpleNamespace
h=SimpleNamespace(RUN=RUN,ENV=env)
idle(1)
Model.reply='Private S6g default Human reply observed'
terminal_ui=Terminal('s6g-default-attach',None,legacy)
try:
    terminal_ui.wait(b'unknown-private-model')
    terminal_ui.prompt(b'Private default bundle Human input')
    terminal_ui.wait(b'Private S6g default Human reply observed')
    terminal_ui.close()
finally:
    if terminal_ui.child.poll() is None:terminal_ui.close(check=False)
record=next(r for r in store()['sessions'].values() if r['cutex_session_id']==legacy)
assert record['runtime_generation']==stock['expected_generation']
assert process_identity(stock['binding']['pid']) is not None
assert sha(NATIVE/'config.toml')==shared_sha
(RUN/'PASS-TASK.json').write_text(json.dumps({'durable':ids,'native':threads,'facts':facts,'messages':ledger()['messages'],'default_cutex_sha256':sha(CUTEX),'default_facade_sha256':sha(MCP)},indent=2))
'''
frozen = "CUTEX=ROOT/'artifacts/s6g-default/cutex'\nMCP=ROOT/'artifacts/s6g-default/cutex-mcp'\n"
probe.body = ast.parse(frozen + setup + '\n' + campaign).body
exec(compile(ast.fix_missing_locations(tree), 'S6g-default-Task', 'exec'), globals())
