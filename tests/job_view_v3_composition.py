"""Actual default Cutex/Core/CodeMode/Job composition, loopback fake Responses.

Reuse only the proven presentation setup, transport and terminal helpers. No
manual grants, completion injection, canonical-store edits or fake receipts.
Run in the existing private bwrap network/PID namespace with a NEW short run.
"""
from pathlib import Path
import json
from job_view_v4_protocol import response_events, fresh_prompt
from job_view_v4_driver import Driver
import time

base_path = Path(__file__).with_name('presentation_boundary.py')
base = base_path.read_text()
prefix, old_body = base.split("    control=Control(1);", 1)
cleanup = "finally:\n" + old_body.rsplit("finally:\n", 1)[1]
changes = {
    'model="unknown-private-model"\\n': 'model="gpt-5.6-terra"\\nmodel_reasoning_effort="low"\\n',
    'unknown-private-model': 'gpt-5.6-terra',
    "artifacts/visible-only-tui-r1/final-bin": "artifacts/job-view-output-reference-bound-r1/bundle",
    "ROOT/'artifacts/presentation-job-r1/schema.json'": "Path('/mnt/mambo/PersonaProjects/cutex-light-core-r1/artifacts/structured-view-v3-r1/schema-json-stable/codex_app_server_protocol.schemas.json')",
    "CONTROLLER=Path(sys.argv[2]).resolve()\nassert CONTROLLER.is_relative_to(ROOT/'target/debug/deps')": "CONTROLLER=None",
    "artifacts/presentation-job-r1/default-bin": "artifacts/job-view-v3-r2",
    "'3d8a73a747cf5b957a7ca0491c28d1517f6d7722'": "'f8c33add01bf9ef8cea04f531fa1319090751cb2'",
    "config['private_job_presentation']={'version':1,'recipients':[durable]}": "config['private_job_presentation']={'version':2,'recipients':[durable]}",
    "current=launch('presentation-launch')": "current=launch_job(globals())",
    "model=http.server.ThreadingHTTPServer": "Model.do_POST=safe_model_response\nmodel=http.server.ThreadingHTTPServer",
    "default_permissions=\":read-only\"": "default_permissions=\":danger-full-access\"",
    "'sandbox':'read-only'": "'sandbox':'danger-full-access'",
    "'--permission','read-only','--sandbox','read-only'": "'--permission','full-access','--sandbox','danger-full-access'",
}
for old, new in changes.items():
    assert old in prefix, old
    prefix = prefix.replace(old, new)

context = {}
driver = None
interaction_started = None
approvals = []
stock_birth = {}
fixture_error = {}
prefix = prefix.replace('        value=f()', '        assert not fixture_error, str(fixture_error)\n        value=f()')
cleanup = cleanup.replace("if os.getpgid(pid)==pid and", "if process_identity(pid) and process_identity(pid)[0]==stock_birth.get(pid) and os.getpgid(pid)==pid and")

def safe_model_response(self):
    try:
        model_response(self)
    except Exception as error:
        fixture_error.update(kind=type(error).__name__, message=str(error)[:1000])
        (context['RUN']/'fixture-error.json').write_text(json.dumps(fixture_error))
        self.send_error(400, 'private fixture assertion failed; no fallback')

def model_response(self):
    global driver
    g = context
    request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
    assert self.path == '/v1/responses'
    g['Model'].requests.append(request)
    g['Model'].calls += 1
    assert g['Model'].calls <= 16 and time.monotonic()-interaction_started < 180, 'interaction budget'
    if driver is None: driver=Driver(g)
    item=driver.next(request)
    events=response_events(item,request,f'jv4-response-{g["Model"].calls}')
    body = ''.join('event: '+e['type']+'\ndata: '+json.dumps(e)+'\n\n' for e in events).encode()
    self.send_response(200)
    self.send_header('Content-Type','text/event-stream')
    self.send_header('Content-Length',str(len(body)))
    self.end_headers()
    self.wfile.write(body)

def launch_job(g):
    import os, socket, struct
    context.update(g)
    g['HOME'].chmod(0o700)
    binary = Path('/mnt/mambo/PersonaProjects/cutex-job-frozen-completion-facts-v2-r1/artifacts/linux/cutex-job-service')
    assert g['sha'](binary) == 'ba1a8d4f3e0b5f739e666e3f515d40b0c543e9e75953759ff181f1e71b29c521'
    for name in ('job-api','job-grant'):
        p=g['HOME']/name; p.write_bytes(os.urandom(32)); p.chmod(0o600)
    sock=g['HOME']/'job.sock'
    assert len(os.fsencode(sock)) <= 100
    daemon=g['owner']([binary,'serve',g['RUN']/'job-state',sock,g['HOME']/'job-api',g['HOME']/'job-grant',g['STOCK'],
        '--completion-v2',f"http://127.0.0.1:{g['bp']}",g['CONF']/'runtime/task-service/job-service-completion.token'],'job')
    g['wait_for'](lambda:sock.exists() if daemon.poll() is None else False,20)
    with socket.socket(socket.AF_UNIX) as peer:
        peer.connect(str(sock)); pid,uid,_=struct.unpack('3i',peer.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12))
    assert uid==os.getuid() and pid==daemon.pid
    desc={'version':1,'adapter':g['verified'](binary),'launcher':g['verified'](g['STOCK']),'endpoint':str(sock),
        'api_token_file':str(g['HOME']/'job-api'),'grant_key_file':str(g['HOME']/'job-grant'),
        'daemon_pid':pid,'daemon_start_ticks':int(g['process_identity'](pid)[0])}
    review=g['action']({'operation':'review_runtime','cutex_session_id':g['durable'],'restart':False,'job_mcp':desc})
    request={'operation':'run','action_id':'job-v3-launch','review':review}
    current=g['action'](request)
    assert current['stage']=='ready' and g['action'](request)==current
    g['stock_pids'].append(current['binding']['pid'])
    stock_birth[current['binding']['pid']]=g['process_identity'](current['binding']['pid'])[0]
    (g['RUN']/'launch.json').write_text(json.dumps(current,indent=2))
    return current

