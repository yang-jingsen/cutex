"""Bounded default-byte composition; reuse original private fixture unchanged.

Real Cutex/provider/native/PTY, authenticated Job producer fixture, fake model.
No Job subprocess or paid provider claim. Native p6/p7 cover compact MCP and
adjacent grouping on these exact CLI bytes; this probe does not force Job timing.
"""
from pathlib import Path

_composition_path = Path(__file__).with_name('presentation_boundary.py')
_composition_source = _composition_path.read_text()
_composition_changes = [
    ('artifacts/visible-only-tui-r1/final-bin', 'artifacts/display-polish-v2-r1/final-bin-v2'),
    ('artifacts/presentation-job-r1/default-bin', 'artifacts/display-v2-composition-r1/default-bin'),
    ("'3d8a73a747cf5b957a7ca0491c28d1517f6d7722'", "'cc4a080df1df4433f6fd67fee9c1c4fa4a42baab'"),
    ("config['private_job_presentation']={'version':1,'recipients':[durable]}",
     "config['private_job_presentation']={'version':2,'recipients':[durable],'template':'service_summary'}"),
    ("terminal.wait('输出读取状态')", "terminal.wait('SERVICE_DISPLAY_BODY')"),
    ("del config['private_job_presentation']", "config['private_job_presentation']={'version':2,'recipients':[durable]}"),
    ('28cbc9a64460db504fe842b787695144e14a90b7f939b7782dd0133d6e45d54c',
     'c0c8d965f39645374ad0c7f655f10bd15f0640232bf3d81d059d627e91cabfc3'),
]
for _old, _new in _composition_changes:
    assert _old in _composition_source, _old
    _composition_source = _composition_source.replace(_old, _new)
_hook = "    control=Control(1);"
assert _hook in _composition_source
_composition_source = _composition_source.replace(_hook, '''    inventory=r.call('mcpServerStatus/list',{'threadId':thread,'detail':'toolsAndAuthOnly','limit':100})
    servers=[s for s in inventory['data'] if s['name']=='cutex']
    assert len(servers)==1 and servers[0]['runtimeStatus']=='connected'
    assert servers[0]['tools'] and all(t.get('inputSchema') for t in servers[0]['tools'].values())
    (RUN/'configured-mcp.json').write_text(json.dumps({'connected':True,'toolNames':sorted(t['name'] for t in servers[0]['tools'].values()),'boundary':'actual configured discovery, not model tool invocation'}))
''' + _hook)
_hook = "    code,replay=api(bp,'/api/job-service/v1/completions',request,token=token);"
assert _hook in _composition_source
_composition_source = _composition_source.replace(_hook, '''    assert receipt['presentation']['title']=='Job summary'
    assert receipt['presentation']['body']=='SERVICE_DISPLAY_BODY'
    assert completed['presentation']['version']==2
''' + _hook)
exec(compile(_composition_source, str(_composition_path), 'exec'), dict(globals(), __file__=str(_composition_path)))
