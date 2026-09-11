"""ONE private restart diagnostic, no suite/retry/PTY or paid model.

One fake Job notification is necessary to instantiate the existing inbound
connection and reproduce the retained post-append contention. Reuse only setup;
do not replay the normal-path/crash/Job acceptance campaigns.
"""
import ast
from pathlib import Path

_diagnostic_source=ast.parse(Path(__file__).with_name('presentation_boundary.py').read_text())
_diagnostic_try_at=next(i for i,n in enumerate(_diagnostic_source.body) if isinstance(n,ast.Try))
exec(compile(ast.Module(body=_diagnostic_source.body[:_diagnostic_try_at],type_ignores=[]),'owned-diagnostic-setup','exec'),globals())
assert mode=='generation'
CUTEX=ROOT/'target/debug/cutex';MCP=ROOT/'target/debug/cutex-mcp'
env['CUTEX_PRESENTATION_DIAGNOSTIC_PHASES']='1'
readers=[]
def owner(args,name):
    log=open(RUN/(name+'.log'),'ab',buffering=0);logs.append(log)
    p=subprocess.Popen([str(a) for a in args],env=env,cwd=RUN,stdout=log,stderr=subprocess.PIPE,start_new_session=True)
    children.append(p)
    def capture():
        for line in p.stderr:
            log.write(line)
            if line.startswith(b'PDIAG '):
                sys.stderr.buffer.write(name.encode()+b' '+line);sys.stderr.buffer.flush()
    t=threading.Thread(target=capture,daemon=True);t.start();readers.append(t)
    return p
def expired(signum,frame):raise TimeoutError('explicit diagnostic deadline')
signal.signal(signal.SIGALRM,expired);signal.alarm(900)
outcome={'scope':'one local-fake post-append restart, not repeated Job/PTY acceptance'}
try:
    setup=_diagnostic_source.body[_diagnostic_try_at].body
    end=next(i for i,n in enumerate(setup) if isinstance(n,ast.Assign)
        and any(isinstance(t,ast.Name) and t.id=='current' for t in n.targets))
    exec(compile(ast.Module(body=setup[:end+1],type_ignores=[]),'owned-actual-launch','exec'),globals())
    assert Model.calls==0
    print('DIAG initial ready; ONE fixture event, no other input',file=sys.stderr,flush=True)
    request={'schema':'cutex.job_service.completion.v1','eventId':'private-event','jobId':'diagnostic-job','jobRevision':1,
        'terminalStatus':'exited','resultSha256':'a'*64,'targetCutexSessionId':durable,'summary':'diagnostic boundary'}
    token=(CONF/'runtime/task-service/job-service-completion.token').read_text().strip()
    code,result=api(bp,'/api/job-service/v1/completions',request,token=token);assert code==200
    mid=result['messageId'];connection,_=gate.accept()
    assert connection.makefile('rb').readline().decode().strip()==mid
    before=snapshot(mid);assert before['state']=='delivered' and before['presentation']['receipt'] is None
    print('DIAG post-append latch reached; ONE reviewed restart, 240s request deadline',file=sys.stderr,flush=True)
    for key in list(env):
        if key.startswith('CUTEX_NATIVE_DELIVERY_TEST_'):del env[key]
    results=queue.Queue()
    def restart():
        try:results.put(launch('diagnostic-restart',True))
        except BaseException as e:results.put(e)
    threading.Thread(target=restart,daemon=True).start()
    wait_for(lambda:'diagnostic-restart' in store().get('explicit_launch_receipts',{}),180)
    print('DIAG Prepared observed; release display latch',file=sys.stderr,flush=True)
    connection.sendall(b'continue\n');connection.close();gate.close();gate=None
    answer=results.get(timeout=245)
    outcome['restart_ready']=isinstance(answer,dict)
    outcome['request_error_type']=None if isinstance(answer,dict) else type(answer).__name__
except BaseException as error:
    outcome['diagnostic_error_type']=type(error).__name__
    print('DIAG stopped:',type(error).__name__,file=sys.stderr,flush=True)
finally:
    signal.alarm(0)
    if durable:
        s=store();entry=s['sessions'][durable]
        receipt=s.get('explicit_launch_receipts',{}).get('diagnostic-restart',{}).get('receipt',{})
        outcome.update(durable=durable,native=entry['codex_session_id'],generation=entry['runtime_generation'],stage=receipt.get('stage'),
            model_requests=Model.calls,cutex=sha(CUTEX),facade=sha(MCP))
        runtime=entry.get('app_server_runtime')
        if runtime:stock_pids.append(runtime['pid'])
    (RUN/'DIAGNOSTIC.json').write_text(json.dumps(outcome,indent=2))
    if gate:gate.close()
    for pid in set(stock_pids):
        try:
            if os.getpgid(pid)==pid and Path(f'/proc/{pid}/exe').readlink()==PATCHED and Path(f'/proc/{pid}/cwd').readlink()==RUN:os.killpg(pid,signal.SIGKILL)
        except (ProcessLookupError,FileNotFoundError):pass
    for p in reversed(children):stop_owned(p)
    for t in readers:t.join(timeout=2)
    for log in logs:log.close()
    model.shutdown()
    print('DIAG owned fixture ended; no retry',file=sys.stderr,flush=True)
