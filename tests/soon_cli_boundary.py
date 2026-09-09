"""S6f same-owner Cutex CLI attach smoke, private namespace/fake Responses.

Reuses accepted fixture setup and the native terminal reader, not campaigns.
This namespace smoke does not claim nested OS sandbox execution.
"""
import ast
import sys
from pathlib import Path

base = Path(__file__).with_name('soon_task_boundary.py').read_text()
ending = "exec(compile(ast.fix_missing_locations(tree),'S6f-private-Task','exec'),globals())"
assert ending in base
exec(compile(base.replace(ending, ''), 'S6f-setup-builder', 'exec'), globals())
task_setup=campaign[campaign.index('def mutation('):campaign.index('busy_mode=')]
review_line="        review=action({'operation':'review','cutex_session_id':ident,'contract':contract})"
assert review_line in source
source=source.replace(review_line, "        before=store()\n        action({'operation':'review','cutex_session_id':ident,'contract':{**contract,'version':1}},ok=False)\n        assert store()==before\n"+review_line)
host_mode = len(sys.argv)>3 and sys.argv[3]=='host'
if host_mode:
    # Same-host-UID exception from S7: fixed guard, owned listener only. Not
    # hostile-child OS isolation. Parent and child probes both deny before
    # syscall; the product feature accepts only this exact guard artifact.
    old="assert socket.if_nameindex() == [(1, 'lo')], 'run in private network namespace'"
    assert old in source
    source=source.replace(old,"assert os.environ.get('LD_PRELOAD') == '/mnt/mambo/PersonaProjects/cutex-light-core-r1/artifacts/s7a/connect-guard-error.so'")
    source=source.replace("CONTROLLER = Path", "env['CUTEX_STOCK_TEST_GUARDED_NATIVE']='1'\nCONTROLLER = Path")
    source=source.replace("    bus=owner([CUTEX", "    os.environ['S4_TEST_ALLOWED_PORTS']=env['S4_TEST_ALLOWED_PORTS']\n    bus=owner([CUTEX")
    source=source.replace("    management=owner([CUTEX", "    os.environ['S4_TEST_ALLOWED_PORTS']=env['S4_TEST_ALLOWED_PORTS']\n    management=owner([CUTEX")
