"""One private run: all34 API import, separate representative history probes.

Never start the all34 migration-plan records. Probe stores are independent copies
with the same IDs and synthetic accounts. No turns, jobs, auth copies, or bodies
in retained diagnostics. Must be invoked by the namespace wrapper.
"""
import shutil, sqlite3, traceback
from pathlib import Path

# Reuse the accepted bounded transport/CLI/process helpers, not a new protocol.
helper=Path(__file__).with_name('selected_projection_composition.py').read_text().split('\ntry:\n')[0]
exec(compile(helper,'accepted-r3-helpers','exec'))
PLAN=json.loads(Path('/p/input/plan.json').read_text())
JOB=Path('/mnt/mambo/PersonaProjects/cutex-job-frozen-completion-facts-v2-r1/artifacts/linux/cutex-job-service')
phase='setup'
sys.path.insert(0,str(SOURCE/'scripts'))
from human_error_text import redact

def json_terminal(value):
    if isinstance(value,bytes):return list(value)
    if isinstance(value,(list,tuple)):return [json_terminal(v) for v in value]
    return value

def drain_exit(child,master,consume,timeout=20):
    """Keep draining the terminal during shutdown; wait() alone can deadlock."""
    end=time.monotonic()+timeout
    while child.poll() is None and time.monotonic()<end:
        if select.select([master],[],[],.1)[0]:
            try:data=os.read(master,65536)
            except OSError:break
            if data:consume(data)
    return child.poll() is not None

def error_evidence():
    out=[]
    for path in sorted((CONF/'runtime/app-server').glob('*/stock.stderr.log')):
        for line in path.read_text(errors='replace').splitlines():
            plain=re.sub(r'\x1b\[[0-9;?<>=]*[ -/]*[@-~]','',line)
            if re.search(r'\b(?:ERROR|WARN)\b',plain):
                out.append(redact(plain))
    # Owner-only diagnostics; never include the rendered transcript or auth.
    return out[-32:]
def prepare(label):
    global HOME,CONF,NATIVE,env
    HOME=ROOT/label;CONF=HOME/'.cutex';NATIVE=CONF/'codex-home'
    for p in (HOME,CONF,NATIVE):p.mkdir(mode=0o700,exist_ok=True)
    env={'PATH':'/usr/bin:/bin','HOME':str(HOME),'CODEX_HOME':str(NATIVE),'TMPDIR':str(ROOT),'TERM':'xterm-256color','LANG':'C.UTF-8'}
    (CONF/'config.json').write_text(json.dumps({'agent_bus_enabled':True,'agent_bus_port':24870,'agent_bus_token':BUS,'management_api_token':HUMAN,'default_profile':PLAN['default_profile']}))
    (CONF/'accounts.json').write_text(json.dumps({'version':3,'accounts':[{'id':i,'name':n,'email':None,'plan_type':None,'last_used_at':None} for n,i in IDS.items()],'active_account_id':None}))
    for name,profile in PLAN['profiles'].items():
        p=CONF/'profiles'/profile['id'];p.mkdir(parents=True,mode=0o700)
        (p/'auth.json').write_text(json.dumps(auth('rehearsal-'+name) if name!='GLM' else {'OPENAI_API_KEY':'private-synthetic-glm'}))
        raw=profile['toml']
        for filename,asset in profile['assets'].items():
            data=base64.b64decode(asset['bytes_b64']);assert hashlib.sha256(data).hexdigest()==asset['sha256']
            (p/filename).write_bytes(data)
            raw=raw.replace(asset['source'],str(p/filename))
        (p/'config.toml').write_text("cutex_provider_mode='selected_profile_v2'\n"+raw)
        (p/'custom-status-items.json').write_text(json.dumps(profile['status']))
    (NATIVE/'config.toml').write_text('cutex_projection_version=2\n'+PLAN['shared']['toml'])
    # Exact nonsecret shared skill assets, read-only input. No plugin installation.
    shutil.copytree('/home/senxiu/.cutex/codex-home/skills',NATIVE/'skills')
    (NATIVE/'sessions').mkdir(mode=0o700)

