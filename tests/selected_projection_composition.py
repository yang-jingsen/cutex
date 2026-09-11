"""Model-free private final-byte projection composition. Run only in wrapper.

Uses existing native WebSocket framing, real root review/activation and managed
bootstrap, not raw state writes. No turn/start, tool submission or real auth.
"""
import ast, base64, fcntl, hashlib, http.client, json, os, queue, re, select
import signal, socket, struct, subprocess, sys, termios, threading, time
from pathlib import Path

assert os.environ.get('SELECTED_COMPOSITION_PRIVATE') == '1'
assert socket.if_nameindex() == [(1, 'lo')]
os.umask(0o077)
ROOT=Path('/p'); HOME=ROOT/'h'; CONF=HOME/'.cutex'; NATIVE=CONF/'codex-home'
for p in (HOME, CONF, NATIVE): p.mkdir(mode=0o700)
SOURCE=Path(__file__).resolve().parents[1]
BIN=Path(sys.argv[1]); CUTEX=BIN/'cutex'; MCP=BIN/'cutex-mcp'
NB=Path('/mnt/mambo/PersonaProjects/cutex-light-core-r1/artifacts/custom-status-r1/bundle')
SERVER=NB/'codex-app-server'
env={'PATH':'/usr/bin:/bin','HOME':str(HOME),'CODEX_HOME':str(NATIVE),'TMPDIR':str(ROOT), 'TERM':'xterm-256color','LANG':'C.UTF-8'}
BUS='private-selected-bus'; HUMAN='private-selected-root'
children=[]; logs=[]; owned=[]; facts=[]; peers=[]
def save(name, value): (ROOT/name).write_text(json.dumps(value,indent=2))
def sha(p): return hashlib.sha256(p.read_bytes()).hexdigest()
def ref(p): return {'path':str(p),'sha256':sha(p)}
def identity(pid):
    try: return (Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()[19],str(Path(f'/proc/{pid}/exe').readlink()))
    except FileNotFoundError: return None
def owner(args,name):
    log=open(ROOT/(name+'.log'),'wb'); logs.append(log)
    p=subprocess.Popen(list(map(str,args)),env=env,cwd=ROOT,stdout=log,stderr=log,start_new_session=True)
    children.append(p); return p
def stop(p):
    if p.poll() is None: p.terminate()
    try:p.wait(timeout=10)
    except subprocess.TimeoutExpired:p.kill();p.wait(timeout=10)
def wait_socket(path,child):
    end=time.monotonic()+30
    while not path.exists():
        assert child.poll() is None and time.monotonic()<end,'owned native socket not ready'
        threading.Event().wait(.03)
def listen(port,child):
    end=time.monotonic()+30
    while True:
        assert child.poll() is None and time.monotonic()<end,'owned service not ready'
        try:
            with socket.create_connection(('127.0.0.1',port),.1):return
        except OSError:threading.Event().wait(.03)
def api(port,path,body=None,token=HUMAN,headers=None):
    assert port in (24870,24871) and path.startswith('/') and not path.startswith('//')
    c=http.client.HTTPConnection('127.0.0.1',port,timeout=180)
    c.request('POST' if body is not None else 'GET',path,None if body is None else json.dumps(body),{'Authorization':'Bearer '+token,'Content-Type':'application/json',**(headers or {})})
    r=c.getresponse();data=r.read();status=r.status;c.close()
    return status,json.loads(data)
def action(body):
    code,value=api(24871,'/v2/agent-management/explicit-launch',body)
    assert code==200,(code,value);return value
def cli(*args):
    p=subprocess.run([str(CUTEX),*map(str,args)],env=env,cwd=ROOT,capture_output=True,timeout=60)
    assert p.returncode==0,p.stderr.decode();return p.stdout
def store():return json.loads((CONF/'cutex-sessions.json').read_text())
rpc_tree=ast.parse((SOURCE/'tests/stock_mcp_boundary.py').read_text())
rpc_class=next(n for n in rpc_tree.body if isinstance(n,ast.ClassDef) and n.name=='RPC')
exec(compile(ast.Module(body=[rpc_class],type_ignores=[]),'accepted-WebSocket-helper','exec'))
def rpc(path):
    r=RPC(path);peers.append(r)
    r.call('initialize',{'clientInfo':{'name':'selected-private','version':'1'},'capabilities':{'experimentalApi':True}});r.notify('initialized');return r
