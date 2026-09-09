"""Actual frozen J daemon/outbox; private grant issuer fixture, not Core MCP proof.
Exact J grant.rs canonical JSON/HMAC contract. No Job source/build changes.
"""
def run_job(g):
    import hashlib,hmac,json,os,socket,time,threading
    from pathlib import Path
    root=g['RUN'];home=g['HOME'];conf=g['CONF'];owner=g['owner'];stock=g['STOCK']
    frozen=Path('/mnt/mambo/PersonaProjects/cutex-tui-job-release-prep-r1/release-r2-env-forwarding')
    manifest=json.loads((frozen/'RELEASE_MANIFEST.json').read_text())
    artifact=manifest['artifacts']['cutex_job_service'];binary=frozen/artifact['path']
    assert manifest['sources']['job_service']['commit']=='36f8b577b5a067cbf5da4ddc10757ea60d306898'
    assert binary.stat().st_size==1580400 and hashlib.sha256(binary.read_bytes()).hexdigest()==artifact['sha256']=='747290895f65f5e336e9ba5f0568af957e986b53d562b10fae3ec3ac9136bf5c'
    api=os.urandom(32);key=os.urandom(32)
    for name,value in [('job-api',api),('job-grant',key)]:
        path=home/name;path.write_bytes(value);path.chmod(0o600)
    state=root/'job-state';sock=home/'job.sock'
    completion=conf/'runtime/task-service/job-service-completion.token'
    daemon=owner([binary,'serve',state,sock,home/'job-api',home/'job-grant',stock,'--completion',f"http://127.0.0.1:{g['bp']}",completion],'job')
    deadline=time.monotonic()+30
    while not sock.exists():
        assert daemon.poll() is None and time.monotonic()<deadline
        threading.Event().wait(.02)
    def canonical(value): return json.dumps(value,sort_keys=True,separators=(',',':'),ensure_ascii=False).encode()
    request={'actionId':'s6c2-job','argv':['/bin/sh','-c','printf s6c2-job-output'],'cwd':str(root),'environment':{},'subscriberCutexSessionId':g['durable'],
        'origin':{'runtimeAgentId':g['current']['runtime_agent_id'],'nativeThreadId':g['thread'],'permissionProfileType':'disabled'}}
    now=int(time.time())
    payload={'schema':'cutex/job-execution-grant/v1','grantId':'jgr_s6c2_private','requestSha256':hashlib.sha256(canonical(request)).hexdigest(),'subjectCutexSessionId':g['durable'],
        'operatingSystemUid':os.geteuid(),'cwd':str(root),'sandboxState':{'permissionProfile':{'type':'disabled'},'codexLinuxSandboxExe':None,'sandboxCwd':root.as_uri(),'useLegacyLandlock':False},
        'launcherPath':str(stock),'launcherSha256':'56ef98ab4032d317ab26e9b5e5a175650717351edb16ed9cde0cb6d1734d62da','issuedAtEpochSecs':now,'expiresAtEpochSecs':now+120}
    grant={'payload':payload,'hmacSha256':hmac.new(key,canonical(payload),hashlib.sha256).hexdigest()}
    def call(params):
        with socket.socket(socket.AF_UNIX) as stream:
            stream.settimeout(30);stream.connect(str(sock));stream.sendall(canonical({'token':api.hex(),'method':'submit','params':params})+b'\n')
            return json.loads(stream.makefile('rb').readline())
    denied=call({'request':{**request,'subscriberCutexSessionId':'cutex.00000000-0000-4000-8000-000000000000'},'grant':grant})
    assert denied['ok'] is False
    response=call({'request':request,'grant':grant});assert response['ok'],response
    job_id=response['result']['job']['jobId']
    replay=call({'request':request,'grant':grant});assert replay['ok'] and replay['result']['job']['jobId']==job_id
    deadline=time.monotonic()+90
    while True:
        data=json.loads((state/'state.json').read_text());job=data['jobs'][job_id]
        if job['completionDelivery']['state']=='delivered': break
        assert time.monotonic()<deadline,{'job':job_id,'state':job['completionDelivery']['state']}
        threading.Event().wait(.05)
    return {'job_id':job_id,'completion':job['completionDelivery'],'artifact_sha256':artifact['sha256'],'grant_boundary':'private signed issuer fixture; actual J verification/process/outbox, not Core metadata issuance'}