body = r'''
    context.update(globals())
    assert init['externalInputVersions']==[1,2]
    inventory=r.call('mcpServerStatus/list',{'threadId':thread,'detail':'toolsAndAuthOnly','limit':100})
    jobs=[s for s in inventory['data'] if s['name']=='cutex_job']
    assert len(jobs)==1 and jobs[0]['runtimeStatus']=='connected' and len(jobs[0]['tools'])==4
    (RUN/'inventory.json').write_text(json.dumps(inventory))
    assert Model.calls==0
    # Same CLI initiates the turn and owns its ordinary approval interaction.
    terminal=Terminal()
    terminal.wait('gpt-5.6-terra')
    prompt='Run the authorized private Job once, then read stdout on completion.'
    os.write(terminal.master,b'\x1b[200~'+prompt.encode()+b'\x1b[201~')
    terminal.wait(prompt)
    interaction_started=time.monotonic()
    os.write(terminal.master,b'\r')
    approval_position=len(terminal.screen)
    while not driver or driver.phase!='done':
        def new_approval_or_done():
            assert time.monotonic()-interaction_started<180, 'interaction budget'
            plain=re.sub(rb'\x1b\[[0-9;?<>=]*[ -/]*[@-~]',b'',terminal.screen[approval_position:])
            match=re.search(rb'Allow the cutex_job MCP server to run tool "([a-z_]+)"\?',plain)
            if match: return match[1].decode()
            return 'done' if driver and driver.phase=='done' else None
        tool=wait_for(new_approval_or_done,180)
        if tool=='done': break
        approval_position=len(terminal.screen)
        assert tool in ('submit','query','read_output'), 'unexpected approval'
        if tool not in approvals:
            os.write(terminal.master,b'\r');approvals.append(tool)
    terminal.wait('v3-job-output acknowledged')
    state=wait_for(lambda:json.loads((RUN/'job-state/state.json').read_text()) if (RUN/'job-state/state.json').exists() else None)
    assert len(state['jobs'])==1
    jid=next(iter(state['jobs']))
    def delivered_job():
        j=json.loads((RUN/'job-state/state.json').read_text())['jobs'][jid]
        return j if j['completionDelivery']['state']=='delivered' else None
    job=wait_for(delivered_job)
    assert job['state']=='exited' and job['exitCode']==0
    assert job['request']['actionId']=='v3-private-job'
    assert job['request']['origin']['nativeThreadId']==thread
    matches=[m['snapshot'] for m in ledger()['messages'].values() if m['snapshot'].get('externalInput',{}).get('view')]
    assert len(matches)==1
    snap=matches[0]; envlp=snap['externalInput']; view=envlp['view']; text=envlp['message']['text']
    assert snap['state']=='delivered' and snap['externalInputReceipt'] and not snap.get('presentation')
    assert text.count(jid)==1 and 'Summary:' not in text and 'Output reference:' not in text
    assert view['schema']=='cutex.job-completion.v1'
    facts=view['data']; assert facts['actionId']=='v3-private-job' and facts['exitCode']==0
    assert facts['execution']['observedRunDurationMillis']>0
    assert facts['stdout']['observedBytes']==13 and facts['stdout']['retainedBytes']==13
    # Normal MCP results may contain the reference. Only the native external
    # projection must omit view facts; never confuse tool output with a leak.
    external=[json.loads(i['output']) for req in Model.requests for i in req.get('input',[])
        if i.get('type')=='function_call_output' and i.get('name')=='external_event']
    assert external and all(e['text']==text and set(e)=={'source','type','text'} for e in external)
    assert facts['outputReference'] not in json.dumps(external)
    assert 'cutex.job-completion.v1' not in json.dumps(external)
    assert '76332d6a6f622d6f7574707574' in json.dumps(Model.requests)
    terminal.wait('Job completed');terminal.wait('Observed run');terminal.wait('v3-private-job')
    terminal.close();terminal=None
    (RUN/'terminal.pty').rename(RUN/'terminal-first.pty')
    timeline=r.call('thread/timeline/list',{'threadId':thread,'limit':100})
    (RUN/'timeline.json').write_text(json.dumps(timeline,ensure_ascii=False))
    before=json.dumps(snap['externalInputReceipt'],sort_keys=True)
    # Reconnect same owner and CLI; no restart-race campaign or extra turn.
    r,init=native(current)
    replay=r.call('thread/timeline/list',{'threadId':thread,'limit':100})
    assert replay==timeline
    terminal=Terminal();terminal.wait('Job completed');terminal.wait('Observed run');terminal.close();terminal=None
    assert json.dumps(snapshot(envlp['message']['id'])['externalInputReceipt'],sort_keys=True)==before
    assert driver.phase=='done' and driver.submit_count==driver.output_count==1 and Model.calls<=16
    assert len(json.loads((RUN/'job-state/state.json').read_text())['jobs'])==1
    (RUN/'PASS.json').write_text(json.dumps({'durable':durable,'thread':thread,'jobId':jid,'job':job,'snapshot':snap,
        'modelRequests':Model.calls,'approvals':approvals,'reconnectReplay':True,'actualCodeModeJob':True,
        'grantInjection':False,'network':'private network namespace + pre-connect tripwire'},ensure_ascii=False,indent=2))
'''
exec(compile(prefix + body + cleanup, str(base_path), 'exec'), globals())
