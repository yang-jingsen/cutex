"""Independent subprocess/filesystem oracle; no services, model or live writes."""
import hashlib,json,os,subprocess,sys
from pathlib import Path

root=Path(sys.argv[1]).resolve(strict=True)
source=Path(__file__).resolve().parents[1]
script=source/'scripts/selected_history_rehearsal.py'
plan=root/'plan.json'
def run(destination):
    return subprocess.run([sys.executable,'-B',str(script),'apply',str(plan),str(destination)],capture_output=True)
facts=[]
existing=root/'existing';existing.mkdir(mode=0o700)
sentinel=existing/'keep';sentinel.write_bytes(b'untouched')
assert run(existing).returncode!=0 and sentinel.read_bytes()==b'untouched'
facts.append('existing destination refuses without overwrite')
link=root/'linked';link.symlink_to(existing,target_is_directory=True)
assert run(link).returncode!=0 and list(existing.iterdir())==[sentinel]
facts.append('symlink destination refuses')
link_parent=root/'linked-parent';link_parent.symlink_to(existing,target_is_directory=True)
assert run(link_parent/'child').returncode!=0 and not (existing/'child').exists()
facts.append('symlink parent refuses before create')
partial=root/'interrupted'
code="import sys,json;sys.path.insert(0,sys.argv[1]);import selected_history_rehearsal as m;m.apply(json.loads(m.read(m.Path(sys.argv[2]))),m.Path(sys.argv[3]),interrupt_after=1)"
p=subprocess.run([sys.executable,'-B','-c',code,str(script.parent),str(plan),str(partial)],capture_output=True)
assert p.returncode!=0 and b'test interruption' in p.stderr
assert (partial/'plan.json').exists() and not (partial/'prepared.json').exists()
before={str(p.relative_to(partial)):hashlib.sha256(p.read_bytes()).hexdigest() for p in partial.rglob('*') if p.is_file()}
assert run(partial).returncode!=0
after={str(p.relative_to(partial)):hashlib.sha256(p.read_bytes()).hexdigest() for p in partial.rglob('*') if p.is_file()}
assert before==after
facts.append('interrupted preparation retained; replay refuses and preserves bytes')
(root/'filesystem-oracle.json').write_text(json.dumps({'pass':facts,'unexpected_runtime_attempts':0},indent=2))
print('Filesystem subprocess oracle:',len(facts),'passed')
