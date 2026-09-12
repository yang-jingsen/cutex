"""Exact default-byte private namespace; no live service or credential access."""
import ast, hashlib, json, os, pathlib, subprocess, sys, tempfile
source=pathlib.Path(__file__).resolve().parents[1]; root=source.parent
manifest=pathlib.Path(sys.argv[1]).resolve(strict=True)
assert manifest.is_relative_to(root/'artifacts')
assert hashlib.sha256(manifest.read_bytes()).hexdigest()==sys.argv[2]
data=json.loads(manifest.read_text())
for rel,digest in data['files'].items():
    assert hashlib.sha256((manifest.parent/rel).read_bytes()).hexdigest()==digest
inputs=root/'rehearsal-r2-db5WwG/materialized'
plan=json.loads((inputs/'plan.json').read_text())
assert len(plan['rows'])==34
row=next(r for r in plan['rows'] if r['formal_name']=='cutex-director-r13')
ast.parse((source/'tests/same_id_maintenance_composition.py').read_text())
assert os.statvfs(root).f_bavail*os.statvfs(root).f_frsize>=100*1024**3
usage=int(subprocess.check_output(['du','-sb',str(root)]).split()[0])
assert usage<=24*1024**3,usage
fixture=pathlib.Path(tempfile.mkdtemp(prefix='same-id-',dir=root));os.chmod(fixture,0o700)
cwd=fixture/'workspace';cwd.mkdir(mode=0o700)
cmd=['/usr/bin/bwrap','--die-with-parent','--unshare-net','--unshare-pid','--tmpfs','/']
for p in ['/usr','/bin','/lib','lib64','/etc','/mnt']:
    p='/'+p.lstrip('/');cmd+=['--ro-bind',p,p]
cmd+=['--bind',str(fixture),'/p','--ro-bind',str(inputs),'/p/input',
      '--bind',str(cwd),row['cwd'],'--tmpfs','/tmp','--proc','/proc','--dev','/dev',
      '--ro-bind','/home/senxiu/.cutex/codex-home/skills','/home/senxiu/.cutex/codex-home/skills',
      '--chdir','/p','/usr/bin/python3','-B',str(source/'tests/same_id_maintenance_composition.py'),
      str(manifest.parent/'bin'),str(manifest.parent/'fixture/seed-tests')]
env={'PATH':'/usr/bin:/bin','HOME':'/p/h','TMPDIR':'/p','SELECTED_COMPOSITION_PRIVATE':'1',
     'TERM':'xterm-256color','LANG':'C.UTF-8'}
with (fixture/'runner.log').open('wb') as log:
    result=subprocess.run(cmd,env=env,stdout=log,stderr=subprocess.STDOUT,timeout=1200)
print('Retained same-ID private fixture:',fixture,'exit:',result.returncode,flush=True)
sys.exit(result.returncode)
