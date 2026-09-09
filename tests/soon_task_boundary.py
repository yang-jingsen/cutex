"""S6f actual private Cutex Task -> pinned Soon native boundary.

Reuse only accepted setup/cleanup and HTTP helpers, not previous campaigns.
Both Task actors are explicitly adopted and activated with NEW v2 markers.
"""
import ast
from pathlib import Path

source=Path(__file__).with_name('external_input_boundary.py').read_text()
for old,new in [
 ("Path(__file__).with_name('stock_launch_boundary.py').read_text()", "Path(__file__).with_name('stock_launch_boundary.py').read_text().replace('timeout=60', 'timeout=240')"),
 ('native-c2aaceb4/bin/codex-app-server','native-a83dbb47b/bin/codex-app-server'),
 ('artifacts/s6c1-schema-handoff/codex_app_server_protocol.schemas.json','artifacts/s6e/schema/codex_app_server_protocol.schemas.json'),
 ('b70d48151c9deb76a9c0ab14a820c582f2bc12a73bbb1512fee9b2f1bec9fa60','4638b86221593dd4bab1f66b504641836ac1adb864e946cbe42cb5cbf9f05a74'),
 ('00e035e34ac1034ee34473f8f68b7704d6058c5b180ff4f4b6cad9fadab3a86d','459861225d5bfb73bb4c3896edb489169637424be410be346f955a39596da7e9'),
 ('[PATCHED,STOCK]','[PATCHED,PATCHED]'),
 ("'version':2 if n==0 else 1","'version':3"),
 ("verified(SCHEMA if n==0 else ROOT/'s4-schema/codex_app_server_protocol.schemas.json')","verified(SCHEMA)"),
 ("if n==0: bundle['native_patch_commit']='c2aaceb411b7851806c62435b97895a63a7d34cd'","bundle['native_patch_commit']='a83dbb47ba6aa775f5d4b679fafc532c4db74c7f'; bundle['cli']=verified(PATCHED.with_name('codex'))"),
 ("contract={'version':1","contract={'version':2"),
 ("args=[PATCHED,'-c'","args=[PATCHED,'-c','default_permissions=\":read-only\"','-c'"),
 ('Model.calls <= 12','Model.calls <= 30'),
]:
    assert old in source,old
    source=source.replace(old,new)
