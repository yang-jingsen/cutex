"""Private S6c2 real Bus/provider/native consumer. No model tools or live state.
Reuses only the accepted S6c1 setup, frozen bundle and owned cleanup entrance.
"""
import ast
from pathlib import Path

_setup_ast=ast.parse(Path(__file__).with_name('external_input_boundary.py').read_text())
body_index=next(i for i,n in enumerate(_setup_ast.body) if isinstance(n,ast.Try))
probe=_setup_ast.body[body_index]
exec(compile(ast.Module(body=_setup_ast.body[:body_index],type_ignores=[]),'S6c1-setup','exec'),globals())
setup_end=next(i for i,n in enumerate(probe.body) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='stock' for t in n.targets))
fault_mode=len(sys.argv)>3 and sys.argv[3]=='fault'
gate_listener=None
if fault_mode:
    gate_listener=socket.socket(socket.AF_UNIX);gate_path=HOME/'delivery-gate.sock';gate_listener.bind(str(gate_path));gate_listener.listen()
    env['CUTEX_NATIVE_DELIVERY_TEST_GATE']=str(gate_path)
    env['CUTEX_NATIVE_DELIVERY_TEST_MESSAGE']='s6c2-ordinary'
    gate_requests=queue.Queue();gate_seen=set()
    def gates():
        while True:
            try: conn,_=gate_listener.accept()
            except OSError:return
            ident=conn.makefile('rb').readline().decode().strip()
            if ident in gate_seen:
                conn.sendall(b'continue\n');conn.close()
            else:
                gate_seen.add(ident);gate_requests.put((ident,conn))
    threading.Thread(target=gates,daemon=True).start()

def ledger():
    return json.loads((CONF/'runtime/management-v2/agent-bus-message-state.json').read_text())
def delivered(mid):
    deadline=time.monotonic()+90
    while True:
        snapshot=ledger()['messages'][mid]['snapshot']
        if snapshot['state']=='delivered':
            assert snapshot['externalInputReceipt']['schema']=='codex.external-input-receipt.v1'
            return snapshot
        assert time.monotonic()<deadline,snapshot
        threading.Event().wait(.05) # observe authoritative business state, not persistence barrier
def hs(receipt,native):
    return {'X-Cutex-Agent-Id':receipt['runtime_agent_id'],'X-Cutex-Mcp-Thread-Id':native,'X-Cutex-Mcp-Generation':str(receipt['expected_generation'])}
def call(receipt,native,path,payload):
    code,value=api(bp,path,payload,token=BUS_TOKEN,headers=hs(receipt,native))
    assert code==200,(code,value)
    facts.append({'path':path,'status':value.get('status'), 'outcome':value.get('outcome',{}).get('kind'), 'action_id':value.get('action_id')});return value
def send(mode,text,external):
    return call(stock,threads[1],'/api/messages/send',{'to':durable,'content':text,'external_message_id':external,'delivery_mode':mode,'kind':'message','from_agent_id':stock['runtime_agent_id'],'all_groups':False,'all_hosts':False})
def director(op,ident,**fields):
    return call(current,thread,'/api/task/v2/director-action',{'schema':'cutex/task-service-director-action/v2','operation':op,'action_id':ident,**fields})
def worker(op,ident,**fields):
    semantic={'operation':op,'body':{'schema':'cutex/task-service-action/v2','action_id':ident,'assignment_id':'a-s6c2',**fields}}
    prepared=call(stock,threads[1],'/api/task/v2/worker-prepare',{'schema':'cutex/task-service-worker-prepare/v2','action':semantic})
    assert prepared['outcome']['kind']=='prepared',prepared
    result=call(stock,threads[1],'/api/task/v2/actions',prepared['outcome']['body'])
    assert result['outcome']['kind']=='committed',result
    return {'status':'committed'}

