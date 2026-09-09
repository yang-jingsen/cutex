"""Independent read-only S8b authority, receipt and native-history oracle."""
import hashlib
import json
from pathlib import Path
import struct
import sys

root=Path(__file__).resolve().parents[2]
run=root/sys.argv[1]
assert run.parent==root and run.resolve()==run
conf=run/'home/.cutex'
assert (run/'home/.cutex-test-private-home').is_file()
result=json.loads((run/'result.json').read_text())
roster=json.loads((conf/'runtime/agent-management/v1/agent-management-v1.json').read_text())
store=json.loads((conf/'cutex-sessions.json').read_text())
seats=json.loads((conf/'runtime/task-service/seat-authority-v1/seat-occupancy-v1.json').read_text())
rotation=roster['actions']['s8b-rotate']
successor=rotation['known_successor_cutex_session']
project=rotation['project_id']
assert rotation['phase']=='complete'
assert roster['projects'][project]['authorized_director_session']==successor
assert seats['project_director_occupancies'][project]['occupant_cutex_session']==successor
assert not seats['active_project_director_transfers']
for aid,action in roster['actions'].items():
    if not action.get('known_successor_cutex_session'):continue
    ident=action['known_successor_cutex_session'];native=action['known_native_session_id']
    record=store['sessions'][ident];intent=roster['bootstrap_intents'][aid]
    assert action['phase']=='complete'
    assert record['codex_session_id']==native==record['explicit_launch']['native_id']
    spec=intent['request'].get('spec') or intent['request']['successor']
    assert record['formal_agent_name']==spec['name']
    receipts=[v['receipt'] for v in store['explicit_launch_receipts'].values() if v['kind']=='bootstrap' and v['receipt']['cutex_session_id']==ident]
    assert len(receipts)==1 and receipts[0]['native_id']==native
    assert action['response']==json.loads((run/(aid+'-receipt.json')).read_text())

def framed(domain,fields):
    return domain+b''.join(struct.pack('>Q',len(s.encode()))+s.encode() for s in fields)

ledger_path=conf/'runtime/management-v2/agent-bus-message-state.json'
ledger=json.loads(ledger_path.read_text()) if ledger_path.exists() else None
assert ledger is not None or result.get('default_bytes') is False
commits={}
context_bodies={}
for record in store['sessions'].values():
    native=record['codex_session_id']
    paths=list((conf/'codex-home/sessions').glob('**/*'+native+'.jsonl'))
    assert len(paths)==1
    rows=[json.loads(line) for line in paths[0].read_text().splitlines()]
    meta=[r['payload'] for r in rows if r['type']=='session_meta']
    assert len(meta)==1 and meta[0]['id']==native
    assert 'cutex-top-level-session' not in json.dumps(meta)
    for i,row in enumerate(rows):
        if row['type']!='external_input' or row['payload']['fact']['phase']!='commit':continue
        fact=row['payload']['fact']['commit'];e=fact['envelope'];r=fact['receipt'];m=e['message']
        assert ledger is not None
        digest=hashlib.sha256(framed(b'codex:external-input:v1\0',[e['ownerId'],e['threadId'],m['id'],m['source']['kind'],m['source']['id'],m['type'],m['delivery'],m['text']])).hexdigest()
        assert digest==e['semanticSha256']==r['semanticSha256']
        rid='eir1_'+hashlib.sha256(framed(b'codex:external-input-receipt:v1\0',[r['ownerId'],r['threadId'],r['messageId'],r['semanticSha256'],r['responseItemId'],r['turnId']])+struct.pack('>Q',r['ordinal'])).hexdigest()
        assert rid==r['receiptId'] and m['id'] not in commits
        item=rows[i+1]
        assert item['type']=='response_item' and item['payload']['id']==m['id']
        assert json.loads(item['payload']['output'])=={k:m[k] for k in ['source','type','text']}
        snapshot=ledger['messages'][m['id']]['snapshot']
        assert snapshot['state']=='delivered' and snapshot['externalInputReceipt']==r
        commits[m['id']]=r
        context_bodies[m['id']]={k:m[k] for k in ['source','type','text']}
if result.get('default_bytes'):
    start=json.loads((run/'management-start.json').read_text())
    assert start['toCutexSessionId']==successor
    assert start['externalInput']['message']['source']['kind']=='service'
    requests=json.loads((run/'model-requests.json').read_text())
    observed=[]
    for request in requests:
        for item in request.get('input',[]):
            try:observed.append(json.loads(item.get('output','')))
            except (ValueError,TypeError):pass
    assert context_bodies[start['messageId']] in observed
    closure=json.loads((run/'closure-to-successor.json').read_text())
    assert closure['toCutexSessionId']==successor
    provider=json.loads((conf/'runtime/task-service/task-worker-actions-v1/task-service/provider-v2/task-service-provider-v2.json').read_text())
    facts=[f for n in provider['completion_notifications'].values() for f in n['facts'] if f['kind']=='delivered']
    assert len(facts)==3
    assert all(any(f['reference']==mid+':'+r['receiptId'] for mid,r in commits.items()) for f in facts)
else:
    cut=json.loads((run/'cutpoint.json').read_text())
    assert cut['stage']['known_successor_cutex_session']==successor
    assert cut['stage']['known_native_session_id']==rotation['known_native_session_id']
    assert cut['stage']['response'] is None
    assert cut['project']['authorized_director_session']!=successor
    assert cut['seats']['project_director_occupancies'][project]['occupant_cutex_session']==successor
    assert len(list((conf/'codex-home/sessions').glob('**/*.jsonl')))==cut['native_count']
print(json.dumps({'same_successor':successor,'both_authorities':True,'original_receipts':True,'native_pairs':len(commits),'duplicate_pairs':0,'default_bytes':bool(result.get('default_bytes'))}))
