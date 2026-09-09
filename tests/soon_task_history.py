"""Independent read-only S6f multi-owner native/Bus receipt oracle."""
import hashlib
import json
import struct
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[2]
run = root / sys.argv[1]
assert run.parent == root
proof = json.loads((run / 'PASS-SOON-TASK.json').read_text())
ledger = json.loads((run / 'home/.cutex/runtime/management-v2/agent-bus-message-state.json').read_text())

def framed(domain, fields):
    return domain + b''.join(struct.pack('>Q', len(s.encode())) + s.encode() for s in fields)

commits = {}
for native in proof['native']:
    paths = list((run / 'home/.cutex/codex-home/sessions').glob('**/*' + native + '.jsonl'))
    assert len(paths) == 1
    history = [json.loads(line) for line in paths[0].read_text().splitlines()]
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
        commits[m['id']] = (r, body)
assert set(commits) == set(proof['notifications'])
for ident in (proof['assignment'], proof['followup']):
    assert ledger['messages'][ident]['snapshot']['externalInput']['message']['delivery'] == 'soon'
provider = json.loads((run / 'home/.cutex/runtime/task-service/task-worker-actions-v1/task-service/provider-v2/task-service-provider-v2.json').read_text())
facts = [f for n in provider['completion_notifications'].values() for f in n['facts'] if f['kind'] == 'delivered']
assert len(facts) == 3
for fact in facts:
    assert any(fact['reference'] == mid + ':' + r['receiptId'] for mid, (r, _) in commits.items())
requests = json.loads((run / 'model-requests.json').read_text())
seen = {ident: 0 for ident in commits}
maximum = dict(seen)
for request in requests:
    bodies = []
    for item in request.get('input', []):
        try:
            bodies.append(json.loads(item.get('output', '')))
        except (ValueError, TypeError):
            pass
    for ident, (_, body) in commits.items():
        count = bodies.count(body)
        # Distinct notifications can legitimately have identical concise text
        # (the first and repaired ReviewReady). Native uniqueness is checked
        # above by exact message ID, never inferred from prose equality.
        assert count <= sum(other == body for _, other in commits.values())
        seen[ident] += count
        maximum[ident] = max(maximum[ident], count)
assert all(seen.values()), seen
assert all(maximum[ident] == sum(other == body for _, other in commits.values()) for ident, (_, body) in commits.items())
print(json.dumps({'owners': len(proof['native']), 'native_pairs': len(commits), 'duplicate_pairs': 0, 'completion_facts': len(facts), 'effective_model_input_observations': list(seen.values())}))
