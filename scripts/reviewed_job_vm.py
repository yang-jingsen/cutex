"""Private VM actual Core -> configured Job MCP proof. No metadata injection.

Uses the immutable earlier private bootstrap fixture as setup only. This file
must live in a new owned root/fixtures with base-fixture.py beside it; binaries
are new root/bin, native remains the frozen sibling vm-r1 bundle.
"""
import json
from pathlib import Path
import time
import threading
import sys
import hashlib
import os
import stat

CONTEXT = {}
STEP = 0
MODE = sys.argv[2] if len(sys.argv)>2 else 'fullaccess'
assert MODE in ('fullaccess', 'readonly', 'decline')

# Pure local preflight, before base fixture starts any child or creates state.
fixture_root=Path(__file__).absolute().parents[1]
assert fixture_root.resolve()==fixture_root
assert stat.S_ISDIR(fixture_root.lstat().st_mode) and fixture_root.stat().st_uid==os.getuid()
fixture_run=fixture_root/sys.argv[1]
assert fixture_run.parent==fixture_root and not os.path.lexists(fixture_run)
socket_candidates=[fixture_run/'bootstrap.sock',fixture_run/'h/job.sock',fixture_run/'h/.cutex/runtime/app-server/000000000000/s']
for candidate in socket_candidates:
    assert len(os.fsencode(candidate))<=100, ('socket preflight failed',str(candidate),len(os.fsencode(candidate)))
    assert not os.path.lexists(candidate)
print(json.dumps({'preflight':'owned-new-short-path','socketBytes':[len(os.fsencode(p)) for p in socket_candidates]}),flush=True)

def model_response(self):
    global STEP
    g = CONTEXT
    assert self.path == '/v1/responses'
    request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
    g['Model'].requests.append(request)
    g['Model'].calls += 1
    assert g['Model'].calls <= 12
    tools = request.get('tools', [])
    (g['RUN']/'model-tools.json').write_text(json.dumps(tools, indent=2))
    n = STEP
    STEP += 1
    if n in (0, 1) and not (MODE=='decline' and n==1):
        assert any(t.get('type')=='namespace' and t.get('name')=='mcp__cutex_job' for t in tools), 'Job namespace not actually advertised'
        args = {'actionId':'configured-core-job', 'argv':['/bin/sh','-c','printf configured-core-output'], 'cwd':str(g['RUN'])}
        if MODE=='readonly':
            args['argv']=['/bin/sh','-c','cat probe-readable; if printf no > probe-denied-a; then exit 71; fi; if printf no > probe-denied-b; then exit 72; fi; printf configured-core-output']
        item = {'type':'function_call','call_id':f'job-submit-{n}','namespace':'mcp__cutex_job','name':'submit','arguments':json.dumps(args)}
    elif n==2 and MODE=='readonly':
        args={'actionId':'wrong-cwd-rejected','argv':['/bin/true'],'cwd':str(g['RUN']/'other-cwd')}
        item={'type':'function_call','call_id':'job-wrong-cwd','namespace':'mcp__cutex_job','name':'submit','arguments':json.dumps(args)}
    elif n in ((3,4) if MODE=='readonly' else (2,3)) and MODE!='decline':
        # Await actual authoritative terminal state, not incidental roundtrips.
        deadline = time.monotonic()+30
        while True:
            state = json.loads((g['RUN']/'job-state/state.json').read_text())
            if state['jobs'] and all(j['state']=='exited' for j in state['jobs'].values()): break
            assert time.monotonic()<deadline, 'configured Job did not exit'
            threading.Event().wait(.02)
        assert len(state['jobs'])==1
        jid = next(iter(state['jobs']))
        args = {'jobId':jid}
        read_output=n==(4 if MODE=='readonly' else 3)
        if read_output: args['stream']='stdout'
        item = {'type':'function_call','call_id':f'job-read-{n}','namespace':'mcp__cutex_job','name':'read_output' if read_output else 'query','arguments':json.dumps(args)}
    else:
        item = {'type':'message','role':'assistant','id':f'job-done-{n}','content':[{'type':'output_text','text':'Private configured Job proof complete'}]}
    events = [{'type':'response.created','response':{'id':f'job-{n}'}}, {'type':'response.output_item.done','item':item}, {'type':'response.completed','response':{'id':f'job-{n}','usage':{'input_tokens':0,'output_tokens':0,'total_tokens':0}}}]
    body = ''.join('event: '+e['type']+'\ndata: '+json.dumps(e)+'\n\n' for e in events).encode()
    self.send_response(200); self.send_header('Content-Type','text/event-stream'); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)

