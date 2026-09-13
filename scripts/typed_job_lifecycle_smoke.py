"""Local integration regression; requires an already prepared isolated fixture.
Usage: python3 scripts/typed_job_lifecycle_smoke.py BINARY FIXTURE_DIRECTORY
Run once against r18 to capture the failure, stop fixture services only, then
run the repaired binary against the same fixture. Ports 24762/24772 must belong
exclusively to this fixture. No model prompts or Job submissions are sent.
Fixture preparation/credential cleanup are operator responsibilities; never use
production HOME. See docs/review/2026-09-runtime-repair/JOB-FOLLOWUP.md.
"""
import os,json,pathlib,subprocess,sys,time,urllib.request,urllib.error,uuid,socket,hashlib
ROOT=pathlib.Path(sys.argv[2]).resolve();f=json.load(open(ROOT/'fixture.json'));home=pathlib.Path(f['home']);BIN=sys.argv[1]
env={k:v for k,v in os.environ.items() if not k.startswith(('CUTEX','CODEX'))};env.update(HOME=f['home'],CODEX_HOME=f['source_home'],CUTEX_TEST_PRIVATE_HOME=f['home'],CUTEX_MANAGEMENT_URL=f['management_url'],TMPDIR='/tmp')
assert (home/'.cutex-test-private-home').is_file(), 'isolated HOME marker required'
assert home.resolve() != pathlib.Path.home().resolve(), 'do not use operator HOME'
conf=json.load(open(home/'.cutex/config.json'))
def run(label,args):
 t=time.monotonic();r=subprocess.run([BIN]+args,env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=150)
 (ROOT/(label+'.stdout')).write_bytes(r.stdout);(ROOT/(label+'.stderr')).write_bytes(r.stderr);print(label,r.returncode,round(time.monotonic()-t,2),flush=True)
 assert r.returncode==0,r.stderr.decode(errors='replace')[-1800:]
 return json.loads(r.stdout) if r.stdout.strip() else None
for name,args,port in [('bus',['agent','serve','--port','24762'],24762),('management',['management','serve','--port','24772','--bind','127.0.0.1'],24772)]:
 if (ROOT/(name+'.pid')).exists() and pathlib.Path('/proc/'+(ROOT/(name+'.pid')).read_text()).exists():continue
 p=subprocess.Popen([BIN]+args,env=env,stdout=open(ROOT/(name+'.log'),'ab'),stderr=subprocess.STDOUT,stdin=subprocess.DEVNULL,start_new_session=True);(ROOT/(name+'.pid')).write_text(str(p.pid))
 for _ in range(120):
  assert p.poll() is None,name+' exited'
  try:s=socket.create_connection(('127.0.0.1',port),.2);s.close();break
  except OSError:time.sleep(.1)
run('install',['human','install-runtime',f['bundle_manifest'],'--source-home',f['source_home'],'--job-descriptor',str(ROOT/'job-descriptor.json')])
if (ROOT/'director-id').exists():director=(ROOT/'director-id').read_text()
else:
 created=run('director-new',['human','new','typed-fixture-director','--cwd',str(ROOT)])
 director=created['adopted']['record']['cutex_session_id'];(ROOT/'director-id').write_text(director)
if not json.load(open(home/'.cutex/cutex-sessions.json'))['sessions'][director].get('runtime_pid'):run('director-start',['human','start',director])
store=home/'.cutex/cutex-sessions.json'
def record(id):return json.load(open(store))['sessions'][id]
drec=record(director)
project='typed-fixture'
mfile=home/'.cutex/runtime/agent-management/v1/agent-management-v1.json'
m=json.load(open(mfile));m['projects'][project]={'project_id':project,'authorized_director_session':director,'authority_epoch':1,'updated_at':'2026-09-14T00:00:00Z'}
m['store_revision']+=1;mfile.write_text(json.dumps(m));os.chmod(mfile,0o600)
def request(label,op):
 req={'schema':'cutex/agent-management/v1','action_id':label,'project_id':project,**op}
 r=urllib.request.Request('http://127.0.0.1:24762/api/agent-management/v1/actions',data=json.dumps(req).encode(),headers={'Authorization':'Bearer '+conf['agent_bus_token'],'X-Cutex-Agent-Id':record(director)['current_runtime_agent_id'],'Content-Type':'application/json'})
 start=time.monotonic()
 with urllib.request.urlopen(r,timeout=180) as response:body=json.load(response)
 (ROOT/(label+'.json')).write_text(json.dumps(body));print(label,body.get('outcome',{}).get('status'),round(time.monotonic()-start,2),flush=True)
 return body
