"""Observed Worker-only preparation for pv1; no turn or Job submission.

Reuse the proven neutral/bootstrap and reviewed Job setup. This wrapper only
selects the accepted new composition and enables its explicit private recipient.
"""
from pathlib import Path
import sys

path = Path(__file__).with_name('fixed_human_prepare.py')
source = path.read_text()
changes = [
    ('cac03d6d1b77e4d3681a2d71b25435a537bc257807c28ea0f5aff491c622b69d',
     'df95936f3f0d1ff62efb978fee32b85e45da0f7f3166de7606cce3b49fb12018'),
    ('ca580a783fc1ab34613be4f81ceab96ef4d393a2',
     '3d8a73a747cf5b957a7ca0491c28d1517f6d7722'),
    ('range(24900,24999)', 'range(24920,24999)'),
]
for old, new in changes:
    assert source.count(old) == 1
    source = source.replace(old, new)
# Inject into the base text before its model-free setup executes, not into a
# running shared config. The exact adopted ID is available before service start.
at = "lines=source.splitlines()"
assert source.count(at) == 1
source = source.replace(at, '''source=source.replace('459861225d5bfb73bb4c3896edb489169637424be410be346f955a39596da7e9', '77e75b7fc47c9b8a9caacf5a7d31c040c6520679a9833f1c2b0e55937ed39c27')
source=source.replace("    durable = ids[0]", "    durable = ids[0]\\n    cfg=json.loads((CONF/'config.json').read_text())\\n    cfg['private_job_presentation']={'version':1,'recipients':[durable]}\\n    (CONF/'config.json').write_text(json.dumps(cfg))")
''' + at)
if sys.argv[1:] == ['resume-port-preflight']:
    # One diagnosed pre-service failure: the adopted identity exists, no claim
    # or owner was launched. Resume that exact state; never create another ID.
    source = source.replace('assert not RUN.exists() and staged_auth.is_file() and not staged_auth.is_symlink()',
                            "assert RUN.is_dir() and (RUN/'h/.cutex/cutex-sessions.json').is_file()")
    source = source.replace('assert staged_auth.stat().st_mode & 0o077 == 0', 'pass')
    hook = "sys.argv=[sys.argv[0],'h1']"
    source = source.replace(hook, '''source=source.replace('assert RUN.parent == ROOT and not RUN.exists()', 'assert RUN.parent == ROOT and RUN.is_dir()')
source=source.replace('RUN.mkdir(mode=0o700)', 'pass')
source=source.replace('HOME.mkdir()', 'None').replace('CONF.mkdir()', 'None').replace('NATIVE.mkdir()', 'None')
begin=source.index('    bp = port()', source.index('\\ntry:'))
end=source.index('    bus = owner(', begin)
source=source[:begin]+"""    records=store()['sessions']
    assert len(records)==1
    durable=next(iter(records)); record=records[durable]
    assert record.get('runtime_generation',0)==0 and not record.get('app_server_runtime') and not record.get('app_server_launch_claim_id') and not record.get('explicit_launch')
    thread=record['codex_session_id'];ids=[durable];threads=[thread]
    bp=port();env['S4_TEST_ALLOWED_PORTS']=f'{bp},{model.server_port}'
    cfg=json.loads((CONF/'config.json').read_text());cfg['agent_bus_port']=bp
    assert cfg['private_job_presentation']['recipients']==[durable]
    (CONF/'config.json').write_text(json.dumps(cfg))
"""+source[end:]
''' + hook)
else:
    assert not sys.argv[1:]
exec(compile(source, str(path), 'exec'), dict(globals(), __file__=str(path)))
