"""Independent read-only S6g native receipt / Task fact / model-input oracle."""
import hashlib
import json
import struct
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[2]
run = root / sys.argv[1]
assert run.parent == root
proof = json.loads((run / 'PASS-TASK.json').read_text())
ledger = json.loads((run / 'home/.cutex/runtime/management-v2/agent-bus-message-state.json').read_text())
provider = json.loads((run / 'home/.cutex/runtime/task-service/task-worker-actions-v1/task-service/provider-v2/task-service-provider-v2.json').read_text())
sessions = json.loads((run / 'home/.cutex/cutex-sessions.json').read_text())['sessions']
offline = next(r for r in sessions.values() if r['cutex_session_id'] == proof['durable'][3])
assert offline['runtime_generation'] == 0 and not offline.get('app_server_runtime')
assert provider['attempts']['a-s6g-accept']['1']['phase'] == 'completed'
assert provider['attempts']['a-s6g-fail']['1']['phase'] == 'failed'
assert provider['assignments']['a-s6g-accept']['closure']['reason'] == 'completed'
assert provider['assignments']['a-s6g-fail']['closure']['reason'] == 'cancelled'
assert 's6g-old-release' not in provider['receipts']

def framed(domain, fields):
    return domain + b''.join(struct.pack('>Q', len(s.encode())) + s.encode() for s in fields)

commits = {}
contexts = []
for native in proof['native']:
    paths = list((run / 'home/.cutex/codex-home/sessions').glob('**/*' + native + '.jsonl'))
    assert len(paths) == 1
    history = [json.loads(line) for line in paths[0].read_text().splitlines()]
    contexts.extend(row['payload'] for row in history if row['type'] == 'turn_context')
    for index, row in enumerate(history):
        if row['type'] != 'external_input' or row['payload']['fact']['phase'] != 'commit':
            continue
        commit = row['payload']['fact']['commit']
        e, r = commit['envelope'], commit['receipt']
        m = e['message']
        digest = hashlib.sha256(framed(b'codex:external-input:v1\0', [e['ownerId'], e['threadId'], m['id'], m['source']['kind'], m['source']['id'], m['type'], m['delivery'], m['text']])).hexdigest()
        assert digest == e['semanticSha256'] == r['semanticSha256']
        rid = 'eir1_' + hashlib.sha256(framed(b'codex:external-input-receipt:v1\0', [r['ownerId'], r['threadId'], r['messageId'], r['semanticSha256'], r['responseItemId'], r['turnId']]) + struct.pack('>Q', r['ordinal'])).hexdigest()
        assert rid == r['receiptId'] and m['id'] not in commits
        item = history[index + 1]
        body = {'source': m['source'], 'type': m['type'], 'text': m['text']}
        assert item['type'] == 'response_item' and item['payload']['id'] == m['id']
        assert json.loads(item['payload']['output']) == body
        snapshot = ledger['messages'][m['id']]['snapshot']
        assert snapshot['state'] == 'delivered' and snapshot['externalInputReceipt'] == r
        assert snapshot['externalInput']['message'] == m
        assert not any(word in m['text'] for word in ['semanticSha256', 'runtimeGeneration', 'eir1_'])
        commits[m['id']] = (r, body)
assert set(commits) == set(proof['messages'])
facts = []
matrix = []
for notification in provider['completion_notifications'].values():
    delivered = [f for f in notification['facts'] if f['kind'] == 'delivered']
    assert len(delivered) == 1
    fact = delivered[0]
    matches = [(mid, r, body) for mid, (r, body) in commits.items() if fact['reference'] == mid + ':' + r['receiptId']]
    assert len(matches) == 1
    mid, receipt, body = matches[0]
    expected = proof['durable'][2] if notification['kind'] == 'review_ready' else proof['durable'][0]
    assert receipt['ownerId'] == expected
    assert body['source']['kind'] == 'service'
    matrix.append({'kind': notification['kind'], 'owner': expected, 'delivery': notification['delivery_mode']})
    facts.append(fact)
assert len(facts) == 6
assert {r['kind'] for r in matrix} == {'review_ready', 'terminal_closure', 'owner_action_required', 'retries_exhausted'}
assert contexts and all(c['sandbox_policy']['type'] == 'read-only' and c['approval_policy'] == 'on-request' for c in contexts)
requests = json.loads((run / 'model-requests.json').read_text())
observed = []
for request in requests:
    for item in request.get('input', []):
        if item.get('name') != 'external_event':
            continue
        try:
            observed.append(json.loads(item.get('output', '')))
        except (ValueError, TypeError):
            pass
for _, body in commits.values():
    assert body in observed, body
print(json.dumps({'native_pairs': len(commits), 'duplicate_pairs': 0, 'completion_facts': len(facts), 'read_only_on_request_contexts': len(contexts), 'routes': matrix}, indent=2))
