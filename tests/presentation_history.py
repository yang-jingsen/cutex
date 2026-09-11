"""Read-only independent persisted-record oracle for one owned fake fixture."""
import json
import sys
from pathlib import Path

root=Path(__file__).resolve().parents[2]
run=root/sys.argv[1]
assert run.parent==root and (run/'h/.cutex-test-private-home').is_file()
state=json.loads((run/'h/.cutex/runtime/management-v2/agent-bus-message-state.json').read_text())
messages=[m['snapshot'] for m in state['messages'].values() if m['snapshot'].get('presentation')]
assert len(messages)==1 and state['version']==5
m=messages[0];p=m['presentation']['receipt'];a4=m['externalInputReceipt']
assert p and a4 and m['state']=='delivered'
files=list((run/'h/.cutex/codex-home/sessions').rglob('*.jsonl'))
assert len(files)==1
rows=[json.loads(line) for line in files[0].read_text().splitlines()]
displays=[r['payload'] for r in rows if r['type']=='event_msg' and r['payload'].get('type')=='presentation_appended']
assert len(displays)==2
assert sum({k:v for k,v in r.items() if k!='type'}==p for r in displays)==1
assert sum(r['presentation']['id']=='private-idle' for r in displays)==1
commits=[r['payload']['fact']['commit'] for r in rows if r['type']=='external_input' and r['payload']['fact']['phase']=='commit']
assert sum(c['receipt']==a4 for c in commits)==1
requests=json.loads((run/'model-requests.json').read_text())
assert requests and all('PRIVATE_VISIBLE_SENTINEL' not in json.dumps(r) and '输出读取状态' not in json.dumps(r,ensure_ascii=False) for r in requests)
print(json.dumps({'fixture':run.name,'display_records':2,'original_a4_commit_count':1,'same_display_receipt_count':1,'display_only_text_excluded':True,'model':'local fake Responses'},ensure_ascii=False))
