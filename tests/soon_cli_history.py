"""Read-only composed CLI/Soon oracle; no service or runtime requests."""
import hashlib
import json
import struct
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[2]
run = root / sys.argv[1]
assert run.parent == root
proof = json.loads((run / 'PASS-CLI.json').read_text())
ledger = json.loads((run / 'home/.cutex/runtime/management-v2/agent-bus-message-state.json').read_text())
snapshots = {k: v['snapshot'] for k, v in ledger['messages'].items()}
threads = {s['externalInput']['threadId'] for s in snapshots.values()}

def framed(domain, fields):
    return domain + b''.join(struct.pack('>Q', len(s.encode())) + s.encode() for s in fields)

commits = {}
contexts = []
for native in threads:
    paths = list((run / 'home/.cutex/codex-home/sessions').glob('**/*' + native + '.jsonl'))
    assert len(paths) == 1
    history = [json.loads(line) for line in paths[0].read_text().splitlines()]
    contexts.extend(r['payload'] for r in history if r['type'] == 'turn_context')
    for index, row in enumerate(history):
        if row['type'] != 'external_input' or row['payload']['fact']['phase'] != 'commit':
            continue
        c = row['payload']['fact']['commit']
        e, r = c['envelope'], c['receipt']
        m = e['message']
        digest = hashlib.sha256(framed(b'codex:external-input:v1\0', [e['ownerId'], e['threadId'], m['id'], m['source']['kind'], m['source']['id'], m['type'], m['delivery'], m['text']])).hexdigest()
        assert digest == e['semanticSha256'] == r['semanticSha256']
        rid = 'eir1_' + hashlib.sha256(framed(b'codex:external-input-receipt:v1\0', [r['ownerId'], r['threadId'], r['messageId'], r['semanticSha256'], r['responseItemId'], r['turnId']]) + struct.pack('>Q', r['ordinal'])).hexdigest()
        assert rid == r['receiptId'] and m['id'] not in commits
        item = history[index + 1]
        body = {'source': m['source'], 'type': m['type'], 'text': m['text']}
        assert item['type'] == 'response_item' and item['payload']['id'] == m['id']
        assert json.loads(item['payload']['output']) == body
        s = snapshots[m['id']]
        assert s['state'] == 'delivered' and s['externalInputReceipt'] == r
        assert m['delivery'] == 'soon' and m['source']['kind'] == 'service'
        commits[m['id']] = (r, body)
assert len(commits) == 3
assert contexts and all(c['sandbox_policy']['type'] == 'read-only' and c['approval_policy'] == 'on-request' for c in contexts)
provider = json.loads((run / 'home/.cutex/runtime/task-service/task-worker-actions-v1/task-service/provider-v2/task-service-provider-v2.json').read_text())
notifications = list(provider['completion_notifications'].values())
assert {n['kind'] for n in notifications} == {'blocked', 'terminal_closure'}
facts = [f for n in notifications for f in n['facts'] if f['kind'] == 'delivered']
assert len(facts) == 2
assert all(any(f['reference'] == mid + ':' + r['receiptId'] for mid, (r, _) in commits.items()) for f in facts)
requests = json.loads((run / 'model-requests.json').read_text())
for _, body in commits.values():
    counts = []
    for request in requests:
        bodies = []
        for item in request.get('input', []):
            try:
                bodies.append(json.loads(item.get('output', '')))
            except (ValueError, TypeError):
                pass
        counts.append(bodies.count(body))
    assert max(counts) == 1
pending = [s for k, s in snapshots.items() if k not in commits]
assert len(pending) == 1 and pending[0]['state'] == 'pending' and 'externalInputReceipt' not in pending[0]
status = json.loads((run / 'pause-status.json').read_text())
assert status['receipt'] is None and status['processing']['reason'] == 'interrupted'
assert json.loads((run / 'cutex-attach.exit.json').read_text()) == {'exit': 0, 'termios_restored': True}
print(json.dumps({'native_pairs': len(commits), 'duplicate_pairs': 0, 'completion_facts': len(facts), 'pending_interrupted': 1, 'model_calls': len(requests), 'read_only_on_request_contexts': len(contexts)}))
