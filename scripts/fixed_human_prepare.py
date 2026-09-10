"""Worker-run preparation only. Never submits a turn or Job. Retains success.
Uses one new fixture and already-transferred fixed bytes; not a Human launcher.
"""
import ast, hashlib, json, os, sys, time, threading, subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
RUN = ROOT / 'h1'
staged_auth = ROOT / 'staged-auth.json'
assert not RUN.exists() and staged_auth.is_file() and not staged_auth.is_symlink()
assert staged_auth.stat().st_mode & 0o077 == 0
os.umask(0o077)
MODE = 'real'
helper = Path(__file__).with_name('reviewed_job_vm.py')
nodes = [n for n in ast.parse(helper.read_text()).body if isinstance(n, ast.FunctionDef) and n.name in ('record_digest', 'prepared_launch')]
assert len(nodes) == 2
exec(compile(ast.Module(body=nodes, type_ignores=[]), str(helper), 'exec'), globals())


def handoff(g):
    rpc, init = g['native_rpc'](g['current'])
    resumed = rpc.call('thread/resume', {'threadId': g['thread']})
    assert resumed['model'] == 'gpt-5.6-terra'
    config = g['current']['review']['configuration']
    assert (config['reasoning'], config['sandbox'], config['approval']) == ('low', 'read-only', 'on-request')
    assert rpc.call('thread/read', {'threadId': g['thread'], 'includeTurns': True})['thread']['turns'] == []
    servers = []; cursor = None
    for _ in range(20):
        params = {'threadId': g['thread'], 'detail': 'toolsAndAuthOnly', 'limit': 1}
        if cursor: params['cursor'] = cursor
        page = rpc.call('mcpServerStatus/list', params)
        servers += [s for s in page['data'] if s['name'] == 'cutex_job']
        cursor = page.get('nextCursor')
        if cursor is None: break
    assert len(servers) == 1 and servers[0]['runtimeStatus'] == 'connected'
    tools = servers[0]['tools']
    assert any(t['name'] == 'submit' and t.get('inputSchema') for t in tools.values())
    assert any(t['name'] == 'read_output' and t.get('inputSchema') for t in tools.values())
    def identity(pid):
        return {'pid': pid, 'start_ticks': g['process_identity'](pid)[0],
                'exe': str(Path(f'/proc/{pid}/exe').readlink()), 'pgid': os.getpgid(pid)}
    record = {'root': str(RUN), 'durable': g['durable'], 'native': g['thread'],
              'bus_port': g['bp'], 'management_port': g['mp'],
              'runtime': identity(g['current']['binding']['pid']),
              'services': [identity(p.pid) for p in g['children'] if p.poll() is None],
              'model': resumed['model'], 'reasoning': config['reasoning'],
              'sandbox': config['sandbox'], 'approval': config['approval'],
              'job_tool_names': sorted(t['name'] for t in tools.values()),
              'neutral_turns': 0, 'generation': g['store']()['sessions'][g['durable']]['runtime_generation']}
    (RUN / 'handoff.json').write_text(json.dumps(record, indent=2))
    # Detached read-only observer, no model/approval/retry operations.
    observer = g['owner'](['/usr/bin/python3', '-B', ROOT/'fixtures/fixed_human_entry.py', 'observe'], 'observer')
    until = time.monotonic()+45
    while not (RUN/'observer-ready').exists():
        assert observer.poll() is None and time.monotonic()<until, 'observer not ready'
        threading.Event().wait(.05)
    record['observer'] = identity(observer.pid)
    (RUN/'handoff.json').write_text(json.dumps(record, indent=2))
    assert g['Model'].calls == 0
    g['keep_success'] = True
    print(json.dumps({'ready': True, 'durable': record['durable'], 'native': record['native'],
                      'model': record['model'], 'reasoning': 'low', 'permissions': 'read-only/on-request',
                      'jobTools': record['job_tool_names'], 'neutralTurns': 0}), flush=True)


