"""S8b owned Bus death after Task Director transfer, before project completion.
Separate feature-hook evidence; never a default-byte distribution claim.
"""
import ast
from pathlib import Path
recovery_source=Path(__file__).read_text()
builder=Path(__file__).with_name('protected_management_boundary.py').read_text()
ending="exec(compile(ast.fix_missing_locations(tree),'S8b-private-protected-management','exec'),globals())"
assert ending in builder
exec(compile(builder.replace(ending,''),'S8b-recovery-builder','exec'),globals())
idx=next(i for i,n in enumerate(setup) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='bus' for t in n.targets))
setup[idx:idx]=ast.parse("env['CUTEX_BOOTSTRAP_TEST_TRANSFER_ACTION']='s8b-rotate'").body
start=campaign.index("request['action_id']=")
rotation=campaign.index("rotation={'schema'")
campaign=campaign[:start]+campaign[rotation:]
campaign=campaign.replace("'mode':'retain_predecessor_with_message'","'mode':'retain_predecessor_bootstrap_only'")
campaign=campaign.replace("'frozen_message':'Private successor instructions: inspect current Task Service state. No external actions.'","'frozen_message':None")
cut=campaign.index("actors['successor']=settle")
campaign=campaign[:cut]+'''
(RUN/'recovery-probe-source.py').write_text(recovery_source)
uncertain=management('director',rotation)
assert uncertain['outcome'].get('code')=='response_uncertain',uncertain
assert bus.wait(timeout=600)==86
before=roster();stage=before['actions']['s8b-rotate']
known=stage['known_successor_cutex_session'];native=stage['known_native_session_id']
assert known and native and stage['phase']=='authority_transfer_pending' and stage['response'] is None
seat_path=CONF/'runtime/task-service/seat-authority-v1/seat-occupancy-v1.json'
seats=json.loads(seat_path.read_text())
assert seats['project_director_occupancies'][project]['occupant_cutex_session']==known
assert before['projects'][project]['authorized_director_session']==durable
assert seats['active_project_director_transfers']
native_count=len(list((NATIVE/'sessions').glob('**/*.jsonl')))
(RUN/'cutpoint.json').write_text(json.dumps({'stage':stage,'project':before['projects'][project],'seats':seats,'native_count':native_count}))
del env['CUTEX_BOOTSTRAP_TEST_TRANSFER_ACTION']
bus=owner([CUTEX,'agent','serve','--port',bp],'bus-recovered');ready(bp,bus)
deadline=time.monotonic()+120
while True:
    observed=call(current,threads[0],'/api/agent-management/v1/actions',{'schema':'cutex/agent-management/v1','action_id':'s8b-recovery-observe','project_id':project,'operation':'query_managed'})
    # Query is blocked by the deliberately partial composite seat transfer.
    # Runtime authentication success is sufficient; no new effect is retried.
    if observed.get('outcome',{}).get('code') not in ['stale_runtime_identity','unauthorized'] and observed.get('schema')=='cutex/agent-management/v1':break
    assert time.monotonic()<deadline,observed
    threading.Event().wait(.05)
actor=settle(rotation,management('director',rotation))
assert actor['durable']==known and actor['native']==native
assert len(list((NATIVE/'sessions').glob('**/*.jsonl')))==native_count
after=roster();seats=json.loads(seat_path.read_text())
assert after['projects'][project]['authorized_director_session']==known
assert after['projects'][project]['authority_epoch']==2
assert seats['project_director_occupancies'][project]['occupant_cutex_session']==known
assert not seats['active_project_director_transfers']
assert management('director',rotation)==json.loads((RUN/'s8b-rotate-receipt.json').read_text())
(RUN/'result.json').write_text(json.dumps({'same_durable':known,'same_native':native,'native_count':native_count,'phase':after['actions']['s8b-rotate']['phase'],'both_authorities':known,'original_action_replayed':True,'default_bytes':False}))
'''
probe.body=setup+ast.parse(campaign).body
exec(compile(ast.fix_missing_locations(tree),'S8b-private-transfer-recovery','exec'),globals())
