"""Read-only S6c3 native/Bus/Task oracle, including truthful pending A4."""
import hashlib,json,struct,sys
from pathlib import Path
root=Path(__file__).resolve().parents[2]
run=root/sys.argv[1]
assert run.parent==root
ledger=json.loads((run/'home/.cutex/runtime/management-v2/agent-bus-message-state.json').read_text())
snapshots={k:v['snapshot'] for k,v in ledger['messages'].items() if v['snapshot'].get('externalInput')}
threads={s['externalInput']['threadId'] for s in snapshots.values()};assert len(threads)==1
native=threads.pop()
paths=list((run/'home/.cutex/codex-home/sessions').glob('**/*'+native+'.jsonl'));assert len(paths)==1
history=[json.loads(line) for line in paths[0].read_text().splitlines()]
def framed(domain,fields):return domain+b''.join(struct.pack('>Q',len(s.encode()))+s.encode() for s in fields)
commits={}
for i,row in enumerate(history):
    if row['type']!='external_input' or row['payload']['fact']['phase']!='commit':continue
    c=row['payload']['fact']['commit'];e=c['envelope'];r=c['receipt'];m=e['message']
    digest=hashlib.sha256(framed(b'codex:external-input:v1\0',[e['ownerId'],e['threadId'],m['id'],m['source']['kind'],m['source']['id'],m['type'],m['delivery'],m['text']])).hexdigest()
    assert digest==e['semanticSha256']==r['semanticSha256']
    rid='eir1_'+hashlib.sha256(framed(b'codex:external-input-receipt:v1\0',[r['ownerId'],r['threadId'],r['messageId'],r['semanticSha256'],r['responseItemId'],r['turnId']])+struct.pack('>Q',r['ordinal'])).hexdigest()
    assert rid==r['receiptId'] and m['id'] not in commits
    item=history[i+1];body={'source':m['source'],'type':m['type'],'text':m['text']}
    assert item['type']=='response_item' and item['payload']['id']==m['id'] and json.loads(item['payload']['output'])==body
    s=snapshots[m['id']];assert s['externalInput']['message']==m
    if s['state']=='delivered':assert s['externalInputReceipt']==r
    else:assert s['state']=='pending' and 'externalInputReceipt' not in s and s['externalInputLastObserved']['receipt']==r
    commits[m['id']]=r
assert commits
provider_path=run/'home/.cutex/runtime/task-service/task-worker-actions-v1/task-service/provider-v2/task-service-provider-v2.json'
facts=[]
if provider_path.exists():
    for n in json.loads(provider_path.read_text())['completion_notifications'].values():
        for f in n['facts']:
            if f['kind']=='delivered':
                assert any(f['reference']==mid+':'+r['receiptId'] for mid,r in commits.items())
                facts.append(f['reference'])
if sys.argv[2]=='taskfact':
    task=[s for k,s in snapshots.items() if k.startswith('tsc_')];assert len(task)==1
    assert task[0]['externalInputCommitGeneration']==2 and len(facts)==1
elif sys.argv[2]=='director':
    assert not facts and any(s['state']=='pending' for s in snapshots.values())
elif sys.argv[2]=='recovery':
    actions=ledger['externalRecoveryActions'];assert len(actions)==1
    a=next(iter(actions.values()));assert a['result']['disposition']=='released'
    assert a['review']['status']['receipt']==commits[a['review']['envelope']['message']['id']]
print(json.dumps({'run':run.name,'native':native,'native_pairs':len(commits),'duplicate_pairs':0,'task_context_facts':len(facts),'commit_generations':[s.get('externalInputCommitGeneration') for s in snapshots.values()],'pending_a4':sum(s['state']=='pending' for s in snapshots.values())}))
