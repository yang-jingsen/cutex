"""S8b real private MCP/create/replace/rotation/Release, no live state.
Uses only the accepted S8a fixture builder and fixed fake Responses scheduler.
"""
import ast
from pathlib import Path

own_source=Path(__file__).read_text()
builder=Path(__file__).with_name('management_bootstrap_boundary.py').read_text()
ending="exec(compile(ast.fix_missing_locations(tree), 'S8a-private-bootstrap', 'exec'), globals())"
assert ending in builder
exec(compile(builder.replace(ending,''),'S8b-builder','exec'),globals())
prefix=campaign[:campaign.index('review_request=')]
model=next(n for n in ast.parse(Path(__file__).with_name('stock_task_mcp_boundary.py').read_text()).body if isinstance(n,ast.ClassDef) and n.name=='Model')
model_text=ast.unparse(model).replace('if Model.pending:',"if Model.pending and data.get('prompt_cache_key') == Model.target:")
model_text=model_text.replace("Model.calls += 1", "Model.requests.append(data)\n        Model.calls += 1")
model=ast.parse(model_text).body[0]
model.body[:0]=ast.parse('requests=[]\ntarget=None').body
tree.body[next(i for i,n in enumerate(tree.body) if isinstance(n,ast.ClassDef) and n.name=='Model')]=model
campaign=prefix+'''
(RUN/'probe-source.py').write_text(own_source)
reviewer=launch(legacy,'s8b-release')
actors={'director':{'receipt':current,'native':threads[0],'durable':durable},'release':{'receipt':reviewer,'native':threads[1],'durable':legacy}}
for actor in actors.values():actor['rpc']=native_rpc(actor['receipt'])[0]
code,candidates=api(mp,'/v2/agent-management/durable-candidates');assert code==200
candidate=next(c for c in candidates if c['cutex_session_id']==legacy)
code,value=api(mp,'/v2/agent-management/durable-import',{'action_id':'s8b-import-release','candidate':candidate,'confirmed_formal_name':'Private S6 Agent 1','assignment':None,'detach':None});assert code==200 and value['complete'],value
code,value=api(mp,'/v2/agent-management/project-mutations',{'schema':'cutex/human-management-project-mutation/v1','action_id':'s8b-release-member','project_id':project,'expected_authority_epoch':1,'expected_project_revision':1,'operation':{'kind':'add_member','cutex_session_id':legacy}});assert code==200,value
seat_token=hashlib.sha256(b'cutex/task-service-seat-management/v1\\0'+HUMAN_TOKEN.encode()).hexdigest()
def bind_release(ident,aid):
    code,value=api(mp,'/v2/task-service/seats/bind',{'schema':'cutex/seat-occupancy-command/v1','action_id':aid,'seat_id':'cutex-release','occupant_cutex_session':ident},token=seat_token)
    assert code==200,(code,value)
bind_release(legacy,'s8b-release-seat')
def idle(actor):
    deadline=time.monotonic()+120
    while True:
        value=actor['rpc'].call('thread/read',{'threadId':actor['native'],'includeTurns':False})['thread']['status']
        if value['type']=='idle':return
        assert time.monotonic()<deadline,value
        threading.Event().wait(.02)
def mcp(who,name,args):
    actor=actors[who];idle(actor)
    Model.target=actor['native'];Model.output=None
    Model.pending=(f's8b-call-{len(facts)}',name,args)
    actor['rpc'].call('turn/start',{'threadId':actor['native'],'input':[{'type':'text','text':'Execute the fixed private S8b protocol fixture','text_elements':[]}]})
    deadline=time.monotonic()+120
    while Model.output is None:
        assert time.monotonic()<deadline,(who,name,args)
        threading.Event().wait(.02)
    output=Model.output
    if isinstance(output,str):output=json.loads(output)
    if isinstance(output,list):
        values=[json.loads(c['text']) for c in output if c.get('type')=='input_text' and c.get('text','').startswith('{')]
        assert len(values)==1,output
        output=values[0]
    if 'content' in output:output=json.loads(next(c['text'] for c in output['content'] if c.get('type')=='text'))
    assert HUMAN_TOKEN not in json.dumps(output) and BUS_TOKEN not in json.dumps(output)
    Model.pending=None;idle(actor)
    facts.append({'actor':who,'tool':name,'arguments':args,'result':output})
    (RUN/'operations.json').write_text(json.dumps(facts))
    return output
def management(who,request):return mcp(who,'cutex_agent_management',{k:v for k,v in request.items() if k!='schema'})
def roster():return json.loads((CONF/'runtime/agent-management/v1/agent-management-v1.json').read_text())
def authorize(request):
    review=action({'operation':'review_bootstrap','request':request,'native_home':str(NATIVE),'bundle_manifest':str(RUN/'bundle-0.json'),'bundle_sha256':sha(RUN/'bundle-0.json'),'expires_at_unix':int(time.time())+3600})
    for patch in [{'version':99},{'request':{**request,'action_id':'wrong'}}]:
        action({'operation':'authorize_bootstrap','review':{**review,**patch}},ok=False)
    authorized={'operation':'authorize_bootstrap','review':review}
    assert action(authorized)==action(authorized)
    return review
def settle(request,value):
    # A bounded native-compatible MCP timeout is uncertainty, not another create.
    # Observe the ORIGINAL action and then replay it, never issue a new identity.
    if value['outcome'].get('code')=='response_uncertain':
        deadline=time.monotonic()+600
        while True:
            snapshot=roster()
            action_record=snapshot['actions'].get(request['action_id'])
            if action_record and action_record.get('response') is not None:break
            failure=snapshot['failure_events'].get('agent-management:'+request['action_id']+':failure')
            if failure:raise AssertionError(failure)
            assert time.monotonic()<deadline,action_record
            threading.Event().wait(.05)
        value=management('director',request)
    assert value['outcome']['status']=='complete',value
    assert management('director',request)==value
    (RUN/(request['action_id']+'-receipt.json')).write_text(json.dumps(value))
    result=value['outcome']['receipt']['result']
    row=result.get('agent') or result.get('successor')
    ident=row['cutex_session_id'];record=store()['sessions'][ident]
    receipt=next(v['receipt'] for v in store()['explicit_launch_receipts'].values() if v['kind']=='runtime' and v['receipt']['review']['subject']['cutex_session_id']==ident)
    assert receipt['stage']=='ready' and record['explicit_launch']['native_id']==row['native_session_id']
    assert record['formal_agent_name']==row['spec']['name']
    stock_pids.append(receipt['binding']['pid'])
    return {'receipt':receipt,'native':row['native_session_id'],'durable':ident,'rpc':native_rpc(receipt)[0]}
request['action_id']=request['bootstrap_intent']='s8b-create'
authorize(request)
denied=management('release',request);assert denied['outcome']['status']=='no_write',denied
actors['old-worker']=settle(request,management('director',request))
old_worker=actors['old-worker']['durable']
assert not any(r.get('prompt_cache_key')==actors['old-worker']['native'] for r in Model.requests),'neutral bootstrap sampled'
replacement={'schema':'cutex/agent-management/v1','action_id':'s8b-replace','bootstrap_intent':'s8b-replace','project_id':project,'operation':'replace','predecessor_cutex_session_id':old_worker,'policy':'close_after_ready','successor':{**spec,'name':'Explicit replacement','cwd':str(RUN/'replacement')},'start_mode':'bootstrap_only','frozen_message':None}
authorize(replacement)
changed={**replacement,'predecessor_cutex_session_id':durable}
assert management('director',changed)['outcome']['status']=='no_write'
actors['worker']=settle(replacement,management('director',replacement))
assert roster()['agents'][old_worker]['retired_at'] is not None
assert actors['worker']['durable']!=old_worker
rotation={'schema':'cutex/agent-management/v1','action_id':'s8b-rotate','bootstrap_intent':'s8b-rotate','project_id':project,'operation':'director_rotate','expected_predecessor_cutex_session':durable,'expected_authority_epoch':1,'mode':'retain_predecessor_with_message','successor':{**spec,'name':'Explicit successor Director','cwd':str(RUN/'successor')},'frozen_message':'Private successor instructions: inspect current Task Service state. No external actions.'}
authorize(rotation)
assert management('director',{**rotation,'expected_authority_epoch':9})['outcome']['status']=='no_write'
actors['successor']=settle(rotation,management('director',rotation))
successor=actors['successor']['durable']
authority=roster()['projects'][project]
assert authority['authorized_director_session']==successor and authority['authority_epoch']==2
assert roster()['agents'][durable]['retired_at'] is None
assert durable in roster()['operator_grants'][project]
# Actual Task snapshot exposes the current project-scoped completion authority.
def d(op,aid,**kw):return mcp('successor','cutex_task_service_director',{'operation':op,'action_id':aid,**kw})
def w(op,aid,**kw):return mcp('worker','cutex_task_service',{'operation':op,'action_id':aid,'assignment_id':'s8b-assignment',**kw})
def t(op,aid,**kw):return mcp('release','cutex_task_service_terminal',{'operation':op,'action_id':aid,'assignment_id':'s8b-assignment',**kw})
def delivered_matching(predicate,count=1):
    deadline=time.monotonic()+180
    while True:
        found=[v['snapshot'] for v in ledger()['messages'].values() if predicate(v['snapshot'])]
        if len(found)==count and all(s['state']=='delivered' for s in found):return found
        assert time.monotonic()<deadline,found
        threading.Event().wait(.02)
start=delivered_matching(lambda s:s.get('externalInput',{}).get('message',{}).get('type')=='management_start')[0]
assert start['externalInput']['message']['source']['kind']=='service'
assert rotation['frozen_message'] in start['externalInput']['message']['text']
(RUN/'management-start.json').write_text(json.dumps(start))
assert d('create_revision','s8b-task',project_id=project,workflow_id='s8b-workflow',task_id='s8b-task',task_revision=1,opaque_contract='Private bounded Release decision fixture.',completion_policy='release_review',completion_authority_cutex_session_id=legacy)['status']=='committed'
assert d('assign','s8b-assign',project_id=project,task_id='s8b-task',task_revision=1,assignment_id='s8b-assignment',assignee_cutex_session_id=actors['worker']['durable'],summary='Private work')['status']=='committed'
delivered_matching(lambda s:s['messageId'].startswith('tsa_'))
assert w('start','s8b-start')['status']=='committed'
assert w('submit','s8b-submit',result_sha256='a'*64,result_reference='private-result')['status']=='committed'
delivered_matching(lambda s:s['messageId'].startswith('tsc_'))
assert t('request_changes','s8b-changes',decision_reference='Private explicit repair')['status']=='committed'
delivered_matching(lambda s:s['messageId'].startswith('tsf_'))
assert w('submit','s8b-resubmit',result_sha256='b'*64,result_reference='private-repair')['status']=='committed'
delivered_matching(lambda s:s['messageId'].startswith('tsc_'),2)
# Seat moves through the private root API; real old Core caller must be denied.
bind_release(durable,'s8b-release-away')
assert t('accept_result','s8b-stale-seat')['status']=='no_write'
bind_release(legacy,'s8b-release-back')
accepted=t('accept_result','s8b-accept');assert accepted['status']=='committed',accepted
assert t('accept_result','s8b-accept')==accepted
assert t('fail_result','s8b-accept')['status']!='committed'
closed=delivered_matching(lambda s:s['messageId'].startswith('tsc_'),3)
closure=next(s for s in closed if s['toCutexSessionId']==successor)
(RUN/'closure-to-successor.json').write_text(json.dumps(closure))
assert all(s['toCutexSessionId']!=durable for s in closed)
assert management('director',rotation)==json.loads((RUN/'s8b-rotate-receipt.json').read_text())
(RUN/'result.json').write_text(json.dumps({'authority':authority,'successor':successor,'old_director':durable,'worker':actors['worker']['durable'],'release':legacy,'operations':len(facts),'same_action_replay':True,'default_bytes':sha(CUTEX)}))
'''
probe.body=setup+ast.parse(campaign).body
cleanup=ast.unparse(ast.Module(body=probe.finalbody,type_ignores=[]))
cleanup=cleanup.replace("[RUN, RUN / 'new-agent']", "[RUN, RUN / 'new-agent', RUN / 'replacement', RUN / 'successor']")
probe.finalbody=ast.parse(cleanup).body
exec(compile(ast.fix_missing_locations(tree),'S8b-private-protected-management','exec'),globals())