def adopt(row):
    group_args=[v for group in row['groups'] for v in ('--group',group)]
    cli('session','adopt',row['native_id'],'--name',row['formal_name'],'--cwd',row['cwd'],*group_args)
    ident=row['durable_id'];assert ident in store()['sessions']
    # Adopt adds its normal cwd-derived convenience group. The supported Set
    # operation restores the exact frozen groups before authoritative import.
    # Added after the second run; requires separately authorized runtime proof.
    cli('session','groups','set',ident,*group_args)
    cli('session','defaults','set',ident,'--runtime-backend','host','--permission',row['permission_alias'],
        '--sandbox',row['sandbox'],'--approval-policy',row['approval'],'--model',row['model'],'--reasoning',row['effort'])
    if row['configured_profile'] is not None:cli('session','profile','set',ident,row['configured_profile'])

def services(label):
    b=owner([CUTEX,'agent','serve','--port','24870'],label+'-bus');listen(24870,b)
    m=owner([CUTEX,'management','serve','--port','24871'],label+'-management');listen(24871,m)
    return b,m

def import_one(row,assignment=None):
    code,cs=api(24871,'/v2/agent-management/durable-candidates');assert code==200
    candidate=next(c for c in cs if c['cutex_session_id']==row['durable_id'])
    request={'action_id':'import-'+row['native_id'],'candidate':candidate,'confirmed_formal_name':row['formal_name'],'assignment':assignment,'detach':None}
    code,result=api(24871,'/v2/agent-management/durable-import',request)
    assert code==200 and result['complete'],(code,result)
    again,replay=api(24871,'/v2/agent-management/durable-import',request)
    assert again==200 and replay==result,'import replay differs'

def project_request(project,operation,revision=0,epoch=0,suffix='create'):
    return {'schema':'cutex/human-management-project-mutation/v1','action_id':project+'-'+suffix,
            'project_id':project,'expected_authority_epoch':epoch,'expected_project_revision':revision,'operation':operation}

def project_summary(project):
    code,value=api(24871,'/v2/agent-management/projects');assert code==200
    return next(p for p in value['projects'] if p['project_id']==project)

def pty_probe(ident,label):
    master,slave=os.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',38,160,0,0))
    original=termios.tcgetattr(slave)
    child=subprocess.Popen([str(CUTEX),'session','stock-attach',ident],env=env,cwd=ROOT,stdin=slave,stdout=slave,stderr=slave,start_new_session=True,preexec_fn=lambda:fcntl.ioctl(0,termios.TIOCSCTTY,0))
    children.append(child);screen=b'';ready=False
    forced_exit=False
    after=None
    def consume(part):
        nonlocal screen
        screen+=part
        if b'\x1b[6n' in part:os.write(master,b'\x1b[1;1R')
        if b'\x1b[c' in part:os.write(master,b'\x1b[?1;2c')
    try:
        end=time.monotonic()+90
        while time.monotonic()<end:
            assert child.poll() is None,'CLI exited before reviewed status'
            if select.select([master],[],[],.1)[0]:
                part=os.read(master,65536);consume(part)
                plain=re.sub(rb'\x1b\[[0-9;?<>=]*[ -/]*[@-~]',b'',screen)
                if b'Bon voyage' in plain and label.encode() in plain and b'38;2;246;163;200' in screen:ready=True;break
        assert ready,'reviewed historical CLI status not rendered'
    finally:
        if child.poll() is None:
            os.write(master,b'\x03\x03')
            if not drain_exit(child,master,consume):
                forced_exit=True;stop(child)
        after=termios.tcgetattr(slave);restored=after==original
        observation={'ready':ready,'exit_code':child.returncode,'terminal_restored':restored,
                     'exit_signal':-child.returncode if child.returncode is not None and child.returncode<0 else None,
                     'forced_exit':forced_exit,'screen_bytes':len(screen),
                     'terminal_before':json_terminal(original),'terminal_after':json_terminal(after),
                     'screen_sha256':hashlib.sha256(screen).hexdigest(),'model_input_sent':False,
                     'exit_keys':'Ctrl-C twice','sanitized_owner_errors':error_evidence()}
        save('pty-'+ident+'.json',observation)
        os.close(master);os.close(slave)
    assert child.returncode==0 and restored,observation
    # Deliberately do not retain screen/history bodies.
    return {'profile':label,'pink':True,'terminal_restored':True,'screen_bytes':len(screen),'screen_sha256':hashlib.sha256(screen).hexdigest(),'input_sent':False}

