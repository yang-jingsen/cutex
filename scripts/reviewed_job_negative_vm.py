"""Negative-only stdio fault client against an already Ready owned VM fixture.

Not Core/model proof: deliberately malformed metadata must NEVER create a Job.
Uses the real adapter/grant issuer/Bus, no signed grants or positive submission.
The separate reviewed_job_vm.py provides actual Core automatic-metadata proof.
"""
import json,os,stat,subprocess,sys,hashlib
from pathlib import Path
root=Path(__file__).resolve().parents[1]
run=root/sys.argv[1]
assert run.parent==root and run.resolve()==run and run.stat().st_uid==os.getuid()
home=run/'h';conf=home/'.cutex'
config=json.loads((conf/'config.json').read_text())
receipt=json.loads((run/'launch-receipt.json').read_text())
review=receipt['review'];descriptor=review['job_mcp']['descriptor']
thread=review['contract']['native_id']
binary=root/'bin/cutex-job-service'
assert hashlib.sha256(binary.read_bytes()).hexdigest()=='d98d5e33322c7200eb9b149bc39d99da3bb169c994fe14c7f401d26c06d7b1f2'
env={'PATH':'/usr/bin:/bin','HOME':str(home),'CODEX_HOME':str(conf/'codex-home'),'TMPDIR':str(root/'tmp'),
     'CUTEX_TEST_PRIVATE_HOME':str(home),'CUTEX_AGENT_ID':receipt['runtime_agent_id'],
     'CUTEX_AGENT_BUS_URL':f"http://127.0.0.1:{config['agent_bus_port']}",'CUTEX_AGENT_BUS_TOKEN':config['agent_bus_token'],
     'LD_PRELOAD':str(root.parent/'vm-r1/connect-guard.so'),'S4_TEST_ALLOWED_PORTS':str(config['agent_bus_port'])}
args=[str(binary),'mcp-stdio',descriptor['endpoint'],descriptor['api_token_file'],descriptor['grant_key_file'],descriptor['launcher']['path']]
sandbox={'permissionProfile':{'type':'disabled'},'codexLinuxSandboxExe':None,'sandboxCwd':run.as_uri(),'useLegacyLandlock':False}
meta={'threadId':thread,'codex/sandbox-state-meta':sandbox}
request={'actionId':'negative-only-never-create','argv':['/bin/true'],'cwd':str(run)}
state=run/'job-state/state.json'
def jobs():return set(json.loads(state.read_text())['jobs']) if state.exists() else set()
before=jobs();facts=[]
def denied(label,arguments,metadata,needle,command=args,child_env=env):
    messages=[{'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2025-06-18'}},
              {'jsonrpc':'2.0','id':2,'method':'tools/call','params':{'name':'submit','arguments':arguments,'_meta':metadata}}]
    p=subprocess.run(command,input=''.join(json.dumps(v)+'\n' for v in messages),text=True,capture_output=True,env=child_env,cwd=run,timeout=35)
    assert p.returncode==0,(label,p.returncode)
    response=[json.loads(v) for v in p.stdout.splitlines()][-1]
    assert 'error' in response and needle in response['error']['message'],(label,response)
    assert jobs()==before
    facts.append({'case':label,'error':response['error']})
    (run/'negative-adapter.json').write_text(json.dumps({'boundary':'negative-only malformed transport; not Core metadata success','facts':facts},indent=2))
denied('missing-core-metadata',request,None,'metadata is missing')
denied('foreign-thread',request,{**meta,'threadId':'00000000-0000-4000-8000-000000000000'},'ambiguous or stale')
denied('external-policy',request,{**meta,'codex/sandbox-state-meta':{**sandbox,'permissionProfile':{'type':'external'}}},'external sandbox authority cannot be reproduced')
denied('forged-subject',{**request,'subscriberCutexSessionId':'forged'},meta,'unknown field')
denied('empty-command',{**request,'argv':[]},meta,'argv')
wrong=home/'negative-wrong-api'
fd=os.open(wrong,os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600)
with os.fdopen(fd,'wb') as f:f.write(os.urandom(32))
bad_args=list(args);bad_args[3]=str(wrong)
denied('wrong-api-credential',request,meta,'credential',command=bad_args)
denied('missing-occurrence',request,meta,'registered',child_env={**env,'CUTEX_AGENT_ID':'stock.00000000-0000-4000-8000-000000000000'})
print(json.dumps({'negativeCases':len(facts),'noJobCreated':True}))