tree=ast.parse(source)
probe=next(n for n in tree.body if isinstance(n,ast.Try))
cut=next(i for i,n in enumerate(probe.body) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='stock' for t in n.targets))
setup=probe.body[:cut]
old=ast.parse(Path(__file__).with_name('external_delivery_boundary.py').read_text())
helpers=[n for n in old.body if isinstance(n,ast.FunctionDef)]
campaign='''
stock=launch(legacy,'s6f-worker')
current=launch(durable,'s6f-director')
for occurrence in (stock,current):
    peer,cap=native_rpc(occurrence)
    assert cap['externalInputVersion']==1 and 'soon' in cap['externalInputDeliveries']
def mutation(action_id,kind,revision,**fields):
    return {'schema':'cutex/human-management-project-mutation/v1','action_id':action_id,'project_id':'s6c2-project','expected_authority_epoch':0 if kind=='create' else 1,'expected_project_revision':revision,'operation':{'kind':kind,**fields}}
create=mutation('s6f-project','create',0,director_cutex_session_id=durable,presentation={'display_name':'Private S6f','badge_label':'S6','color':'cyan'})
for n,ident in enumerate(ids):
    code,candidates=api(mp,'/v2/agent-management/durable-candidates');assert code==200
    candidate=next(c for c in candidates if c['cutex_session_id']==ident)
    code,result=api(mp,'/v2/agent-management/durable-import',{'action_id':f's6f-import-{n}','candidate':candidate,'confirmed_formal_name':f'Private S6 Agent {n}','assignment':create if n==0 else None,'detach':None})
    assert code==200 and result['complete'],(code,result)
code,result=api(mp,'/v2/agent-management/project-mutations',mutation('s6f-member','add_member',1,cutex_session_id=legacy));assert code==200,(code,result)
contract='Private Soon Task fixture. Use Task Service tools; no external actions.'
assert director('create_revision','s6f-create',project_id='s6c2-project',workflow_id='wf-s6f',task_id='t-s6c2',task_revision=1,opaque_contract=contract,contract_sha256=hashlib.sha256(contract.encode()).hexdigest(),completion_policy='director_acceptance')['status']=='committed'
busy_mode=len(sys.argv)>3 and sys.argv[3]=='busy'
if busy_mode:
    Model.gated=threading.Event();Model.gate=threading.Event();release=Model.gate
    worker_rpc,_=native_rpc(stock)
    busy_turn=worker_rpc.call('turn/start',{'threadId':threads[1],'input':[{'type':'text','text':'Private genuine Human timing probe','text_elements':[]}]})['turn']['id']
    assert Model.gated.wait(30)
assert director('assign','s6f-assign',project_id='s6c2-project',task_id='t-s6c2',task_revision=1,assignment_id='a-s6c2',assignee_cutex_session_id=legacy,summary='Private assignment')['status']=='committed'
def family(prefix,count=1):
    until=time.monotonic()+60
    while True:
        # The first poll may precede creation of the Bus repository. This is
        # fixture observation only, not a provider read-success fallback.
        try: found=[k for k in ledger()['messages'] if k.startswith(prefix)]
        except FileNotFoundError: found=[]
        if len(found)>=count:return [(k,delivered(k)) for k in found]
        assert time.monotonic()<until,(prefix,found)
        threading.Event().wait(.02)
if busy_mode:
    later=call(current,thread,'/api/messages/send',{'to':legacy,'content':'AfterTurn waits for the current turn','external_message_id':'s6f-afterturn','delivery_mode':'after_turn','kind':'message','from_agent_id':current['runtime_agent_id'],'all_groups':False,'all_hosts':False})['id']
    until=time.monotonic()+60
    while True:
        queued=[s['snapshot'] for k,s in ledger()['messages'].items() if k.startswith('tsa_')]
        if queued and queued[0].get('externalInputLastObserved',{}).get('deliveryState')=='pending':break
        assert time.monotonic()<until,queued
        threading.Event().wait(.02)
    assert queued[0]['state']=='pending' and 'externalInputReceipt' not in queued[0]
    release.set()
assignment=family('tsa_')[0]
if busy_mode:
    assert assignment[1]['externalInputReceipt']['turnId']==busy_turn
    after=delivered(later);assert after['externalInputReceipt']['turnId']!=busy_turn
assert assignment[1]['externalInput']['message']['delivery']=='soon'
assert assignment[1]['externalInput']['message']['text'].count('Assignment ID: a-s6c2')==1
assert contract in assignment[1]['externalInput']['message']['text']
assert worker('start','s6f-start')['status']=='committed'
if len(sys.argv)>3 and sys.argv[3]=='watchdog':
    family('tsw_',2)
assert worker('submit','s6f-submit',result_sha256='a'*64,result_reference='private:first')['status']=='committed'
review=family('tsc_')[0]
assert director('request_changes','s6f-changes',assignment_id='a-s6c2',decision_reference='Private repair requested')['status']=='committed'
followup=family('tsf_')[0]
assert followup[1]['externalInput']['message']['delivery']=='soon'
assert 'Private repair requested' in followup[1]['externalInput']['message']['text']
assert worker('report_status','s6f-repair-status',summary='Private repair done')['status']=='committed'
assert worker('submit','s6f-repair-submit',result_sha256='b'*64,result_reference='private:repaired')['status']=='committed'
family('tsc_',2)
assert director('accept_result','s6f-accept',assignment_id='a-s6c2',decision_reference='Private accepted')['status']=='committed'
family('tsc_',3)
assert sha(NATIVE/'config.toml')==shared_sha and ledger()['version']==4
(RUN/'PASS-SOON-TASK.json').write_text(json.dumps({'native':threads,'durable':ids,'assignment':assignment[0],'followup':followup[0],'notifications':list(ledger()['messages']),'model_calls':Model.calls,'boundary':'real providers/native, harness drives outbound Worker actions; not autonomous MCP execution'},indent=2))
'''
# Optional deterministic busy response gate, or the existing real watchdog
# clock/poll settings. No artificial persistence barrier or state-file edit.
source=source.replace("        events = [{'type':'response.created'", "        gate=getattr(Model,'gate',None)\n        if gate is not None:\n            Model.gate=None;Model.gated.set();assert gate.wait(60)\n        events = [{'type':'response.created'")
source=source.replace('if not Model.empty:', 'if not Model.empty and gate is None:')
source=source.replace("    bus=owner([CUTEX", "    if len(sys.argv)>3 and sys.argv[3]=='watchdog':env.update(CUTEX_TASK_WATCHDOG_POLL_SECS='5',CUTEX_TASK_WATCHDOG_STALE_SECS='60',CUTEX_TASK_WATCHDOG_ESCALATION_SECS='60')\n    bus=owner([CUTEX")
tree=ast.parse(source);probe=next(n for n in tree.body if isinstance(n,ast.Try))
cut=next(i for i,n in enumerate(probe.body) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='stock' for t in n.targets))
if "'watchdog'" in campaign:campaign=campaign.replace('until=time.monotonic()+60','until=time.monotonic()+180')
probe.body=probe.body[:cut]+ast.parse(campaign).body
index=tree.body.index(probe)
tree.body[index:index]=helpers
exec(compile(ast.fix_missing_locations(tree),'S6f-private-Task','exec'),globals())
