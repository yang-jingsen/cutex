"""Real S6c3 Task cutpoints; private API authority change is not full rotation."""
import ast
from pathlib import Path

def setup_cutpoints(g):
    import socket
    g['stop_owned'](g['management'])
    env=g['env'];env['CUTEX_NATIVE_TEST_LOST_SUBMIT']='first';env['CUTEX_NATIVE_TEST_LOST_OWNER']=g['durable']
    env['CUTEX_NATIVE_DELIVERY_TEST_STAGE']={'director':'before_business_commit','taskfact':'after_task_fact','closure':'none'}[g['sys'].argv[3]]
    env['CUTEX_NATIVE_DELIVERY_TEST_MESSAGE']='tsn-ff584381cd6e8ed44f54dbf573aafbb6eeec3f027ae37ad52d20dd63b935ff82'
    path=g['HOME']/'task-gate.sock';env['CUTEX_NATIVE_DELIVERY_TEST_GATE']=str(path)
    gate=socket.socket(socket.AF_UNIX);gate.bind(str(path));gate.listen();gate.settimeout(90);g['task_gate']=gate
    g['management']=g['owner']([g['CUTEX'],'management','serve','--port',g['mp']],'management-fault-owner');g['ready'](g['mp'],g['management'])

def task_cutpoint(g):
    import json,time,threading
    if g['sys'].argv[3]=='closure':return
    gate=g['task_gate'];connection,_=gate.accept();mid=connection.makefile('rb').readline().decode().strip()
    assert mid.startswith('tsc_')
    snapshot=g['ledger']()['messages'][mid]['snapshot'];assert snapshot['state']=='pending' and 'externalInputReceipt' not in snapshot
    original=snapshot['externalInputLastObserved']['receipt'];assert original
    provider_path=g['CONF']/'runtime/task-service/task-worker-actions-v1/task-service/provider-v2/task-service-provider-v2.json'
    def notification():return next(iter(json.loads(provider_path.read_text())['completion_notifications'].values()))
    facts=notification()['facts']
    assert (g['HOME']/'lost-native-submit').read_text()==g['mid']
    if g['sys'].argv[3]=='director':
        assert not any(f['kind']=='delivered' for f in facts)
        request={'schema':'cutex/agent-management/v1','action_id':'s6c3-authority-change','project_id':'s6c2-project','authorized_director_session':g['legacy'],'expected_authorized_director_session':g['durable'],'expected_authority_epoch':1}
        code,result=g['api'](g['mp'],'/v2/agent-management/authority',request);assert code==200,(code,result)
        connection.sendall(b'continue\n');connection.close();gate.close()
        deadline=time.monotonic()+30
        while True:
            pending=g['ledger']()['messages'][mid]['snapshot']
            if 'authority/seat mismatch' in json.dumps(pending.get('error')):break
            assert time.monotonic()<deadline,pending
            threading.Event().wait(.02)
        assert pending['state']=='pending' and 'externalInputReceipt' not in pending
        assert not any(f['kind']=='delivered' for f in notification()['facts'])
        (g['RUN']/'PASS-DIRECTOR-FENCE.json').write_text(json.dumps({'message':mid,'native_receipt':original,'native':g['thread'],'durable':g['durable'],'boundary':'real Human authority CAS before business commit; deliberately incomplete coupled transfer, no full rotation claim','task_delivered':False,'bus_delivered':False},indent=2))
        raise SystemExit(0)
    delivered=[f for f in facts if f['kind']=='delivered'];assert len(delivered)==1 and delivered[0]['reference']==mid+':'+original['receiptId']
    # Both owned transport and runtime-owner processes die after the actual
    # Task fact, while native remains an owned/recoverable occurrence.
    g['stop_owned'](g['management']);g['stop_owned'](g['bus']);connection.close();gate.close()
    for key in ['CUTEX_NATIVE_TEST_LOST_SUBMIT','CUTEX_NATIVE_TEST_LOST_OWNER','CUTEX_NATIVE_DELIVERY_TEST_MESSAGE','CUTEX_NATIVE_DELIVERY_TEST_STAGE','CUTEX_NATIVE_DELIVERY_TEST_GATE']:g['env'].pop(key,None)
    g['bus']=g['owner']([g['CUTEX'],'agent','serve','--port',g['bp']],'bus-recovered');g['ready'](g['bp'],g['bus'])
    g['management']=g['owner']([g['CUTEX'],'management','serve','--port',g['mp']],'management-recovered');g['ready'](g['mp'],g['management'])
    g['current']=g['launch'](g['durable'],'s6c3-task-restart',restart=True)
    final=g['delivered'](mid);assert final['externalInputReceipt']==original and final['externalInputCommitGeneration']==2
    assert notification()['facts']==facts
    g['cutpoint_result']={'message':mid,'receipt':original,'generation':2,'task_facts_unchanged':True}

