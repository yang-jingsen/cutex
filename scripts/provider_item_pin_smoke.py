"""Model-free final-byte readiness check in a NEW private guest fixture.

Reuses the frozen private root/provider setup, not the Job/model campaign.
No auth copying, real profile, turn/start, held retry or existing-owner access.
"""
import hashlib
from pathlib import Path
import sys

root = Path(__file__).resolve().parents[1]
base = root / 'fixtures/base-fixture.py'
assert hashlib.sha256(base.read_bytes()).hexdigest() == 'ed1ebdb57d413db2f3f382aa65c1bf014e4ccb8b8483b63dd85e774717d9728e'
assert len(sys.argv) == 2 and sys.argv[1].isalnum()
run = root / sys.argv[1]
assert not run.exists()
assert len(str(run / 'h/.cutex/runtime/app-server/000000000000/s').encode()) <= 100
source = base.read_text()


def replace(old, new):
    global source
    assert source.count(old) == 1, old
    source = source.replace(old, new)


replace("STOCK = FROZEN/'native/bin/codex'", "STOCK = ROOT/'native/bin/codex'")
replace("CUTEX = FROZEN/'package/artifacts/linux/cutex'", "CUTEX = ROOT/'bin/cutex'")
replace("MCP = FROZEN/'package/artifacts/linux/cutex-mcp'", "MCP = ROOT/'bin/cutex-mcp'")
replace("PATCHED = FROZEN / 'native/bin/codex-app-server'", "PATCHED = ROOT / 'native/bin/codex-app-server'")
replace("SCHEMA = FROZEN / 'native/schema.json'", "SCHEMA = ROOT / 'native/schema/codex_app_server_protocol.schemas.json'")
replace("9caa26abe4ec3094543b402e235654f8434d09e49e14017b856b4cdac0801bde", "cac03d6d1b77e4d3681a2d71b25435a537bc257807c28ea0f5aff491c622b69d")
replace("0c425b5f9fca90835fd2b4377a1bca212532f66c", "ca580a783fc1ab34613be4f81ceab96ef4d393a2")
replace('range(24800,24999)', 'range(24900,24999)')
replace("        assert self.path == '/v1/responses'", "        raise AssertionError('MODEL REQUEST FORBIDDEN in readiness-only probe')")
start = source.index('    from job_mcp_helper_r2 import run_job')
end = source.index('\nfinally:', start)
source = source[:start] + '''    rpc, init = native_rpc(current)
    assert init.get('externalInputVersion') == 1
    assert 'soon' in init.get('externalInputDeliveries', [])
    history = rpc.call('thread/read', {'threadId': thread, 'includeTurns': True})
    assert history['thread']['turns'] == []
    assert current['stage'] == 'ready' and Model.calls == 0
    record = store()['sessions'][durable]
    assert record['codex_session_id'] == thread
    assert record['app_server_runtime'] == current['binding']
    result = {'stage': current['stage'], 'durable': durable, 'native': thread,
              'generation': record['runtime_generation'], 'modelCalls': Model.calls,
              'cutex': sha(CUTEX), 'facade': sha(MCP), 'nativeServer': sha(PATCHED),
              'nativeCli': sha(STOCK), 'host': sha(PATCHED.with_name('codex-code-mode-host')),
              'schema': sha(SCHEMA), 'rootActivationReplay': True, 'nonrootActivationDenied': True,
              'boundary': 'real private provider/readiness; fake config; no model/Job/CLI interaction'}
    (RUN / 'RESULT.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result), flush=True)
''' + source[end:]
exec(compile(source, str(base), 'exec'), dict(__file__=str(base), __name__='__main__'))
