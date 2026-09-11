"""Fresh private namespace and exact prebuilt bytes; never live endpoints."""
import hashlib,json,os,pathlib,subprocess,sys,tempfile
source=pathlib.Path(__file__).resolve().parents[1];root=source.parent
input_root=pathlib.Path(sys.argv[1]).resolve(strict=True)
assert input_root.is_relative_to(root) and (input_root/'prepared.json').is_file()
plan=json.loads((input_root/'plan.json').read_text())
manifest=pathlib.Path(sys.argv[2]).resolve(strict=True) if len(sys.argv)>2 else root/'artifacts/selected-projection-r3/build-manifest.json'
expected=sys.argv[3] if len(sys.argv)>3 else '3bc284978dc2e5cb61c3b1021ca24bd87c3adbe7f338dceeada97d92786098e3'
assert manifest.is_relative_to(root/'artifacts')
assert hashlib.sha256(manifest.read_bytes()).hexdigest()==expected
for rel,sha in json.loads(manifest.read_text())['files'].items():assert hashlib.sha256((manifest.parent/rel).read_bytes()).hexdigest()==sha
mode=sys.argv[4] if len(sys.argv)>4 else 'all'
assert mode in ('all','registry','probe:cesc-tutor-r1','probe:cute-codex-log-wal-fix-r2','probe:tethys-director-r2','probe:ifm-ema-figures','probe:scpolya-2')
fixture=pathlib.Path(tempfile.mkdtemp(prefix='history-r3-',dir=root));os.chmod(fixture,0o700)
(fixture/'prerequisite').mkdir(mode=0o700)
cmd=['/usr/bin/bwrap','--die-with-parent','--unshare-net','--unshare-pid','--tmpfs','/']
for p in ['/usr','/bin','/lib','/lib64','/etc','/mnt']:cmd+=['--ro-bind',p,p]
cmd+=['--bind',str(fixture),'/p','--ro-bind',str(input_root),'/p/input','--tmpfs','/tmp','--proc','/proc','--dev','/dev']
# Preserve exact cwd strings in an empty private filesystem, not host workspaces.
work=fixture/'workspaces';work.mkdir(mode=0o700)
for index,cwd in enumerate(sorted({r['cwd'] for r in plan['rows']},key=lambda s:(len(pathlib.Path(s).parts),s))):
    assert cwd.startswith(('/home/senxiu/Projects/','/mnt/mambo/Projects/')) and '..' not in pathlib.Path(cwd).parts
    empty=work/str(index);empty.mkdir(mode=0o700)
    cmd+=['--bind',str(empty),cwd]
skills='/home/senxiu/.cutex/codex-home/skills'
cmd+=['--ro-bind',skills,skills,'--chdir','/p','/usr/bin/python3','-B',str(source/'tests/selected_history_composition.py'),str(manifest.parent/'bin'),mode]
env={'PATH':'/usr/bin:/bin','HOME':'/p/h','TMPDIR':'/p','SELECTED_COMPOSITION_PRIVATE':'1','TERM':'xterm-256color','LANG':'C.UTF-8'}
with (fixture/'runner.log').open('wb') as log:
    result=subprocess.run(cmd,env=env,stdout=log,stderr=subprocess.STDOUT,timeout=1800)
print('Retained private history run:',fixture,'exit:',result.returncode,flush=True)
sys.exit(result.returncode)
