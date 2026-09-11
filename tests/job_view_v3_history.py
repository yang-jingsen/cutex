"""Independent read-only producer/Bus/native history oracle after owned cleanup."""
import json
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[2]
run = root / sys.argv[1]
assert run.parent == root and (run/'h/.cutex-test-private-home').is_file()
state = json.loads((run/'job-state/state.json').read_text())
assert len(state['jobs']) == len(state['outbox']) == 1
job = next(iter(state['jobs'].values()))
outbox = next(iter(state['outbox'].values()))
assert job['state'] == 'exited' and job['exitCode'] == 0
assert outbox['wireVersion'] == 2 and outbox['acknowledged']
frozen = outbox['frozenRequestV2']
assert frozen['schema'] == 'cutex.job_service.completion.v2'
bus = json.loads((run/'h/.cutex/runtime/management-v2/agent-bus-message-state.json').read_text())
assert bus['version'] == 6 and len(bus['messages']) == 1
snapshot = next(iter(bus['messages'].values()))['snapshot']
envelope = snapshot['externalInput']
receipt = snapshot['externalInputReceipt']
assert snapshot['state'] == 'delivered' and not snapshot.get('presentation')
assert outbox['receipt']['a4Receipt'] == receipt
assert envelope['version'] == 2
data = envelope['view']['data']
assert data['jobId'] == frozen['jobId'] == job['jobId']
for key, value in frozen['facts'].items():
    if key != 'factsVersion':
        assert data[key] == value, key
assert data['outputReference'] == frozen['outputReference']
assert data['execution']['observedRunDurationMillis'] > 0
assert envelope['message']['text'].count(job['jobId']) == 1
files = list((run/'h/.cutex/codex-home/sessions').rglob('*.jsonl'))
assert len(files) == 1
rows = [json.loads(line) for line in files[0].read_text().splitlines()]
commits = [(i,r['payload']['fact']['commit']) for i,r in enumerate(rows)
    if r['type'] == 'external_input' and r['payload']['fact']['phase'] == 'commit']
assert len(commits) == 1
i, commit = commits[0]
assert commit['receipt'] == receipt and commit['envelope']['view'] == envelope['view']
canonical = rows[i+1]
assert canonical['type'] == 'response_item' and canonical['payload']['name'] == 'external_event'
assert data['outputReference'] not in json.dumps(canonical)
assert not any(r['type'] == 'event_msg' and r['payload'].get('type') == 'presentation_appended' for r in rows)
requests = json.loads((run/'model-requests.json').read_text())
external = [json.loads(item['output']) for request in requests for item in request.get('input',[])
    if item.get('name') == 'external_event' and item.get('type') == 'function_call_output']
assert external and all(e['text'] == envelope['message']['text'] and set(e) == {'source','type','text'} for e in external)
assert data['outputReference'] not in json.dumps(external)
print(json.dumps({'fixture':run.name,'jobs':1,'outboxAcknowledged':True,'nativeCommitPairs':1,
    'sameOriginalA4':True,'noPresentation':True,'producerFactsEqualView':True,
    'modelText':envelope['message']['text'],'view':envelope['view'],
    'externalProjectionExcludesView':True,'provider':'local deterministic fake'},ensure_ascii=False))
