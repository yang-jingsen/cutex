"""Run existing bridge tests each in a separate, explicitly marked private HOME.
The legacy tests reuse message IDs and a process-global repository singleton.
"""
import json,os,subprocess,tempfile
from pathlib import Path
root=Path(__file__).resolve().parents[2]
assert Path(os.environ['TMPDIR']).is_relative_to(root)
result=subprocess.run(['cargo','test','--lib','--no-run','--message-format=json'],capture_output=True,text=True,check=True)
artifacts=[json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
binary=next(row['executable'] for row in artifacts if row.get('reason')=='compiler-artifact' and row.get('executable') and row['profile']['test'])
listed=subprocess.check_output([binary,'--list'],text=True)
names=[line.removesuffix(': test') for line in listed.splitlines() if line.startswith('app_server::bus_bridge::') and line.endswith(': test')]
assert names
failures=[]
for name in names:
    home=Path(tempfile.mkdtemp(prefix='s6c2-unit-',dir=root/'tmp'))
    (home/'.cutex-test-private-home').write_text('private bridge regression fixture\n')
    env={**os.environ,'HOME':str(home),'CUTEX_TEST_PRIVATE_HOME':str(home)}
    run=subprocess.run([binary,'--exact',name,'--nocapture'],env=env,capture_output=True,text=True,timeout=60)
    (home/'result.log').write_text(run.stdout+run.stderr)
    print(json.dumps({'test':name,'exit':run.returncode,'fixture':str(home)}),flush=True)
    if run.returncode:failures.append(name)
print(json.dumps({'selected':len(names),'passed':len(names)-len(failures),'failed':failures}))
assert not failures