def auth(label):
    claims={'sub':'dummy-user','email':label+'@example.invalid','https://api.openai.com/auth':{'chatgpt_user_id':'user-'+label,'chatgpt_account_id':label,'chatgpt_plan_type':'plus'}}
    jwt='e30.'+base64.urlsafe_b64encode(json.dumps(claims).encode()).decode().rstrip('=')+'.dummy'
    return {'auth_mode':'chatgpt','OPENAI_API_KEY':None,'tokens':{'id_token':jwt,'access_token':'dummy-access','refresh_token':'dummy-refresh','account_id':label},'last_refresh':None}
IDS={'aemeath':'cd6a39eb-3997-45c6-9824-5113fe36a4b8','octobre':'b341adf8-7af9-432c-a12b-0e9a674458ed','GLM':'3f38c782-bac6-403d-b11d-c801326e0bb1'}
def profile(name):
    p=CONF/'profiles'/IDS[name];p.mkdir(parents=True,mode=0o700)
    (p/'auth.json').write_text(json.dumps(auth(name) if name!='GLM' else {'OPENAI_API_KEY':'dummy-glm-key'}))
    text="cutex_provider_mode='selected_profile_v2'\ncli_auth_credentials_store='file'\n"
    if name=='GLM':
        catalog=json.loads(subprocess.check_output(['/usr/bin/git','-C','/mnt/mambo/PersonaProjects/cutex-light-core-r1/source','show','8cde795620e8b2fa6ba3bfa1fd15a5732a1e12f6:codex-rs/models-manager/models.json'],env=env))
        sample=next(m for m in catalog['models'] if m['slug']=='gpt-5.6-sol')
        sample['slug']='glm-5.3';sample['display_name']='Private GLM catalog';sample['default_reasoning_level']='max'
        sample['supported_reasoning_levels']=[{'effort':'max','description':'Private catalog max'}]
        (p/'models.json').write_text(json.dumps({'models':[sample]}))
        text+="model='glm-5.3'\nmodel_reasoning_effort='max'\nmodel_provider='GLM'\nmodel_catalog_json="+json.dumps(str(p/'models.json'))+"\n[model_providers.GLM]\nname='GLM'\nbase_url='https://www.colabapi.com/v1'\nwire_api='responses'\nrequires_openai_auth=false\nenv_key='OPENAI_API_KEY'\n"
    else:text+="model='gpt-5.6-sol'\nmodel_reasoning_effort='max'\n"
    text+="[tui]\nstatus_line=['custom:bon-voyage','custom:profile','model-with-reasoning']\nstatus_line_use_colors=true\n"
    (p/'config.toml').write_text(text)
    (p/'custom-status-items.json').write_text(json.dumps({'items':[{'id':'custom:bon-voyage','title':'Bon voyage','source':{'kind':'static','value':'Bon voyage !'},'style':{'fg':'#F6A3C8','bold':True}},{'id':'custom:profile','title':'Profile','source':{'kind':'launch_profile'},'style':{'fg':'#FFFFFF','bold':True}}]}))
    return p
def launch(ident,action_id,restart=False):
    review=action({'operation':'review_runtime','cutex_session_id':ident,'restart':restart})
    receipt=action({'operation':'run','action_id':action_id,'review':review})
    save(action_id+'.json',receipt)
    assert receipt['stage']=='ready',receipt
    pid=receipt['binding']['pid'];owned.append((pid,identity(pid)))
    return receipt