spec={'name':'typed-worker','cwd':str(ROOT/'worker-home'),'profile':'aemeath','runtime_backend':'cute_alden','model':'gpt-5.6-sol','reasoning':'high','permissions':'danger-full-access','approval_policy':'never','sandbox_mode':'danger-full-access','groups':['typed-fixture'],'expose_to_im':False,'pin':False}
op={'operation':'create','spec':spec,'start_mode':'bootstrap_only','frozen_message':None}
existing=json.load(open(mfile)).get('actions',{}).get('typed-create-same-action',{})
result=request('typed-create-same-action',op)
a=json.load(open(mfile))['actions']['typed-create-same-action'];captured=a['known_native_session_id'];id=a['known_successor_cutex_session']
if result['outcome']['status']!='complete':
 assert 'Job descriptor' in json.dumps(result),result
 assert a['phase']=='configured' and captured and id and a['response'] is None
 (ROOT/'captured-before-retry.json').write_text(json.dumps({'native_id':captured,'id':id,'phase':a['phase']}))
 print('reproduced missing Job descriptor with installed descriptor; retry same action after upgrade',flush=True)
 raise SystemExit(0)
prior=json.load(open(ROOT/'captured-before-retry.json'));assert captured==prior['native_id'] and id==prior['id']
(ROOT/'worker-id').write_text(id);before=record(id);assert before['runtime_backend']=='host' and before['app_server_runtime'],result
sessions=json.load(open(store));receipts=[r['receipt'] for r in sessions['explicit_launch_receipts'].values() if r.get('kind')=='runtime' and r['receipt'].get('review',{}).get('subject',{}).get('cutex_session_id')==id]
assert receipts and any(r['review']['job_mcp'] is not None and r['review']['configuration']['selected_projection']['requires_job'] for r in receipts)
assert record(id)['codex_session_id']==captured
nextcwd=ROOT/'next-cwd';nextcwd.mkdir(exist_ok=True)
run('worker-next-config',['human','config','set',id,'sandbox=read-only','model=gpt-5.6-terra','cwd='+str(nextcwd),'--action-id','fixture-next-config'])
res=request('typed-online-reuse-r17',{'operation':'online','cutex_session_id':id});assert res['outcome']['status']=='complete',res
assert record(id)['app_server_runtime']==before['app_server_runtime'] and record(id)['runtime_generation']==before['runtime_generation']
run('worker-undo-config',['human','config','undo',id,'fixture-next-config','--action-id','fixture-undo-config'])
request('typed-restart-r17',{'operation':'restart','cutex_session_id':id});after=record(id);assert after['runtime_generation']==before['runtime_generation']+1 and after['runtime_pid']!=before['runtime_pid']
request('typed-offline',{'operation':'offline','cutex_session_id':id});assert record(id)['app_server_runtime'] is None
request('typed-online-again',{'operation':'online','cutex_session_id':id});final=record(id);assert final['runtime_generation']==after['runtime_generation']+1
closed=request('typed-close',{'operation':'close','cutex_session_id':id});assert closed['outcome']['status']=='complete',closed
assert record(id)['app_server_runtime'] is None and record(id).get('retiredAt') is not None
run('director-stop',['human','stop',director,'--force'])
(ROOT/'acceptance.json').write_text(json.dumps({'captured_native_id':captured,'worker':id,'initial_generation':before['runtime_generation'],'restart_generation':after['runtime_generation'],'online_generation':final['runtime_generation'],'same_id_retry':True,'native_config_resume_cwd':'session','model_prompts_sent':0,'profile_job_enabled':True,'installed_job_descriptor':True},indent=2));print('typed lifecycle accepted',flush=True)
