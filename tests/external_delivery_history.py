"""Read-only independent native pair/digest versus durable Bus oracle."""
import hashlib,json,struct,sys
from pathlib import Path
root=Path(__file__).resolve().parents[2]
run=root/sys.argv[1]
assert run.parent==root
proof=json.loads((run/('PASS-FAULT.json' if (run/'PASS-FAULT.json').exists() else 'PASS-PARTIAL.json')).read_text())
ledger=json.loads((run/'home/.cutex/runtime/management-v2/agent-bus-message-state.json').read_text())
paths=list((run/'home/.cutex/codex-home/sessions').glob('**/*'+proof['native']+'.jsonl'))
assert len(paths)==1
history=[json.loads(line) for line in paths[0].read_text().splitlines()]
def framed(domain,fields):return domain+b''.join(struct.pack('>Q',len(s.encode()))+s.encode() for s in fields)
commits={}
for index,row in enumerate(history):
    if row['type']!='external_input' or row['payload']['fact']['phase']!='commit':continue
    commit=row['payload']['fact']['commit'];e=commit['envelope'];r=commit['receipt'];m=e['message']
    digest=hashlib.sha256(framed(b'codex:external-input:v1\0',[e['ownerId'],e['threadId'],m['id'],m['source']['kind'],m['source']['id'],m['type'],m['delivery'],m['text']])).hexdigest()
    assert digest==e['semanticSha256']==r['semanticSha256']
    rid='eir1_'+hashlib.sha256(framed(b'codex:external-input-receipt:v1\0',[r['ownerId'],r['threadId'],r['messageId'],r['semanticSha256'],r['responseItemId'],r['turnId']])+struct.pack('>Q',r['ordinal'])).hexdigest()
    assert rid==r['receiptId'] and m['id'] not in commits
    item=history[index+1];body={'source':m['source'],'type':m['type'],'text':m['text']}
    assert item['type']=='response_item' and item['payload']['id']==m['id'] and json.loads(item['payload']['output'])==body
    snapshot=ledger['messages'][m['id']]['snapshot']
    assert snapshot['state']=='delivered' and snapshot['externalInputReceipt']==r
    frozen=snapshot['externalInput'];assert frozen['message']==m and frozen['semanticSha256']==digest
    commits[m['id']]=body
assert len(commits)==(2 if 'generations' in proof else 3)
if 'task' in proof:
    provider=json.loads((run/'home/.cutex/runtime/task-service/task-worker-actions-v1/task-service/provider-v2/task-service-provider-v2.json').read_text())
    notification=next(iter(provider['completion_notifications'].values()))
    receipt=ledger['messages'][proof['task']]['snapshot']['externalInputReceipt']
    delivered=[f for f in notification['facts'] if f['kind']=='delivered']
    assert len(delivered)==1 and delivered[0]['reference']==proof['task']+':'+receipt['receiptId']
    assert notification['project_id']=='s6c2-project'
    assert receipt['ownerId']==proof['durable']
    job=json.loads((run/'job-state/state.json').read_text())['jobs'][proof['job']['job_id']]
    assert job['completionDelivery']['state']=='delivered'
    assert any(body['source']=={'kind':'service','id':'cutex-job-service'} and proof['job']['job_id'] in body['text'] for body in commits.values())
requests=json.loads((run/'model-requests.json').read_text())
seen={ident:0 for ident in commits}
for request in requests:
    bodies=[]
    for item in request.get('input',[]):
        try:bodies.append(json.loads(item.get('output','')))
        except (ValueError,TypeError):pass
    for ident,body in commits.items():
        count=bodies.count(body);assert count<=1;seen[ident]+=count
assert all(seen.values()),seen
print(json.dumps({'native_pairs':len(commits),'duplicate_pairs':0,'bus_receipts_equal_native':True,'effective_model_input_observations':list(seen.values())}))