def finish_cutpoints(g):
    import json,time,threading
    if g['sys'].argv[3]=='taskfact':
        (g['RUN']/'PASS-CUTPOINTS.json').write_text(json.dumps({'durable':g['durable'],'native':g['thread'],'ordinary':g['mid'],'review_ready':g['cutpoint_result'],'release_review_accept':'not exercised: Director MCP transport requires project authority; separate release authority is not impersonated','closure':'separate director-acceptance fixture; no claim here','lost_reply_reconciled':True},indent=2))
        return
    accepted=g['director']('accept_result','s6c3-accept',assignment_id='a-s6c2',decision_reference='private-accept:s6c3')
    assert accepted['status']=='committed',accepted
    g['task_gate'].close()
    deadline=time.monotonic()+60
    while True:
        new=[k for k in g['ledger']()['messages'] if k.startswith('tsc_') and k!=g['taskmid']]
        if new:break
        assert time.monotonic()<deadline
        threading.Event().wait(.02)
    closure=g['delivered'](new[0]);assert 'TerminalClosure' in closure['externalInput']['message']['text']
    (g['RUN']/'PASS-CUTPOINTS.json').write_text(json.dumps({'durable':g['durable'],'native':g['thread'],'ordinary':g['mid'],'review_ready':g.get('cutpoint_result',{'message':g['taskmid']}),'closure':new[0],'closure_receipt':closure['externalInputReceipt'],'lost_reply_reconciled':True},indent=2))

def release_project(g):
    import hashlib
    api=g['api'];mp=g['mp']
    for n,ident in enumerate(g['ids']):
        code,candidates=api(mp,'/v2/agent-management/durable-candidates');assert code==200
        candidate=next(c for c in candidates if c['cutex_session_id']==ident)
        code,result=api(mp,'/v2/agent-management/durable-import',{'action_id':f's6c2-import-{n}','candidate':candidate,'confirmed_formal_name':f'Private S6 Agent {n}','assignment':None,'detach':None});assert code==200 and result['complete']
    for action,epoch,revision,operation in [
        ('s6c2-project',0,0,{'kind':'create','director_cutex_session_id':g['legacy'],'presentation':{'display_name':'Private Release Review','badge_label':'S6','color':'cyan'}}),
        ('s6c2-member',1,1,{'kind':'add_member','cutex_session_id':g['durable']})]:
        code,result=api(mp,'/v2/agent-management/project-mutations',{'schema':'cutex/human-management-project-mutation/v1','action_id':action,'project_id':'s6c2-project','expected_authority_epoch':epoch,'expected_project_revision':revision,'operation':operation});assert code==200,(code,result)
    seat_token=hashlib.sha256(b'cutex/task-service-seat-management/v1\0'+g['HUMAN_TOKEN'].encode()).hexdigest()
    code,result=api(mp,'/v2/task-service/seats/bind',{'schema':'cutex/seat-occupancy-command/v1','action_id':'private-release-seat','seat_id':'cutex-release','occupant_cutex_session':g['durable']},token=seat_token);assert code==200,(code,result)
    def director(op,ident,**fields):
        if op=='create_revision':fields.update(completion_policy='release_review',completion_authority_cutex_session_id=g['durable'])
        receipt,thread=(g['current'],g['thread']) if op=='accept_result' else (g['stock'],g['threads'][1])
        return g['call'](receipt,thread,'/api/task/v2/director-action',{'schema':'cutex/task-service-director-action/v2','operation':op,'action_id':ident,**fields})
    g['director']=director

tree=ast.parse(Path(__file__).with_name('external_delivery_boundary.py').read_text())
probe=next(n for n in tree.body if isinstance(n,ast.Try))
start=next(i for i,n in enumerate(probe.body) if isinstance(n,ast.FunctionDef) and n.name=='mutation')
stop=next(i for i,n in enumerate(probe.body) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='contract' for t in n.targets))
branch=ast.parse("if sys.argv[3]=='taskfact':\n release_project(globals())\nelse:\n pass").body[0]
branch.orelse=probe.body[start:stop];probe.body[start:stop]=[branch]
probe.body.insert(1,ast.parse('setup_cutpoints(globals())').body[0])
index=next(i for i,n in enumerate(probe.body) if isinstance(n,ast.Assert) and "worker('submit'" in ast.unparse(n))
probe.body.insert(index+1,ast.parse('task_cutpoint(globals())').body[0])
end=next(i for i,n in enumerate(probe.body) if isinstance(n,ast.ImportFrom) and n.module=='external_delivery_job')
probe.body=probe.body[:end]+ast.parse('finish_cutpoints(globals())').body
exec(compile(ast.fix_missing_locations(tree),'S6c2-private-task-cutpoints','exec'),globals())
