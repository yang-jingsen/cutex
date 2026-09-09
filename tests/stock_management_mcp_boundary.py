"""S5b: reuse S5a's private stock/bootstrap/provider setup and owned cleanup.

The AST cut is explicit and asserted: no S5a task campaign is executed. All
Management calls below traverse actual stock Core tool search and configured MCP.
Run with one NEW short owned evidence directory, e.g. s5b01.
"""
import ast
from pathlib import Path


def private_executable_path():
    # This stock fixture never uses systemd scopes. Exercise the existing
    # SystemctlUnavailable direct-process fallback, not a fake scope reply or
    # the operator's user bus. Fixed compiler setup has already completed.
    bindir=RUN/'bin';bindir.mkdir()
    for name in ['bash','sh','env','python3','git','uname','hostname','ps','kill','true','false']:
        target=Path('/usr/bin')/name
        if target.is_file(): (bindir/name).symlink_to(target)
    assert not (bindir/'systemctl').exists()
    env['PATH']=str(bindir)


def management_cases():
    project = 's5a-project'  # inherited private fixture project, never a live ID
    def manage(op, action_id, actor='director', **kw):
        return mcp(actor, 'cutex_agent_management',
                   dict(operation=op, action_id=action_id, project_id=project, **kw))
    def complete(value):
        assert value['outcome']['status'] == 'complete', value
        return value['outcome']['receipt']
    def denied(value):
        assert value['outcome']['status'] in ['no_write', 'owner_action_required'], value
        assert value['outcome'].get('code') != 'invalid_provider_response', value
        return value
    def grant(role, operation, revision):
        status, result = api(mp, '/v2/agent-management/operator-actions', {
            'schema':'cutex/human-management-operator-action/v1',
            'action_id':'s5b-'+operation+'-'+role, 'project_id':project,
            'expected_authority_epoch':1, 'expected_grant_revision':revision,
            'operation':operation, 'operator_cutex_session_id':agents[role]['durable']})
        assert status == 200, (status, result)
        return result

    # Public private-fixture membership/group APIs, never hand-edited stores.
    status, result = api(mp, '/v2/agent-management/project-mutations',
        mutation('s5b-add-disposable','add_member',2,cutex_session_id=agents['disposable']['durable']))
    assert status == 200, (status,result)
    for role in ['director','worker']:
        status,result=api(bp,'/api/agents/groups',{'target':agents[role]['receipt']['runtime_agent_id'],
                         'groups':['s5b-private'],'mode':'set'},token=BUS_TOKEN)
        assert status==200 and result['ok'],(status,result)
    query=manage('query_managed','s5b-query'); complete(query)
    assert manage('query_managed','s5b-query') == query
    listed=mcp('director','cutex_agent_list',{})
    assert listed['ok'] and listed['scope']=='local_group_visible',listed
    for role in ['director','worker']:
        row=next(a for a in listed['agents'] if a['cutex_session_id']==agents[role]['durable'])
        assert row['native_session_id']==agents[role]['native'] and row['formal_name']==agents[role]['name'],row
        assert row['runtime_observation']=='registered_online' and row['mapping_observation']=='current',row
    for args in [{'all_hosts':True},{'all_groups':True},{'caller':'spoof'}]:
        value=mcp('director','cutex_agent_list',args)
        assert value['code']=='unsupported_scope_or_arguments',value
    denied(manage('query_managed','s5b-worker-denied',actor='worker'))
    denied(mcp('director','cutex_agent_management',{'operation':'query_managed','action_id':'s5b-foreign','project_id':'foreign-private-project'}))
    denied(manage('query_managed','s5b-forged',caller_cutex_session_id=agents['director']['durable']))
    for operation in ['grant_operator','activate']:
        denied(manage(operation,'s5b-forbidden-'+operation))
    missing=manage('offline','s5b-missing')
    assert missing['outcome']['code']=='missing_cutex_session_id',missing

    # Current provider privilege rules, not a new blanket role policy.
    grant('worker','grant',0)
    for op,target in [('offline','director'),('close','worker')]:
        denied(manage(op,'s5b-protected-'+target,actor='worker',cutex_session_id=agents[target]['durable']))
    grant('worker','revoke',1)
    # Explicit stock restart/online/replace must never launch the default fork.
    before={k:store()['sessions'][a['durable']] for k,a in agents.items()}
    for op in ['online','restart']:
        denied(manage(op,'s5b-stock-'+op,cutex_session_id=agents['worker']['durable']))
    spec={'name':'Explicit never-created successor','cwd':str(RUN),'profile':'alpha',
          'runtime_backend':'host','model':'gpt-5.4','reasoning':'low','permissions':'read-only',
          'approval_policy':'on-request','sandbox_mode':'read-only','groups':['s5b-private']}
    # Invalid supported-spec boundary rejects before create; no stock bootstrap
    # is claimed. replace/rotation use invalid inputs too, not spoofed readiness.
    invalid={**spec,'profile':''}  # provider validate(), before reservation or bootstrap
    denied(manage('create','s5b-create-denied',spec=invalid,start_mode='bootstrap_only'))
    denied(manage('replace','s5b-replace-denied',predecessor_cutex_session_id=agents['worker']['durable'],
                  policy='close_before_create',successor=invalid,start_mode='bootstrap_only'))
    denied(manage('director_rotate','s5b-rotate-denied',expected_predecessor_cutex_session=agents['director']['durable'],
                  expected_authority_epoch=1,mode='retain_predecessor_bootstrap_only',successor=invalid))
    for op,kw in [('create',{'spec':spec,'start_mode':'bootstrap_only'}),
                  ('replace',{'predecessor_cutex_session_id':agents['worker']['durable'],'policy':'keep_old','successor':spec,'start_mode':'bootstrap_only'}),
                  ('director_rotate',{'expected_predecessor_cutex_session':agents['director']['durable'],'expected_authority_epoch':1,'mode':'retain_predecessor_bootstrap_only','successor':spec})]:
        denied(manage(op,'s5b-role-'+op,actor='worker',**kw))
    for role,a in agents.items():
        after=store()['sessions'][a['durable']]
        for field in ['cutex_session_id','codex_session_id','runtime_generation','app_server_runtime','explicit_launch']:
            assert after.get(field)==before[role].get(field),(role,field)

    # Reuse existing outbound Task/send compatibility through actual stock.
    task={'project_id':project,'workflow_id':'s5b-workflow','task_id':'s5b-task','task_revision':1,
          'opaque_contract':'private active-task fence','completion_policy':'director_acceptance',
          'assignment_id':'s5b-assignment','assignee_cutex_session_id':agents['worker']['durable'],'summary':'private task'}
    result=director('create_and_assign','s5b-task-create',**task);assert result['status']=='committed',result
    result=worker('start','s5b-task-start',assignment='s5b-assignment');assert result['status']=='committed',result
    # Existing explicit stock review (Human fixture only) protects active Tasks.
    result=action({'operation':'review_runtime','cutex_session_id':agents['worker']['durable'],'restart':True},ok=False)
    (RUN/'active-task-refusal.json').write_text(json.dumps(result,indent=2))
    result=worker('submit','s5b-task-submit',assignment='s5b-assignment',result_sha256='a'*64,result_reference='private-s5b');assert result['status']=='committed',result
    result=director('accept_result','s5b-task-accept',assignment_id='s5b-assignment');assert result['status']=='committed',result
    result=mcp('director','query_managed',{'action_id':'s5b-legacy-query','project_id':project});assert result['outcome']['status']=='complete',result
    result=mcp('director','send',{'to':agents['worker']['durable'],'message':'private S5b outbound','external_message_id':'s5b-send','delivery_mode':'passive'})
    assert result['ok'] and result['to_cutex_session_id']==agents['worker']['durable'],result

    # Independent actual provider occurrence negatives, not just schema denial.
    raw={'schema':'cutex/agent-management/v1','operation':'query_managed','action_id':'s5b-negative','project_id':project}
    denials=[]
    for label,hs in [('stale',{**headers('director'),'X-Cutex-Mcp-Generation':'99'}),
                     ('foreign',{**headers('director'),'X-Cutex-Mcp-Thread-Id':agents['worker']['native']}),
                     ('missing',{k:v for k,v in headers('director').items() if k!='X-Cutex-Mcp-Thread-Id'})]:
        status,result=direct('director','/api/agent-management/v1/actions',raw,hs)
        assert status!=200 or result.get('outcome',{}).get('status')!='complete',(label,status,result)
        status,result=direct('director','/api/agents?all_groups=false&all_hosts=false',None,hs)
        assert status!=200 or result.get('ok')!=True,(label,status,result)
        denials.append(label)
    facade_env={**env,'CUTEX_AGENT_ID':agents['worker']['receipt']['runtime_agent_id'],
        'CUTEX_RUNTIME_GENERATION':str(agents['worker']['receipt']['expected_generation']),
        'CUTEX_AGENT_BUS_URL':f'http://127.0.0.1:{bp}/','CUTEX_AGENT_BUS_TOKEN':BUS_TOKEN,'S4_TEST_ALLOWED_PORTS':''}
    missing={'jsonrpc':'2.0','id':1,'method':'tools/call','params':{'name':'cutex_agent_list','arguments':{}}}
    rejected=subprocess.run([str(MCP)],input=json.dumps(missing)+'\n',text=True,capture_output=True,env=facade_env,cwd=RUN,timeout=10)
    assert rejected.returncode==0 and json.loads(rejected.stdout)['error']['code']==-32602
    denials.append('missing_core_before_transport')

    unrelated=owner(['/usr/bin/python3','-c','import signal; signal.pause()'],'unrelated-owned-process')
    worker_pid=agents['worker']['receipt']['binding']['pid']
    offline=manage('offline','s5b-offline',cutex_session_id=agents['worker']['durable']);complete(offline)
    identity=process_identity(worker_pid)
    assert identity is None or identity[1]=='Z',identity
    assert unrelated.poll() is None,'unrelated private process was stopped'
    assert manage('offline','s5b-offline',cutex_session_id=agents['worker']['durable'])==offline
    denied(manage('close','s5b-offline',cutex_session_id=agents['worker']['durable']))
    stopped=store()['sessions'][agents['worker']['durable']]
    assert stopped['explicit_launch']==before['worker']['explicit_launch'] and not stopped.get('app_server_runtime'),stopped.keys()
    closed=manage('close','s5b-close',cutex_session_id=agents['disposable']['durable']);complete(closed)
    assert manage('close','s5b-close',cutex_session_id=agents['disposable']['durable'])==closed
    fresh=manage('query_managed','s5b-final-query');complete(fresh)
    snapshot=json.loads((CONF/'runtime/agent-management/v1/agent-management-v1.json').read_text())
    for fact in facts:
        if fact['tool']=='cutex_agent_management' and fact['result'].get('outcome',{}).get('status')=='complete':
            assert snapshot['actions'][fact['arguments']['action_id']]['response']==fact['result']
    assert snapshot['agents'][agents['disposable']['durable']]['retired_at'] is not None
    summary={'actions':{k:{'phase':v['phase'],'operation':v['operation'],'request_sha256':v['request_sha256']} for k,v in snapshot['actions'].items()},
             'retired_disposable':agents['disposable']['durable'],'offline_worker':agents['worker']['durable']}
    (RUN/'management-state-summary.json').write_text(json.dumps(summary,indent=2))
    assert sha(NATIVE/'config.toml')==shared_sha
    raw_schema=json.dumps(Model.discovered)
    assert 'cutex_agent_management' in raw_schema and 'cutex_agent_list' in raw_schema
    for forbidden in ['caller_cutex_session_id','runtime_generation','principal','expected_assignment_revision',BUS_TOKEN,HUMAN_TOKEN]:
        assert forbidden not in raw_schema,forbidden
    (RUN/'discovered-tools.json').write_text(json.dumps(Model.discovered,indent=2))
    (RUN/'PASS.json').write_text(json.dumps({'operations':len(facts),'model_calls':Model.calls,
        'denials':denials,'facade_sha256':bundle['facade']['sha256'],
        'stop_oracle':'owned stock PID absent/zombie; unrelated owned process alive; existing systemctl-unavailable direct fallback, no cgroup proof',
        'stock_lifecycle':'query/list/offline/close real; create/replace/rotate input rejection, not stock bootstrap',
        'inbound':'not implemented; private harness drives calls'},indent=2))