def prepared_launch(g):
    import os
    import socket
    import struct
    root, home = g['RUN'], g['HOME']
    binary = g['ROOT']/'bin/cutex-job-service'
    assert g['sha'](binary)=='d98d5e33322c7200eb9b149bc39d99da3bb169c994fe14c7f401d26c06d7b1f2'
    home.chmod(0o700)
    (root/'probe-readable').write_text('private-read-success\n')
    (root/'other-cwd').mkdir()
    for name in ['job-api','job-grant']:
        p = home/name; p.write_bytes(os.urandom(32)); p.chmod(0o600)
    sock = home/'job.sock'
    daemon_args=[binary,'serve',root/'job-state',sock,home/'job-api',home/'job-grant',g['STOCK'],'--completion',f"http://127.0.0.1:{g['bp']}",g['CONF']/'runtime/task-service/job-service-completion.token']
    if MODE=='readonly':
        # Trace only this owned daemon's exec boundaries. No environment dump,
        # live process discovery, credentials content, or model metadata proxy.
        daemon_args=['/usr/bin/strace','-f','-e','trace=execve','-s','512','-o',root/'job-exec.trace',*daemon_args]
    daemon = g['owner'](daemon_args,'job')
    until = time.monotonic()+20
    while not sock.exists():
        assert daemon.poll() is None and time.monotonic()<until
        threading.Event().wait(.02)
    with socket.socket(socket.AF_UNIX) as peer:
        peer.connect(str(sock))
        daemon_pid,uid,_=struct.unpack('3i',peer.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12))
    assert uid==os.getuid() and daemon_pid in g['owned_tree'](daemon.pid)
    descriptor = {'version':1,'adapter':g['verified'](binary),'launcher':g['verified'](g['STOCK']),'endpoint':str(sock),'api_token_file':str(home/'job-api'),'grant_key_file':str(home/'job-grant'),'daemon_pid':daemon_pid,'daemon_start_ticks':int(g['process_identity'](daemon_pid)[0])}
    if MODE=='readonly':
        negative=[]
        for field,value in [('version',99),('daemon_start_ticks',descriptor['daemon_start_ticks']+1),('api_token_file',str(home/'absent')),('endpoint',str(home/'absent.sock'))]:
            bad={**descriptor,field:value}
            status,_=g['api'](g['mp'],'/v2/agent-management/explicit-launch',{'operation':'review_runtime','cutex_session_id':g['durable'],'restart':False,'job_mcp':bad})
            assert status!=200
            negative.append({'field':field,'status':status})
        status,_=g['api'](g['mp'],'/v2/agent-management/explicit-launch',{'operation':'review_runtime','cutex_session_id':g['durable'],'restart':False,'job_mcp':descriptor},token=g['BUS_TOKEN'])
        assert status==401
        negative.append({'field':'nonroot-review','status':status})
        (root/'descriptor-negatives.json').write_text(json.dumps(negative,indent=2))
    review = g['action']({'operation':'review_runtime','cutex_session_id':g['durable'],'restart':False,'job_mcp':descriptor})
    assert review['job_mcp']['descriptor']==descriptor
    request = {'operation':'run','action_id':'configured-job-launch','review':review}
    current = g['action'](request)
    assert current['stage']=='ready' and g['action'](request)==current
    g['stock_pids'].append(current['binding']['pid'])
    if MODE=='readonly':
        changed=json.loads(json.dumps(request)); changed['review']['job_mcp']['descriptor']['version']=99
        g['action'](changed,ok=False)
        without=g['action']({'operation':'review_runtime','cutex_session_id':g['durable'],'restart':True})
        g['action']({'operation':'run','action_id':'no-silent-job-omission','review':without},ok=False)
        assert g['process_identity'](current['binding']['pid']) is not None
        fresh=g['action']({'operation':'review_runtime','cutex_session_id':g['durable'],'restart':True,'job_mcp':descriptor})
        restart={'operation':'run','action_id':'reviewed-job-restart','review':fresh}
        next_owner=g['action'](restart)
        assert next_owner['stage']=='ready' and g['action'](restart)==next_owner
        assert next_owner['runtime_agent_id']!=current['runtime_agent_id']
        assert next_owner['review']['subject']['cutex_session_id']==g['durable']
        g['stock_pids'].append(next_owner['binding']['pid'])
        (root/'first-launch-receipt.json').write_text(json.dumps(current,indent=2))
        current=next_owner
    (root/'review.json').write_text(json.dumps(review,indent=2))
    (root/'launch-receipt.json').write_text(json.dumps(current,indent=2))
    return current