def attach(ident,label):
    master,slave=os.openpty();fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',32,140,0,0))
    original=termios.tcgetattr(slave)
    child=subprocess.Popen([str(CUTEX),'session','stock-attach',ident],env=env,cwd=ROOT,stdin=slave,stdout=slave,stderr=slave,start_new_session=True,preexec_fn=lambda:fcntl.ioctl(0,termios.TIOCSCTTY,0))
    children.append(child);screen=b'';ready=False
    try:
        end=time.monotonic()+60
        while time.monotonic()<end:
            assert child.poll() is None,'CLI exited before status'
            if select.select([master],[],[],.1)[0]:
                part=os.read(master,65536);screen+=part
                if b'\x1b[6n' in part:os.write(master,b'\x1b[1;1R')
                if b'\x1b[c' in part:os.write(master,b'\x1b[?1;2c')
                plain=re.sub(rb'\x1b\[[0-9;?<>=]*[ -/]*[@-~]',b'',screen)
                if b'Bon voyage !' in plain and label.encode() in plain and b'38;2;246;163;200' in screen:ready=True;break
        assert ready,'reviewed label/pink not rendered'
    finally:
        if child.poll() is None:
            os.write(master,b'\x03\x03')
            try:child.wait(timeout=10)
            except subprocess.TimeoutExpired:stop(child)
        restored=termios.tcgetattr(slave)==original
        os.close(master);os.close(slave)
        (ROOT/('cli-'+label+'.ansi')).write_bytes(screen)
    assert child.returncode==0 and restored,'CLI did not exit normally/restore terminal'
    return {'label':label,'pink':True,'restored':True,'input_sent':False}

