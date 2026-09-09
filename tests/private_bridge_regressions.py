"""Run each bridge unit regression in a distinct verified private process HOME.
The repository is process-global; sharing one test HOME aliases fixed fixture IDs.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

root=Path(__file__).resolve().parents[2]
binary=Path(sys.argv[1]).resolve()
assert binary.is_file() and binary.is_relative_to(root/'target/debug/deps')
evidence=root/sys.argv[2]
assert evidence.parent==root and not evidence.exists()
evidence.mkdir(mode=0o700)
base={'PATH':'/usr/bin:/bin','HOME':str(root/'home'),'TMPDIR':str(root/'tmp')}
listed=subprocess.run([str(binary),'--list'],env=base,capture_output=True,text=True,check=True)
names=[line.split(': test')[0] for line in listed.stdout.splitlines() if line.startswith('app_server::bus_bridge::') and line.endswith(': test')]
assert names
results=[]
for n,name in enumerate(names):
    home=evidence/str(n);home.mkdir(mode=0o700)
    (home/'.cutex-test-private-home').write_text('S8b isolated unit process\n')
    env=dict(base,HOME=str(home),CUTEX_TEST_PRIVATE_HOME=str(home))
    result=subprocess.run([str(binary),name,'--exact','--nocapture'],env=env,cwd=root/'source',capture_output=True,text=True,timeout=120)
    (home/'test.log').write_text(result.stdout+result.stderr)
    results.append({'test':name,'exit':result.returncode,'nonzero_selection':'1 passed' in result.stdout})
(evidence/'result.json').write_text(json.dumps(results))
failed=[r for r in results if r['exit'] or not r['nonzero_selection']]
print(json.dumps({'selected':len(results),'failed':failed}))
assert not failed