def run_job(g):
    rpc, init = g['native_rpc'](g['current'])
    rpc.call('thread/resume', {'threadId':g['thread']})
    inventory = rpc.call('mcpServerStatus/list', {'threadId':g['thread'],'detail':'toolsAndAuthOnly'})
    assert any(s['name']=='cutex_job' and len(s['tools'])>=4 for s in inventory['data']), inventory
    (g['RUN']/'mcp-inventory.json').write_text(json.dumps(inventory,indent=2))
    started = rpc.call('turn/start', {'threadId':g['thread'],'input':[{'type':'text','text':'Run the private configured Job probe.'}]})
    deadline = time.monotonic()+90
    notifications=[]
    pending=list(rpc.events); rpc.events.clear()
    while time.monotonic()<deadline:
        msg = pending.pop(0) if pending else rpc.messages.get(timeout=max(.1,deadline-time.monotonic()))
        notifications.append(msg)
        (g['RUN']/'native-events.json').write_text(json.dumps(notifications,indent=2))
        if msg.get('method')=='turn/completed' and msg['params']['turn']['id']==started['turn']['id']: break
        if 'id' in msg and 'method' in msg:
            if msg['method']=='mcpServer/elicitation/request':
                assert msg['params']['serverName']=='cutex_job'
                rpc.send(json.dumps({'id':msg['id'],'result':{'action':'decline' if MODE=='decline' else 'accept','content':{},'_meta':None}}).encode())
                continue
            raise AssertionError(('native request requires explicit fixture handling',msg['method'],msg.get('params')))
    else: raise AssertionError('native turn did not complete')
    if MODE=='decline':
        state_path=g['RUN']/'job-state/state.json'
        assert not state_path.exists() or not json.loads(state_path.read_text())['jobs']
        assert any(m.get('method')=='mcpServer/elicitation/request' for m in notifications)
        return {'mode':MODE,'approvalDeclined':True,'noJobCreated':True,'actualCore':True}
    state = json.loads((g['RUN']/'job-state/state.json').read_text())
    assert len(state['jobs'])==1, state
    jid, job = next(iter(state['jobs'].items()))
    assert job['state']=='exited' and job['request']['origin']['nativeThreadId']==g['thread']
    assert job['request']['subscriberCutexSessionId']==g['durable']
    assert job['request']['origin']['runtimeAgentId']==g['current']['runtime_agent_id']
    deadline=time.monotonic()+90
    while True:
        state=json.loads((g['RUN']/'job-state/state.json').read_text()); job=state['jobs'][jid]
        if job['completionDelivery']['state']=='delivered': break
        assert time.monotonic()<deadline, job['completionDelivery']
        threading.Event().wait(.05)
    ledger=g['ledger']()
    matched=[m['snapshot'] for m in ledger['messages'].values() if jid in json.dumps(m) and m['snapshot']['state']=='delivered']
    assert len(matched)==1 and matched[0].get('externalInputReceipt')
    (g['RUN']/'completion-ledger.json').write_text(json.dumps(matched,indent=2))
    assert 'configured-core-output'.encode().hex() in json.dumps(g['Model'].requests)
    if MODE=='readonly':
        assert job['exitCode']==0
        assert not (g['RUN']/'probe-denied-a').exists() and not (g['RUN']/'probe-denied-b').exists()
        assert job['request']['origin']['permissionProfileType']=='managed'
        assert 'private-read-success'.encode().hex() in json.dumps(g['Model'].requests)
        assert 'sandboxCwd differs from the bound request cwd' in json.dumps(g['Model'].requests)
    return {'jobId':jid,'native_thread':g['thread'],'durable':g['durable'],'job':job,'receipt':matched[0]['externalInputReceipt'],'actualCore':True,'manualMetadataOrGrant':False,'modelCalls':g['Model'].calls}

base = Path(__file__).with_name('base-fixture.py').read_text()
assert hashlib.sha256(base.encode()).hexdigest()=='ed1ebdb57d413db2f3f382aa65c1bf014e4ccb8b8483b63dd85e774717d9728e'
assert base.count('    current=launch(durable,\'vm-job-subscriber\')')==1
base = base.replace("CUTEX = FROZEN/'package/artifacts/linux/cutex'", "CUTEX = ROOT/'bin/cutex'")
base = base.replace("MCP = FROZEN/'package/artifacts/linux/cutex-mcp'", "MCP = ROOT/'bin/cutex-mcp'")
base = base.replace("CUTEX = ROOT/'bin/cutex'", "CUTEX = ROOT/'bin/cutex-final-5f752958'")
base = base.replace("model = http.server.ThreadingHTTPServer", "Model.do_POST = model_response\nmodel = http.server.ThreadingHTTPServer")
base = base.replace('actual Job daemon/process/completion; actual separate Job MCP adapter issuer, harness-driven trusted metadata', 'actual native Core configured MCP; per-scenario results below, fake Responses, no injected metadata')
if MODE=='readonly':
    base=base.replace('default_permissions=":danger-full-access"', 'default_permissions=":read-only"')
    base=base.replace("'sandbox': 'danger-full-access'", "'sandbox': 'read-only'")
    base=base.replace("'--permission', 'full-access', '--sandbox', 'danger-full-access'", "'--permission', 'read-only', '--sandbox', 'read-only'")
base = base.replace("    current=launch(durable,'vm-job-subscriber')\n    from job_mcp_helper_r2 import run_job", "    current=prepared_launch(globals())")
CONTEXT = dict(globals())
exec(compile(base, str(Path(__file__).with_name('base-fixture.py')), 'exec'), CONTEXT)