base = ROOT/'fixtures/base-fixture.py'
source = base.read_text()
assert hashlib.sha256(source.encode()).hexdigest() == 'ed1ebdb57d413db2f3f382aa65c1bf014e4ccb8b8483b63dd85e774717d9728e'
for old,new in [
 ("STOCK = FROZEN/'native/bin/codex'", "STOCK = ROOT/'native/bin/codex'"),
 ("CUTEX = FROZEN/'package/artifacts/linux/cutex'", "CUTEX = ROOT/'bin/cutex'"),
 ("MCP = FROZEN/'package/artifacts/linux/cutex-mcp'", "MCP = ROOT/'bin/cutex-mcp'"),
 ("PATCHED = FROZEN / 'native/bin/codex-app-server'", "PATCHED = ROOT / 'native/bin/codex-app-server'"),
 ("SCHEMA = FROZEN / 'native/schema.json'", "SCHEMA = ROOT / 'native/schema/codex_app_server_protocol.schemas.json'"),
 ('9caa26abe4ec3094543b402e235654f8434d09e49e14017b856b4cdac0801bde', 'cac03d6d1b77e4d3681a2d71b25435a537bc257807c28ea0f5aff491c622b69d'),
 ('0c425b5f9fca90835fd2b4377a1bca212532f66c','ca580a783fc1ab34613be4f81ceab96ef4d393a2'),
 ('range(24800,24999)','range(24900,24999)'),
 ('profile_id = str(uuid.uuid4())', "profile_id = 'cd6a39eb-3997-45c6-9824-5113fe36a4b8'"),
 ("'alpha'", "'aemeath'"),
 ('unknown-private-model','gpt-5.6-terra'),
 ("'--permission', 'full-access', '--sandbox', 'danger-full-access'", "'--permission', 'read-only', '--sandbox', 'read-only'"),
 ("'sandbox': 'danger-full-access'", "'sandbox': 'read-only'"),
 ("env=env,cwd=RUN,\n                       stdout=log", "env=({k:v for k,v in env.items() if k not in ('LD_PRELOAD','S4_TEST_ALLOWED_PORTS')} if str(args[0])==str(PATCHED) else env),cwd=RUN,\n                       stdout=log"),
]:
    assert old in source, old
    source=source.replace(old,new)
lines=source.splitlines()
for i,line in enumerate(lines):
    if "(folder / 'config.toml').write_text" in line:
        lines[i]="    (folder / 'config.toml').write_text(\"cutex_provider_mode='aemeath_chatgpt_v1'\\nmodel='gpt-5.6-terra'\\nmodel_provider='openai'\\nmodel_reasoning_effort='low'\\n\")"
    if line.startswith('    args = [PATCHED,'):
        lines[i]="    args = [PATCHED,'-c','model=\"gpt-5.6-terra\"','-c','model_reasoning_effort=\"low\"','-c','cli_auth_credentials_store=\"file\"','-c','default_permissions=\":read-only\"','--disable-plugin-startup-tasks-for-tests','--listen','unix://'+str(sock)]"
    if line=="    sock = RUN / 'bootstrap.sock'":
        lines[i]="    NATIVE.chmod(0o700)\n    import shutil\n    shutil.copyfile(staged_auth,NATIVE/'auth.json')\n    (NATIVE/'auth.json').chmod(0o600)\n    staged_auth.unlink()\n"+line
source='\n'.join(lines)+'\n'
start=source.index("    current=launch(durable,'vm-job-subscriber')")
end=source.index('\nfinally:',start)
source=source[:start]+"    current=prepared_launch(globals())\n    handoff(globals())\n"+source[end:]
# Successful preparation intentionally leaves only owned services/runtime alive.
start=source.index('    if durable:\n',source.index('\nfinally:'))
end=source.index('    for log in logs:',start)
source=source[:start]+"    if not globals().get('keep_success',False):\n"+'\n'.join('    '+l for l in source[start:end].splitlines())+'\n'+source[end:]
sys.argv=[sys.argv[0],'h1']
try:
    exec(compile(source,str(base),'exec'),dict(globals(),__file__=str(base)))
except Exception as error:
    import traceback
    print(json.dumps({'failed':type(error).__name__, 'frames':[
        {'file':Path(f.filename).name,'line':f.lineno,'function':f.name}
        for f in traceback.extract_tb(error.__traceback__)]}),flush=True)
    sys.exit(1)