source=Path(__file__).with_name('stock_task_mcp_boundary.py').read_text()
tree=ast.parse(source)
campaign=next(n for n in tree.body if isinstance(n,ast.Try))
cut=next(i for i,n in enumerate(campaign.body) if isinstance(n,ast.Assign)
         and any(isinstance(t,ast.Name) and t.id=='project' for t in n.targets))
campaign.body=campaign.body[:cut]+[ast.Expr(ast.Call(ast.Name('management_cases',ast.Load()),[],[]))]
campaign.body.insert(0,ast.Expr(ast.Call(ast.Name('private_executable_path',ast.Load()),[],[])))
# Add one genuine adopted offline disposable native thread, but never launch it.
class FixtureRoles(ast.NodeTransformer):
    def visit_List(self,node):
        if [getattr(e,'value',None) for e in node.elts]==['director','worker']:
            node.elts.append(ast.Constant('disposable'))
        return self.generic_visit(node)
    def visit_For(self,node):
        self.generic_visit(node)
        if any(isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='contract' for t in n.targets) for n in node.body):
            node.body.insert(0,ast.parse("if role == 'disposable': continue").body[0])
        return node
tree=FixtureRoles().visit(tree)
ast.fix_missing_locations(tree)
exec(compile(tree,'S5a-private-setup-reused-for-S5b','exec'),globals())
