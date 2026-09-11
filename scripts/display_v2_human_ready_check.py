"""Read-only prepared-owner/configured-MCP check; never sends a turn."""
import ast
import hashlib
import json
from pathlib import Path
import fixed_human_entry as entry

root = Path(__file__).resolve().parents[1]
run = root / 'h1'
info = json.loads((run / 'handoff.json').read_text())
assert entry.identity_matches(info['runtime'])
assert all(entry.identity_matches(p) for p in info['services'])
assert entry.identity_matches(info['observer'])
conf = run / 'h/.cutex'
record = json.loads((conf / 'cutex-sessions.json').read_text())['sessions'][info['durable']]
assert record['runtime_generation'] == info['generation'] == 1
assert record['codex_session_id'] == info['native']
assert record['app_server_runtime']['pid'] == info['runtime']['pid']
policy = json.loads((conf / 'config.json').read_text())['private_job_presentation']
assert policy == {'version': 2, 'recipients': [info['durable']]}
base = root / 'fixtures/base-fixture.py'
assert hashlib.sha256(base.read_bytes()).hexdigest() == 'ed1ebdb57d413db2f3f382aa65c1bf014e4ccb8b8483b63dd85e774717d9728e'
nodes = [n for n in ast.parse(base.read_text()).body if isinstance(n, ast.ClassDef) and n.name == 'RPC']
scope = dict(vars(entry))
exec(compile(ast.Module(body=nodes, type_ignores=[]), str(base), 'exec'), scope)
rpc = scope['RPC'](record['app_server_runtime']['endpoint'].removeprefix('unix://'))
rpc.call('initialize', {'clientInfo': {'name': 'private-ready-check', 'version': '1'}, 'capabilities': {'experimentalApi': True}})
rpc.notify('initialized')
rpc.call('thread/resume', {'threadId': info['native']})
assert rpc.call('thread/read', {'threadId': info['native'], 'includeTurns': True})['thread']['turns'] == []
inventory = rpc.call('mcpServerStatus/list', {'threadId': info['native'], 'detail': 'toolsAndAuthOnly', 'limit': 100})
found = {}
for server in inventory['data']:
    if server['name'] not in ('cutex', 'cutex_job'):
        continue
    assert server['runtimeStatus'] == 'connected'
    assert all(t.get('inputSchema') for t in server['tools'].values())
    found[server['name']] = sorted(t['name'] for t in server['tools'].values())
assert found['cutex_job'] == ['cancel', 'query', 'read_output', 'submit']
assert len(found['cutex']) == 7
assert not (root / 'staged-auth.json').exists()
assert (conf / 'codex-home/auth.json').stat().st_mode & 0o077 == 0
print(json.dumps({'ready': True, 'durable': info['durable'], 'native': info['native'],
                  'generation': info['generation'], 'ports': [info['bus_port'], info['management_port']],
                  'tools': found, 'neutralTurns': 0, 'presentationPolicy': 'v2 default suppress',
                  'temporaryAuthRetained': True, 'stagedAuthRemoved': True}))