def run_probe(row,index):
    global phase
    phase='probe-'+str(index)+'-prepare';prepare('p'+str(index))
    history=row['history'];path=NATIVE/'sessions'/Path(history['source_path']).name
    shutil.copyfile(Path('/p/input/prefixes')/(row['native_id']+'.jsonl'),path)
    assert sha(path)==history['sha256']
    adopt(row)
    b,m=services('p'+str(index))
    try:
        phase='probe-'+str(index)+'-review'
        bundle={'version':3,'upstream_commit':'3d2ee51ca2d5db578f328aa75e20aa22c0197c9a','native_patch_commit':'8cde795620e8b2fa6ba3bfa1fd15a5732a1e12f6','executable':ref(SERVER),'cli':ref(NB/'codex'),'code_mode_host':ref(NB/'codex-code-mode-host'),'schema':ref(NB/'codex_app_server_protocol.schemas.json'),'facade':ref(MCP),'shared_config':ref(NATIVE/'config.toml')}
        manifest=ROOT/('bundle-'+str(index)+'.json');manifest.write_text(json.dumps(bundle))
        contract={'version':2,'native_id':row['native_id'],'native_home':str(NATIVE),'bundle_manifest':str(manifest),'bundle_sha256':sha(manifest)}
        review=action({'operation':'review','cutex_session_id':row['durable_id'],'contract':contract})
        action({'operation':'activate','action_id':'probe-activate-'+str(index),'review':review})
        descriptor=None;daemon=None
        if PLAN['profiles'][row['effective_profile']]['toml'].find('[mcp_servers.cutex_job]')>=0:
            for filename in ('job-api','job-grant'):(HOME/filename).write_bytes(os.urandom(32))
            sock=HOME/'job.sock'
            daemon=owner([JOB,'serve',HOME/'job-state',sock,HOME/'job-api',HOME/'job-grant',NB/'codex'],'p'+str(index)+'-job')
            wait_socket(sock,daemon)
            descriptor={'version':1,'adapter':ref(JOB),'launcher':ref(NB/'codex'),'endpoint':str(sock),'api_token_file':str(HOME/'job-api'),'grant_key_file':str(HOME/'job-grant'),'daemon_pid':daemon.pid,'daemon_start_ticks':int(identity(daemon.pid)[0])}
        review=action({'operation':'review_runtime','cutex_session_id':row['durable_id'],'restart':False,'job_mcp':descriptor})
        phase='probe-'+str(index)+'-run'
        result=action({'operation':'run','action_id':'probe-run-'+str(index),'review':review})
        assert result['stage']=='ready',(result['stage'],result.get('error'))
        pid=result['binding']['pid'];birth=identity(pid);owned.append((pid,birth))
        peer=rpc(Path(result['binding']['endpoint'].removeprefix('unix://')))
        phase='probe-'+str(index)+'-read'
        response=peer.call('thread/read',{'threadId':row['native_id'],'includeTurns':True})['thread']
        assert response['id']==row['native_id'] and response['turns']
        types={}
        for turn in response['turns']:
            for item in turn['items']:types[item['type']]=types.get(item['type'],0)+1
        save('read-'+row['native_id']+'.json',{'native_id':response['id'],'turns_read':len(response['turns']),
              'item_types':types,'ready_generation':result['expected_generation'],
              'configuration':{k:review['configuration'][k] for k in ('profile_name','inherited','model','reasoning','sandbox','approval')}})
        phase='probe-'+str(index)+'-attach'
        visual=pty_probe(row['durable_id'],row['effective_profile'])
        configuration=review['configuration']
        assert configuration['inherited']==(row['configured_profile'] is None)
        assert configuration['model']==row['model'] and configuration['reasoning']==row['effort']
        assert not any(e.get('method')=='turn/started' for e in peer.events)
        phase='probe-'+str(index)+'-close'
        # Owned private occurrence only. Never touch the all34 registry store.
        if identity(pid)==birth:os.kill(pid,signal.SIGTERM)
        deadline=time.monotonic()+15
        while identity(pid)==birth and time.monotonic()<deadline:threading.Event().wait(.05)
        assert identity(pid)!=birth,'owned runtime did not exit'
        assert sha(path)==history['sha256'],'history prefix/file changed without a turn'
        db=sqlite3.connect('file:'+str(NATIVE/'state_5.sqlite')+'?mode=ro',uri=True)
        catalog=db.execute('select id,memory_mode,history_mode from threads where id=?',(row['native_id'],)).fetchone();db.close()
        assert catalog==(row['native_id'],'enabled',history['mode']),catalog
        if daemon:stop(daemon)
        return {'native_id':row['native_id'],'formal_name':row['formal_name'],'source_status':row['source_status'],
                'probe_only':True,'profile':row['effective_profile'],'inherited':configuration['inherited'],
                'model':row['model'],'effort':row['effort'],'sandbox':row['sandbox'],'approval':row['approval'],
                'history_mode':history['mode'],'memory_mode':'enabled','history_sha256':history['sha256'],
                'turns_read':len(response['turns']),'item_types':types,'visual':visual,'generation':result['expected_generation']}
    finally:
        save('errors-'+row['native_id']+'.json',{'phase':phase,'messages':error_evidence()})
        stop(m);stop(b)