try:
    # Namespace has no external route; no live credentials or model endpoint.
    save('config-facts.json',{'model_turns':0,'network':'private loopback namespace','profile_ids':IDS})
    (CONF/'config.json').write_text(json.dumps({'agent_bus_enabled':True,'agent_bus_port':24870,'agent_bus_token':BUS,'management_api_token':HUMAN,'default_profile':'aemeath'}))
    for name in IDS:profile(name)
    (CONF/'accounts.json').write_text(json.dumps({'version':3,'accounts':[{'id':i,'name':n,'email':None,'plan_type':None,'last_used_at':None} for n,i in IDS.items()],'active_account_id':None}))
    for label in ('octobre','GLM'): (ROOT/label).mkdir(mode=0o700)
    (NATIVE/'config.toml').write_text('[projects."/p"]\ntrust_level="trusted"\n[projects."/p/octobre"]\ntrust_level="trusted"\n[projects."/p/GLM"]\ntrust_level="trusted"\n[analytics]\nenabled=false\n')
    bundle={'version':3,'upstream_commit':'3d2ee51ca2d5db578f328aa75e20aa22c0197c9a','native_patch_commit':'8cde795620e8b2fa6ba3bfa1fd15a5732a1e12f6','executable':ref(SERVER),'cli':ref(NB/'codex'),'code_mode_host':ref(NB/'codex-code-mode-host'),'schema':ref(NB/'codex_app_server_protocol.schemas.json'),'facade':ref(MCP),'shared_config':ref(NATIVE/'config.toml')}
    save('bundle.json',bundle)
    boot=owner([SERVER,'--auth-file',CONF/'profiles'/IDS['aemeath']/'auth.json','-c','cli_auth_credentials_store="file"','-c','default_permissions=":read-only"','--listen','unix:///p/b.sock'],'bootstrap')
    wait_socket(ROOT/'b.sock',boot);r=rpc(ROOT/'b.sock')
    thread=r.call('thread/start',{'cwd':'/p','sandbox':'read-only','approvalPolicy':'never','ephemeral':False,'historyMode':'paginated'})['thread']['id']
    # Accepted native persistent start ACK is the barrier, not this read.
    assert r.call('thread/read',{'threadId':thread,'includeTurns':True})['thread']['turns']==[]
    stop(boot)
    cli('session','adopt',thread,'--name','Private selected Director','--cwd','/p')
    ident=next(k for k,v in store()['sessions'].items() if v.get('codex_session_id')==thread)
    cli('session','defaults','set',ident,'--runtime-backend','host','--permission',':read-only','--sandbox','read-only','--approval-policy','never','--model','gpt-5.6-sol','--reasoning','max')
    bus=owner([CUTEX,'agent','serve','--port','24870'],'bus');listen(24870,bus)
    management=owner([CUTEX,'management','serve','--port','24871'],'management');listen(24871,management)
    contract={'version':2,'native_id':thread,'native_home':str(NATIVE),'bundle_manifest':'/p/bundle.json','bundle_sha256':sha(ROOT/'bundle.json')}
    review=action({'operation':'review','cutex_session_id':ident,'contract':contract})
    action({'operation':'activate','action_id':'selected-activate','review':review})
    current=launch(ident,'selected-launch')
    assert current['review']['configuration']['inherited']
    facts.append(attach(ident,'aemeath'))
    current=launch(ident,'selected-restart',True)
    assert current['expected_generation']==2
    project={'schema':'cutex/human-management-project-mutation/v1','action_id':'selected-project','project_id':'selected-private','expected_authority_epoch':0,'expected_project_revision':0,'operation':{'kind':'create','director_cutex_session_id':ident,'presentation':{'display_name':'Private selected','badge_label':'SP','color':'cyan'}}}
    code,candidates=api(24871,'/v2/agent-management/durable-candidates');assert code==200
    candidate=next(c for c in candidates if c['cutex_session_id']==ident)
    code,result=api(24871,'/v2/agent-management/durable-import',{'action_id':'selected-import','candidate':candidate,'confirmed_formal_name':'Private selected Director','assignment':project,'detach':None});assert code==200 and result['complete'],result
    for label in ('octobre','GLM'):
        model='glm-5.3' if label=='GLM' else 'gpt-5.6-sol'
        spec={'name':'Private '+label,'cwd':str(ROOT/label),'profile':label,'runtime_backend':'host','model':model,'reasoning':'max','permissions':':danger-full-access','approval_policy':'never','sandbox_mode':'danger-full-access','groups':['selected-private'],'expose_to_im':False,'pin':False}
        assert spec['cwd'] != '/p' and spec['cwd'] != str(ROOT/('GLM' if label=='octobre' else 'octobre'))
        request={'schema':'cutex/agent-management/v1','action_id':'selected-create-'+label,'bootstrap_intent':'selected-create-'+label,'project_id':'selected-private','operation':'create','spec':spec,'start_mode':'bootstrap_only','frozen_message':None}
        review=action({'operation':'review_bootstrap','request':request,'native_home':str(NATIVE),'bundle_manifest':'/p/bundle.json','bundle_sha256':sha(ROOT/'bundle.json'),'expires_at_unix':int(time.time())+600})
        action({'operation':'authorize_bootstrap','review':review})
        headers={'X-Cutex-Agent-Id':current['runtime_agent_id'],'X-Cutex-Mcp-Thread-Id':thread,'X-Cutex-Mcp-Generation':str(current['expected_generation'])}
        code,result=api(24870,'/api/agent-management/v1/actions',request,BUS,headers);save(label+'-create.json',result)
        assert code==200 and result['outcome']['status']=='complete',result
        record=next(v for v in store()['sessions'].values() if v['formal_agent_name']==spec['name'])
        receipt=next(v['receipt'] for v in store()['explicit_launch_receipts'].values() if v['kind']=='runtime' and v['receipt']['review']['subject']['cutex_session_id']==record['cutex_session_id'])
        assert receipt['stage']=='ready';pid=receipt['binding']['pid'];owned.append((pid,identity(pid)))
        peer=rpc(Path(receipt['binding']['endpoint'].removeprefix('unix://')))
        assert peer.call('thread/read',{'threadId':record['codex_session_id'],'includeTurns':True})['thread']['turns']==[]
        facts.append(attach(record['cutex_session_id'],label))
    save('PASS.json',{'facts':facts,'model_turns':0,'registered_restart_generation':2,'cutex':sha(CUTEX),'facade':sha(MCP),'native_cli':sha(NB/'codex'),'native_server':sha(SERVER),'boundary':'actual root review/bootstrap/registration/TUI, synthetic auth, no provider request'})
finally:
    for pid,birth in reversed(owned):
        if birth is not None and identity(pid)==birth:
            try:os.kill(pid,signal.SIGTERM)
            except ProcessLookupError:pass
    for p in reversed(children):stop(p)
    for log in logs:log.close()