terminal_source = Path('/mnt/mambo/PersonaProjects/cutex-light-core-r1/artifacts/s7a/terminal_probe_error.py').read_text()
terminal = next(n for n in ast.parse(terminal_source).body if isinstance(n, ast.ClassDef) and n.name == 'Terminal')
init = next(n for n in terminal.body if isinstance(n, ast.FunctionDef) and n.name == '__init__')
args = next(n for n in init.body if isinstance(n, ast.Assign) and any(isinstance(t, ast.Name) and t.id == 'args' for t in n.targets))
args.value = ast.parse("[str(CUTEX), 'session', 'stock-attach', thread]", mode='eval').body
campaign = '''
from types import SimpleNamespace
h=SimpleNamespace(RUN=RUN,ENV=env)
terminal=None
stock=launch(legacy,'s6f-cli-worker')
current=launch(durable,'s6f-cli-director')
rpc,cap=native_rpc(stock)
assert 'soon' in cap['externalInputDeliveries']
rpc.call('thread/name/set',{'threadId':threads[1],'name':'Private terminal fixture'})
try:
    terminal=Terminal('cutex-attach',None,legacy)
    terminal.wait(b'unknown-private-model')
    assert Model.calls==0
    terminal.prompt(b'Private genuine CLI input')
    terminal.wait(b'Private S5a fixture complete')
    assert Model.calls==1
    Model.pending=('s6f-list','cutex_agent_list',{'all_groups':False,'all_hosts':False})
    terminal.prompt(b'Private configured MCP list request')
    deadline=time.monotonic()+60
    while Model.output is None:
        assert terminal.child.poll() is None and time.monotonic()<deadline
        threading.Event().wait(.02)
    output=Model.output
    if isinstance(output,str):output=json.loads(output)
    if isinstance(output,list):
        values=[json.loads(c['text']) for c in output if c.get('type')=='input_text' and c.get('text','').startswith('{')]
        assert len(values)==1,output
        output=values[0]
    if 'content' in output:output=json.loads(next(c['text'] for c in output['content'] if c.get('type')=='text'))
    assert output['ok'] and output['scope']=='local_group_visible',output
    assert BUS_TOKEN not in json.dumps(output) and HUMAN_TOKEN not in json.dumps(output)
    (RUN/'mcp-list.json').write_text(json.dumps(output))
    Model.pending=None
    #TASK_SOON
    if len(sys.argv)>3 and sys.argv[3]=='host':
        import shlex
        script='from pathlib import Path\\n'
        for label,path in [('workspace',RUN/'forbidden-write'),('outside',RUN.parent/('forbidden-'+RUN.name))]:
            script+=f"try:\\n Path({str(path)!r}).write_text('forbidden'); print('{label}:ALLOWED')\\nexcept OSError as e: print('{label}:DENIED:'+str(e.errno))\\n"
        Model.output=None
        Model.pending=('s6f-sandbox','exec_command',{'cmd':'/usr/bin/python3 -c '+shlex.quote(script),'max_output_tokens':1000})
        terminal.prompt(b'Private read-only sandbox check')
        deadline=time.monotonic()+60
        while Model.output is None:
            assert terminal.child.poll() is None and time.monotonic()<deadline
            threading.Event().wait(.02)
        assert 'workspace:DENIED:30' in str(Model.output) and 'outside:DENIED:30' in str(Model.output),Model.output
        (RUN/'sandbox-result.json').write_text(json.dumps(Model.output))
        Model.pending=None
        Model.output=None
        Model.pending=('s6f-approval','exec_command',{'cmd':'printf forbidden > '+shlex.quote(str(RUN/'must-not-exist')),'sandbox_permissions':'require_escalated','justification':'Private S6f approval fixture: decline this request.'})
        terminal.prompt(b'Private explicit approval check')
        terminal.wait(b'Would you like to run')
        os.write(terminal.master,b'\\x1b')
        terminal.wait(b'Conversation interrupted')
        assert not (RUN/'must-not-exist').exists()
        Model.pending=None
        calls=Model.calls
        paused=call(current,thread,'/api/messages/send',{'to':legacy,'content':'Private Soon must respect Human interruption','external_message_id':'s6f-paused','delivery_mode':'soon','kind':'message','from_agent_id':current['runtime_agent_id'],'all_groups':False,'all_hosts':False})['id']
        deadline=time.monotonic()+60
        while True:
            state=ledger()['messages'][paused]['snapshot']
            if state.get('externalInputLastObserved'):break
            assert time.monotonic()<deadline,state
            threading.Event().wait(.02)
        assert state['state']=='pending' and 'externalInputReceipt' not in state,state
        frozen=state['externalInput']
        params={'version':1,'ownerId':legacy,'threadId':threads[1],'runtimeGeneration':stock['expected_generation'],'messages':[{'messageId':paused,'semanticSha256':frozen['semanticSha256']}]}
        for _ in range(3):
            status=rpc.call('thread/externalInput/status',params)['statuses'][0]
            assert status['deliveryState']=='pending' and Model.calls==calls,status
        (RUN/'pause-status.json').write_text(json.dumps(status))
    terminal.close();terminal=None
    record=next(r for r in store()['sessions'].values() if r['cutex_session_id']==legacy)
    assert record['runtime_generation']==stock['expected_generation']
    assert process_identity(stock['binding']['pid']) is not None
    assert sha(NATIVE/'config.toml')==shared_sha
    (RUN/'PASS-CLI.json').write_text(json.dumps({'durable':legacy,'native':threads[1],'generation':stock['expected_generation'],'same_owner_pid':stock['binding']['pid'],'model_calls':Model.calls,'boundary':'actual Cutex stock-attach and coherent native CLI; controlled same-host-UID guard and actual read-only sandbox' if len(sys.argv)>3 and sys.argv[3]=='host' else 'actual Cutex stock-attach and coherent native CLI; namespace smoke, not nested sandbox proof'}))
finally:
    if terminal is not None:terminal.close(check=False)
'''
task_step=task_setup+'''
assert director('assign','s6f-cli-assign',project_id='s6c2-project',task_id='t-s6c2',task_revision=1,assignment_id='a-s6c2',assignee_cutex_session_id=legacy,summary='Private assignment')['status']=='committed'
deadline=time.monotonic()+90
while True:
    try: messages=[(k,v['snapshot']) for k,v in ledger()['messages'].items() if k.startswith('tsa_')]
    except FileNotFoundError: messages=[]
    if messages and messages[0][1]['state']=='delivered':break
    assert time.monotonic()<deadline,messages
    threading.Event().wait(.02)
assert messages[0][1]['externalInput']['message']['delivery']=='soon'
terminal.wait(b'Private Task Soon a-s6c2 observed')
(RUN/'task-soon.json').write_text(json.dumps(messages[0][1]))
assert worker('start','s6f-cli-start')['status']=='committed'
assert worker('block','s6f-cli-block',summary='Private actionable fixture blocker')['status']=='committed'
deadline=time.monotonic()+90
while True:
    completion=[v['snapshot'] for k,v in ledger()['messages'].items() if k.startswith('tsc_')]
    if completion and all(s['state']=='delivered' for s in completion):break
    assert time.monotonic()<deadline,completion
    threading.Event().wait(.02)
assert completion[0]['externalInput']['message']['delivery']=='soon'
assert worker('resume','s6f-cli-resume')['status']=='committed'
assert director('cancel','s6f-cli-cancel',assignment_id='a-s6c2')['status']=='committed'
deadline=time.monotonic()+90
while True:
    completion=[v['snapshot'] for k,v in ledger()['messages'].items() if k.startswith('tsc_')]
    if len(completion)==2 and all(s['state']=='delivered' for s in completion):break
    assert time.monotonic()<deadline,completion
    threading.Event().wait(.02)
assert all(s['externalInput']['message']['delivery']=='soon' for s in completion)
(RUN/'owner-action-closure.json').write_text(json.dumps(completion))
'''
campaign=campaign.replace('    #TASK_SOON', '\n'.join('    '+line if line else '' for line in task_step.splitlines()))
tree = ast.parse(source)
model_source=Path(__file__).with_name('stock_task_mcp_boundary.py').read_text()
model_source=model_source.replace('if n==0:', "if n==0 and name!='exec_command':")
model_source=model_source.replace("'namespace':'mcp__cutex','name':name", "**({} if name=='exec_command' else {'namespace':'mcp__cutex'}),'name':name")
model = next(n for n in ast.parse(model_source).body if isinstance(n,ast.ClassDef) and n.name=='Model')
model.body[:0] = ast.parse('requests=[]').body
handler = next(n for n in model.body if isinstance(n,ast.FunctionDef) and n.name=='do_POST')
read = next(i for i,n in enumerate(handler.body) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='data' for t in n.targets))
handler.body[read+1:read+1] = ast.parse('Model.requests.append(data)').body
item_index=next(i for i,n in enumerate(handler.body) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='item' for t in n.targets))
handler.body[item_index+1:item_index+1]=ast.parse('''
if any(i.get('name')=='external_event' and 'Assignment ID: a-s6c2' in i.get('output','') for i in data.get('input',[])):
    item['content'][0]['text']='Private Task Soon a-s6c2 observed'
''').body
tree.body[next(i for i,n in enumerate(tree.body) if isinstance(n,ast.ClassDef) and n.name=='Model')] = model
probe = next(n for n in tree.body if isinstance(n, ast.Try))
cut = next(i for i,n in enumerate(probe.body) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='stock' for t in n.targets))
probe.body = probe.body[:cut] + ast.parse(campaign).body
tree.body.insert(tree.body.index(probe), terminal)
tree.body[tree.body.index(probe):tree.body.index(probe)] = helpers
exec(compile(ast.fix_missing_locations(tree), 'S6f-private-CLI', 'exec'), globals())