def registry():
    global phase
    prepare('h')
    rows=PLAN['rows'];assert len(rows)==34
    phase='all34-adopt'
    for row in rows:adopt(row)
    extra=PLAN['prerequisite']
    cli('session','adopt',extra['native_id'],'--name',extra['formal_name'],'--cwd','/p/prerequisite')
    b,m=services('all34')
    phase='all34-import'
    for row in rows+[extra]:import_one(row)
    for project,p in PLAN['projects'].items():
        request=project_request(project,{'kind':'create','director_cutex_session_id':p['director_id'],
                                 'presentation':{'display_name':project,'badge_label':'P','color':'cyan'}})
        code,result=api(24871,'/v2/agent-management/project-mutations',request);assert code==200,(code,result)
        for row in rows:
            if row['project_id']!=project or row['durable_id']==p['director_id']:continue
            current=project_summary(project)
            request=project_request(project,{'kind':'add_member','cutex_session_id':row['durable_id']},current['project_revision'],current['authority_epoch'],row['native_id'])
            code,result=api(24871,'/v2/agent-management/project-mutations',request);assert code==200,(code,result)
    phase='all34-verify'
    records=store()['sessions'];assert len(records)==35
    managed=json.loads((CONF/'runtime/agent-management/v1/agent-management-v1.json').read_text())
    for row in rows:
        r=records[row['durable_id']]
        assert r['codex_session_id']==row['native_id'] and r['formal_agent_name']==row['formal_name']
        for key,value in [('profile',row['configured_profile']),('model_defaults',row['model']),('reasoning_defaults',row['effort']),('sandbox_mode',row['sandbox']),('approval_policy',row['approval'])]:assert r.get(key)==value,(key,row['durable_id'])
        assert not r.get('app_server_runtime') and not r.get('current_runtime_agent_id') and r.get('runtime_generation',0)==0
        assert managed['current_project_memberships'][row['durable_id']]['project_id']==row['project_id']
        assert r['agent_groups']==row['groups'],'group projection drift'
    save('all34-PASS.json',{'subjects':34,'outside_cohort_nonlaunching_prerequisites':1,'offline_registry':35,'api_import_replay_equal':True,'project_membership_equal':True,'private_authority_epochs':'fresh; not migrated originals'})
    stop(m);stop(b)

try:
    mode=sys.argv[2] if len(sys.argv)>2 else 'all'
    if mode in ('all','registry'):registry()
    representatives=['cesc-tutor-r1','cute-codex-log-wal-fix-r2','tethys-director-r2','ifm-ema-figures'] if mode=='all' else ([mode.removeprefix('probe:')] if mode.startswith('probe:') else [])
    rows=PLAN['rows']
    proof=[]
    for index,name in enumerate(representatives):
        proof.append(run_probe(next(r for r in rows if r['formal_name']==name),index))
        save('probe-progress.json',proof)
    save('PASS.json',{'mode':mode,'all34_api_import':mode in ('all','registry'),'representatives':proof,'automatic_model_turns':0,'job_executions':0,'auth':'synthetic only','offline_originals_and_all34_registry_unchanged':True})
except BaseException as error:
    save('FAILURE.json',{'phase':phase,'type':type(error).__name__,'error':str(error),'traceback':traceback.format_exc()})
    raise
finally:
    for pid,birth in reversed(owned):
        if birth is not None and identity(pid)==birth:
            try:os.kill(pid,signal.SIGTERM)
            except ProcessLookupError:pass
    for p in reversed(children):stop(p)
    for log in logs:log.close()
