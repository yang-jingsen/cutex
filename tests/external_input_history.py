"""Read-only independent oracle over one completed private S6c probe."""
import hashlib
import json
import sqlite3
import struct
import sys
from pathlib import Path

root=Path(__file__).resolve().parents[2]
run=root/sys.argv[1]
assert run.parent==root and (run/'PASS.json').is_file()
proof=json.loads((run/'PASS.json').read_text())
native=run/'home/.cutex/codex-home'
paths=list((native/'sessions').glob('**/*'+proof['thread']+'.jsonl'))
assert len(paths)==1
history=[json.loads(line) for line in paths[0].read_text().splitlines()]
commits=[]
def framed(domain, fields):
    return domain+b''.join(struct.pack('>Q',len(s.encode()))+s.encode() for s in fields)
for index,row in enumerate(history):
    if row['type']!='external_input' or row['payload']['fact']['phase']!='commit': continue
    commit=row['payload']['fact']['commit']; e=commit['envelope']; r=commit['receipt']; m=e['message']
    assert e['ownerId']==proof['durable'] and e['threadId']==proof['thread']
    digest=hashlib.sha256(framed(b'codex:external-input:v1\0',[
        e['ownerId'],e['threadId'],m['id'],m['source']['kind'],m['source']['id'],m['type'],m['delivery'],m['text']])).hexdigest()
    assert digest==e['semanticSha256']==r['semanticSha256']
    rid='eir1_'+hashlib.sha256(framed(b'codex:external-input-receipt:v1\0',[
        r['ownerId'],r['threadId'],r['messageId'],r['semanticSha256'],r['responseItemId'],r['turnId']])+struct.pack('>Q',r['ordinal'])).hexdigest()
    assert rid==r['receiptId'] and r['messageId']==m['id']==r['responseItemId']
    item=history[index+1]
    assert item['type']=='response_item' and item['payload']['id']==m['id']
    assert item['payload']['name']=='external_event' and item['payload']['namespace']=='external'
    body={'source':m['source'],'type':m['type'],'text':m['text']}
    assert json.loads(item['payload']['output'])==body
    commits.append((r,body))
assert [r['ordinal'] for r,b in commits]==[1,2,3]
assert [r['messageId'] for r,b in commits]==proof['pair_ids']
assert commits[0][0]==proof['receipt']
requests=json.loads((run/'model-requests.json').read_text())
seen=[0]*len(commits)
for request in requests:
    bodies=[]
    for item in request.get('input',[]):
        try: value=json.loads(item.get('output',''))
        except (ValueError,TypeError): continue
        if isinstance(value,dict) and value.get('source',{}).get('id')=='private-test-service': bodies.append(value)
    for i,(_,body) in enumerate(commits):
        count=bodies.count(body); assert count<=1; seen[i]+=count
assert all(seen),seen
databases=list(native.glob('state_*.sqlite')); assert len(databases)==1
db=sqlite3.connect(databases[0].as_uri()+'?mode=ro',uri=True)
assert db.execute('select count(*) from threads where id=?',(proof['thread'],)).fetchone()==(1,)
db.close()
print(json.dumps({'pairs':len(commits),'ordinals':[1,2,3],'native_metadata_rows':1,
    'model_requests':len(requests),'exact_canonical_occurrences':seen,'duplicate_pairs':0}))
