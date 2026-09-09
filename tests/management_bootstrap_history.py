"""Independent read-only S8a journal/native-history oracle; no service calls."""
import json
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[2]
run = root / sys.argv[1]
assert run.parent == root and run.resolve() == run
home = run / 'home'
assert (home / '.cutex-test-private-home').is_file()
conf = home / '.cutex'
stage_only = len(sys.argv)>2 and sys.argv[2]=='stage-only'
result = json.loads((run / 'result.json').read_text()) if not stage_only else None
roster = json.loads((conf / 'runtime/agent-management/v1/agent-management-v1.json').read_text())
sessions = json.loads((conf / 'cutex-sessions.json').read_text())
action = roster['actions']['s8a-create']
intent = roster['bootstrap_intents']['s8a-create']
assert roster['schema'] == 'cutex/agent-management-store/v2'
assert intent['request']['bootstrap_intent'] == action['action_id']
assert intent['director'] == action['caller_cutex_session']
if result is not None and result.get('cutpoint') == 'pre-id':
    assert action['known_native_session_id'] is None
    assert action['known_successor_cutex_session'] is None
    assert action['response']['outcome']['status'] == 'owner_action_required'
    assert result['native_start_retry'] is False and result['model_calls'] == 0
    assert len(sessions['sessions']) == 2
    print(json.dumps({'pre_id': 'unknown/fenced, not successful creation', 'durable_count': 2}))
    raise SystemExit(0)

ident = action['known_successor_cutex_session']
native = action['known_native_session_id']
assert action['phase'] == 'complete'
assert action['response'] == json.loads((run / 'create-result.json').read_text())
record = sessions['sessions'][ident]
assert ident == record['cutex_session_id'] and native == record['codex_session_id']
assert record['formal_agent_name'] == intent['request']['spec']['name']
assert roster['agents'][ident]['project_id'] == intent['request']['project_id']
assert record['explicit_launch']['native_id'] == native
adoptions = [v['receipt'] for v in sessions['explicit_launch_receipts'].values()
             if v['kind'] == 'bootstrap' and v['receipt']['cutex_session_id'] == ident]
assert len(adoptions) == 1 and adoptions[0]['native_id'] == native
paths = list((conf / 'codex-home/sessions').glob('**/*'+native+'.jsonl'))
assert len(paths) == 1
rows = [json.loads(line) for line in paths[0].read_text().splitlines()]
meta = [r['payload'] for r in rows if r['type'] == 'session_meta']
assert len(meta) == 1 and meta[0]['id'] == native
assert 'cutex-top-level-session' not in json.dumps(meta)
users = [r for r in rows if r['type'] == 'response_item' and r['payload'].get('role') == 'user'
         and any(c.get('text')=='Private Human input after neutral Management create'
                 for c in r['payload'].get('content',[]))]
assert len(users) == 1
assert len([r for r in rows if r['type']=='turn_context'])==1
assert 'Private Human input after neutral Management create' in json.dumps(users[0])
if not stage_only:
    assert result['neutral_model_calls'] == 0 and result['cli_model_calls'] == 1
requests = json.loads((run/'model-requests.json').read_text())
assert len(requests)==1 and requests[0].get('prompt_cache_key')==native
terminal = json.loads((run / 's8a-created-attach.exit.json').read_text())
if not stage_only:
    assert terminal == {'exit': 0, 'termios_restored': True}
print(json.dumps({'native': native, 'durable': ident, 'adoption_receipts': 1,
                  'native_files': 1, 'human_turns': 1, 'neutral_model_calls': 0,
                  'source': meta[0].get('source'), 'terminal': terminal,
                  'boundary':'stage/history only; not terminal acceptance' if stage_only else 'complete fixture'}))
