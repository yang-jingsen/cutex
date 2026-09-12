"""One owned, model-free same-ID maintenance oracle; namespace wrapper only.

Reuses accepted transports and private project setup. Never writes authority
stores, dispatches a Task, submits a Job, or supplies a model turn.
"""
import copy, shutil, sqlite3, traceback
from pathlib import Path

helper=Path(__file__).with_name('selected_projection_composition.py').read_text().split('\ntry:\n')[0]
exec(compile(helper,'accepted-projection-helpers','exec'))
PLAN=json.loads(Path('/p/input/plan.json').read_text())
tree=ast.parse((SOURCE/'tests/selected_history_composition.py').read_text())
names={'prepare','adopt','services','import_one','project_request','project_summary','error_evidence'}
exec(compile(ast.Module(body=[n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name in names],type_ignores=[]),'accepted-history-setup','exec'))
sys.path.insert(0,str(SOURCE/'scripts'))
from human_error_text import redact
JOB=Path('/mnt/mambo/PersonaProjects/cutex-job-frozen-completion-facts-v2-r1/artifacts/linux/cutex-job-service')
phase='prepare'; result={}; code=1
try:
    row=next(r for r in PLAN['rows'] if r['formal_name']=='cutex-director-r13')
    assert row['source_status']=='online-observed'
    prepare('h')
    # Original legacy fields, privately substituted asset/auth locations only.
    for profile in PLAN['profiles'].values():
        path=CONF/'profiles'/profile['id']/'config.toml'
        path.write_text(path.read_text().removeprefix("cutex_provider_mode='selected_profile_v2'\n"))
    raw=(NATIVE/'config.toml').read_text().removeprefix('cutex_projection_version=2\n')
    (NATIVE/'config.toml').write_text(raw)
    (NATIVE/'memories').mkdir(mode=0o700,exist_ok=True)
    (NATIVE/'memories'/'maintenance-fixture.txt').write_text('Private inert memory preservation oracle.\n')
    path=NATIVE/'sessions'/Path(row['history']['source_path']).name
    shutil.copyfile(Path('/p/input/prefixes')/(row['native_id']+'.jsonl'),path)
    assert sha(path)==row['history']['sha256']
    # Native itself reconstructs its derived catalog from accepted history.
    # No SQL writes or copied authority DB. Close the only owner normally.
    phase='source-catalog'
    source_owner=owner([SERVER,'--auth-file',CONF/'profiles'/IDS['aemeath']/'auth.json',
        '-c','cli_auth_credentials_store="file"','-c','default_permissions='+json.dumps(':'+row['sandbox']),
        '--listen','unix:///p/source.sock'],'source-native')
    wait_socket(Path('/p/source.sock'),source_owner)
    source_peer=rpc(Path('/p/source.sock'))
    response=source_peer.call('thread/resume',{'threadId':row['native_id'],'path':str(path),
        'model':row['model'],'cwd':row['cwd'],'approvalPolicy':'never',
        'permissions':':'+row['sandbox'],'excludeTurns':True})
    assert response['thread']['id']==row['native_id']
    source_peer.sock.close();peers.remove(source_peer)
    source_owner.send_signal(signal.SIGTERM);source_owner.wait(timeout=30)
    assert source_owner.returncode==0,'source owner did not exit normally'
    assert sha(path)==row['history']['sha256'],'source prefix changed without model turn'
    wal=NATIVE/'state_5.sqlite-wal'
    assert not wal.exists() or wal.stat().st_size==0,'source WAL pending; no migration checkpoint workaround'
    phase='existing-authority'
    adopt(row)
    cli('session','defaults','set',row['durable_id'],'--runtime-backend','cute-alden')
    bus,management=services('maintenance')
    import_one(row)
    request=project_request(row['project_id'],{'kind':'create','director_cutex_session_id':row['durable_id'],
        'presentation':{'display_name':'Private original authority','badge_label':'P','color':'cyan'}})
    status,value=api(24871,'/v2/agent-management/project-mutations',request)
    assert status==200,(status,value)
    (ROOT/'.migration-private-fixture').touch()
    (HOME/'.cutex-test-private-home').touch()
    save('task-seed.json',{'id':row['durable_id'],'project':row['project_id']})
    seed_env={**env,'CUTEX_TEST_PRIVATE_HOME':str(HOME)}
    seed_args=[sys.argv[2],'agent_management::migration::tests::maintenance_seed_existing_task','--ignored','--exact','--nocapture']
    seed=subprocess.run(seed_args,
        env=seed_env,cwd=ROOT,capture_output=True,timeout=30)
    assert seed.returncode==0 and (ROOT/'task-seed-result.json').exists(),redact(seed.stderr.decode())
    before=copy.deepcopy(store()['sessions'][row['durable_id']])
    before_project=project_summary(row['project_id'])
    for filename in ('job-api','job-grant'):(HOME/filename).write_bytes(os.urandom(32))
    sock=HOME/'job.sock'
    daemon=owner([JOB,'serve',HOME/'job-state',sock,HOME/'job-api',HOME/'job-grant',NB/'codex'],'job')
    wait_socket(sock,daemon)
    descriptor={'version':1,'adapter':ref(JOB),'launcher':ref(NB/'codex'),'endpoint':str(sock),
        'api_token_file':str(HOME/'job-api'),'grant_key_file':str(HOME/'job-grant'),
        'daemon_pid':daemon.pid,'daemon_start_ticks':int(identity(daemon.pid)[0])}
    bundle={'version':3,'upstream_commit':'3d2ee51ca2d5db578f328aa75e20aa22c0197c9a',
        'native_patch_commit':'8cde795620e8b2fa6ba3bfa1fd15a5732a1e12f6','executable':ref(SERVER),
        'cli':ref(NB/'codex'),'code_mode_host':ref(NB/'codex-code-mode-host'),
        'schema':ref(NB/'codex_app_server_protocol.schemas.json'),'facade':ref(MCP),
        'shared_config':ref(NATIVE/'config.toml')}
    phase='review'
    action_id='private-same-id-migration-1'
    request={'operation':'maintenance_review','request':{'action_id':action_id,'cutex_session_id':row['durable_id'],
        'destination':'/p/new-home','bundle':bundle,'expires_at_unix':int(time.time())+3600,'job_mcp':descriptor}}
    status,_=api(24871,'/v2/agent-management/explicit-launch',request,token=BUS)
    assert status in (401,403),'Agent credential obtained Human maintenance authority'
    save('review-request.json',request)
    review=json.loads(cli('session','stock','--request',ROOT/'review-request.json',
        '--management-url','http://127.0.0.1:24871/'))
    save('review.json',review)
    bad=copy.deepcopy(review);bad['subject']['revision']+=1
    status,_=api(24871,'/v2/agent-management/explicit-launch',{'operation':'maintenance_apply','review':bad})
    assert status!=200 and not Path('/p/new-home').exists()
    phase='apply'
    applied=action({'operation':'maintenance_apply','review':review});save('applied.json',applied)
    assert applied['phase']=='applied'
    assert action({'operation':'maintenance_apply','review':review})==applied
    after=store()['sessions'][row['durable_id']]
    allowed={'runtime_backend','explicit_launch','revision','updated_at'}
    assert {k:v for k,v in before.items() if k not in allowed}=={k:v for k,v in after.items() if k not in allowed}
    assert project_summary(row['project_id'])==before_project
    copied=Path('/p/new-home/sessions')/path.name
    assert sha(copied)==sha(path) and copied.stat().st_ino!=path.stat().st_ino
    assert (Path('/p/new-home/memories/maintenance-fixture.txt').read_bytes()==(NATIVE/'memories/maintenance-fixture.txt').read_bytes())
    status,_=api(24871,'/v2/agent-management/explicit-launch',{'operation':'review_runtime','cutex_session_id':row['durable_id'],'restart':False,'job_mcp':descriptor})
    assert status!=200,'ordinary protected runtime guard bypassed'
    phase='start'
    started=action({'operation':'maintenance_start','action_id':action_id});save('started.json',started)
    status_receipt=action({'operation':'maintenance_status','action_id':action_id});save('status.json',status_receipt)
    assert started['phase']=='activated' and status_receipt['runtime']['stage']=='ready',started.get('error')
    runtime=status_receipt['runtime'];pid=runtime['binding']['pid'];birth=identity(pid);owned.append((pid,birth))
    assert action({'operation':'maintenance_start','action_id':action_id})==started
    assert identity(pid)==birth
    peer=rpc(Path(runtime['binding']['endpoint'].removeprefix('unix://')))
    read=peer.call('thread/read',{'threadId':row['native_id'],'includeTurns':False})['thread']
    assert read['id']==row['native_id']
    assert project_summary(row['project_id'])==before_project
    assert sha(path)==row['history']['sha256'] and sha(copied)==sha(path)
    save('task-seed.json',{'id':row['durable_id'],'project':row['project_id'],'verify':True})
    verified=subprocess.run(seed_args,env=seed_env,cwd=ROOT,capture_output=True,timeout=30)
    assert verified.returncode==0 and (ROOT/'task-verified.json').exists(),redact(verified.stderr.decode())
    assert not any(e.get('method')=='turn/started' for p in peers for e in p.events)
    result={'same_id':row['durable_id'],'native_id':read['id'],'ready_generation':runtime['expected_generation'],
        'authority_unchanged':True,'source_prefix_unchanged':True,'independent_copy':True,
        'apply_replay':True,'start_replay':True,'agent_denied':True,'stale_review_denied':True,
        'ordinary_protected_guard_denied':True,'model_turns':0,'jobs':0,
        'task_setup':'real provider, private seated principal, no dispatch/ACK',
        'configuration':{k:review['configuration'][k] for k in ('profile_name','model','reasoning','sandbox','approval')}}
    code=0
except Exception as error:
    result={'phase':phase,'error':redact(str(error)),'trace':redact(traceback.format_exc()),'owner_errors':error_evidence()}
finally:
    for peer in peers:
        try:peer.sock.close()
        except Exception:pass
    for pid,birth in owned:
        if identity(pid)==birth:os.kill(pid,signal.SIGTERM)
    for child in reversed(children):stop(child)
    result['owned_child_exit_codes']=[p.returncode for p in children]
    source_log=ROOT/'source-native.log'
    if code and source_log.exists():
        result['source_native_errors']=[redact(line) for line in source_log.read_text(errors='replace').splitlines()
            if line.startswith('Error:') or ' ERROR ' in line][-8:]
    save('PASS.json' if code==0 else 'FAIL.json',result)
sys.exit(code)