try:
    exec(compile(ast.Module(body=probe.body[:setup_end],type_ignores=[]),'S6c1-private-launch-setup','exec'),globals())
    stock=launch(legacy,'s6c2-stock')
    current=launch(durable,'s6c2-native')
    rpc,init=native_rpc(current);assert init['externalInputVersion']==1
    ordinary=send('after_turn','Private ordinary payload 私有','s6c2-ordinary')
    assert ordinary['ok'],ordinary
    mid=ordinary['id']
    if fault_mode:
        gated,connection=gate_requests.get(timeout=45);assert gated==mid
        pending=ledger()['messages'][mid]['snapshot'];assert pending['state']=='pending' and 'externalInputReceipt' not in pending
        original=pending['externalInputLastObserved']['receipt'];assert original
        # Actual owner-process death after native A4 and before Bus commit.
        stop_owned(management);connection.close()
        management=owner([CUTEX,'management','serve','--port',mp],'management-recovered');ready(mp,management)
        current=launch(durable,'s6c2-crash-restart',restart=True)
        first=delivered(mid);assert first['externalInputReceipt']==original and first['externalInputCommitGeneration']==2
        race=send('after_turn','Private stale-generation race','s6c2-ordinary');raceid=race['id']
        gated,connection=gate_requests.get(timeout=45);assert gated==raceid
        old=ledger()['messages'][raceid]['snapshot'];assert old['state']=='pending'
        race_receipt=old['externalInputLastObserved']['receipt']
        results=queue.Queue()
        def restarting():
            try:results.put(launch(durable,'s6c2-race-restart',restart=True))
            except BaseException as error:results.put(error)
        threading.Thread(target=restarting,daemon=True).start()
        deadline=time.monotonic()+30
        while True:
            actions=store().get('explicit_launch_receipts',{})
            stage=actions.get('s6c2-race-restart',{})
            if 'prepared' in json.dumps(stage):break
            assert time.monotonic()<deadline,'restart did not acquire lifecycle fence'
            threading.Event().wait(.02)
        connection.sendall(b'continue\n');connection.close()
        current=results.get(timeout=90);assert isinstance(current,dict),type(current).__name__
        final=delivered(raceid);assert final['externalInputReceipt']==race_receipt and final['externalInputCommitGeneration']==3
        (RUN/'PASS-FAULT.json').write_text(json.dumps({'durable':durable,'native':thread,'generations':[1,2,3],'messages':[mid,raceid],'receipts':[original,race_receipt],'oracle':'real management process death and concurrent explicit restart before business commit'},indent=2))
        sys.exit(0)
    first=delivered(mid)
    assert first['externalInput']['message']['source']=={'kind':'agent','id':legacy}
    assert first['externalInput']['message']['text']=='Message Type: MESSAGE\nPayload:\n[message from Private S6 Agent 1] Private ordinary payload 私有'
    if len(sys.argv)>3 and sys.argv[3]=='modes':
        def observed(mid,predicate):
            deadline=time.monotonic()+45
            while True:
                item=ledger()['messages'][mid]['snapshot']
                if predicate(item):return item
                assert time.monotonic()<deadline,item
                threading.Event().wait(.02)
        soon=send('soon','Unsupported mode remains pending','s6c2-soon')['id']
        observed(soon,lambda s:'does not support soon' in json.dumps(s.get('error')))
        passive=send('passive','Passive must not wake idle owner','s6c2-passive')['id']
        passive_state=observed(passive,lambda s:s.get('externalInputLastObserved',{}).get('deliveryState')=='pending')
        assert passive_state['state']=='pending' and 'externalInputReceipt' not in passive_state
        wake=send('after_turn','Independent eligible message is not blocked','s6c2-wake')['id']
        delivered(wake);delivered(passive)
        assert ledger()['messages'][soon]['snapshot']['state']=='pending'
        Model.empty=True
        empty=send('after_turn','No output does not authorize automatic retry','s6c2-empty')['id']
        snapshot=delivered(empty)
        params={'version':1,'ownerId':durable,'threadId':thread,'runtimeGeneration':current['expected_generation'],
            'messages':[{'messageId':empty,'semanticSha256':snapshot['externalInput']['semanticSha256']}]}
        deadline=time.monotonic()+30
        while True:
            native=rpc.call('thread/externalInput/status',params)['statuses'][0]
            if native['processing']['state']=='held':break
            assert time.monotonic()<deadline,native
        assert native['processing']['reason']=='no_output'
        assert native['receipt']==snapshot['externalInputReceipt']
        # Repeated status is a read, never retry permission or an extra wake.
        calls=Model.calls
        for _ in range(3):assert rpc.call('thread/externalInput/status',params)['statuses'][0]['processing']==native['processing']
        assert Model.calls==calls
        (RUN/'PASS-MODES.json').write_text(json.dumps({'soon':soon,'passive':passive,'wake':wake,'no_output':empty,'model_calls':calls,'native':thread,'durable':durable},indent=2))
        sys.exit(0)
    # Existing Human APIs build actual project authority; facade never holds root.
    def mutation(action_id,kind,revision,**fields):
        return {'schema':'cutex/human-management-project-mutation/v1','action_id':action_id,'project_id':'s6c2-project','expected_authority_epoch':0 if kind=='create' else 1,'expected_project_revision':revision,'operation':{'kind':kind,**fields}}
    create=mutation('s6c2-project','create',0,director_cutex_session_id=durable,presentation={'display_name':'S6c2 Private','badge_label':'S6','color':'cyan'})
    for n,ident in enumerate(ids):
        code,candidates=api(mp,'/v2/agent-management/durable-candidates');assert code==200
        candidate=next(c for c in candidates if c['cutex_session_id']==ident)
        code,result=api(mp,'/v2/agent-management/durable-import',{'action_id':f's6c2-import-{n}','candidate':candidate,'confirmed_formal_name':f'Private S6 Agent {n}','assignment':create if n==0 else None,'detach':None})
        assert code==200 and result['complete'],(code,result)
    code,result=api(mp,'/v2/agent-management/project-mutations',mutation('s6c2-member','add_member',1,cutex_session_id=legacy));assert code==200,(code,result)
    contract='Private completion fixture; no external actions.'
    created=director('create_revision','s6c2-create',project_id='s6c2-project',workflow_id='wf-s6c2',task_id='t-s6c2',task_revision=1,opaque_contract=contract,contract_sha256=hashlib.sha256(contract.encode()).hexdigest(),completion_policy='director_acceptance')
    assert created['status']=='committed',created
    assigned=director('assign','s6c2-assign',project_id='s6c2-project',task_id='t-s6c2',task_revision=1,assignment_id='a-s6c2',assignee_cutex_session_id=legacy,summary='Private fixture')
    assert assigned['status']=='committed',assigned
    assert worker('start','s6c2-start')['status']=='committed'
    assert worker('submit','s6c2-submit',result_sha256='a'*64,result_reference='private-result:s6c2')['status']=='committed'
    deadline=time.monotonic()+90
    while True:
        completion=[(k,v['snapshot']) for k,v in ledger()['messages'].items() if k.startswith('tsc_')]
        if completion: break
        assert time.monotonic()<deadline,'no actual Task canonical projection'
        threading.Event().wait(.05)
    taskmid=completion[0][0];taskreceipt=delivered(taskmid)
    assert taskreceipt['externalInput']['message']['source']=={'kind':'service','id':'cutex-task-service'}
    assert taskreceipt['externalInput']['message']['text'].count('a-s6c2')==1
    assert 'tsn-' not in taskreceipt['externalInput']['message']['text']
    from external_delivery_job import run_job
    job=run_job(globals())
    (RUN/'PASS-PARTIAL.json').write_text(json.dumps({'ordinary':mid,'task':taskmid,'job':job,'durable':durable,'native':thread,'receipts':[first['externalInputReceipt'],taskreceipt['externalInputReceipt']]},indent=2))
finally:
    if gate_listener:gate_listener.close()
    exec(compile(ast.Module(body=probe.finalbody,type_ignores=[]),'S6c1-owned-cleanup','exec'),globals())
