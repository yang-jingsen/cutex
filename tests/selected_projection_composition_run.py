"""Own namespace wrapper; one fresh evidence root per explicitly authorized run."""
import os, pathlib, subprocess, sys, tempfile
root=pathlib.Path('/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1')
binary=pathlib.Path(sys.argv[1]).resolve(strict=True)
assert binary.is_relative_to(root/'artifacts')
fixture=pathlib.Path(tempfile.mkdtemp(prefix='selected-r3-',dir=root));os.chmod(fixture,0o700)
script=pathlib.Path(__file__).with_name('selected_projection_composition.py').resolve()
cmd=['/usr/bin/bwrap','--die-with-parent','--unshare-net','--unshare-pid','--tmpfs','/']
for p in ['/usr','/bin','/lib','/lib64','/etc','/mnt']:
    cmd+=['--ro-bind',p,p]
cmd+=['--bind',str(fixture),'/p','--tmpfs','/tmp','--proc','/proc','--dev','/dev','--chdir','/p','/usr/bin/python3','-B',str(script),str(binary)]
env={'PATH':'/usr/bin:/bin','HOME':'/p/h','TMPDIR':'/p','SELECTED_COMPOSITION_PRIVATE':'1','TERM':'xterm-256color','LANG':'C.UTF-8'}
with (fixture/'runner.log').open('wb') as log:
    result=subprocess.run(cmd,env=env,stdout=log,stderr=subprocess.STDOUT,timeout=600)
print('Retained private composition:',fixture,'exit:',result.returncode,flush=True)
sys.exit(result.returncode)
