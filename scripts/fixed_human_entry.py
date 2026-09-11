"""One-operation Human entry: attach only, read-only observer, or exact cleanup."""
import ast, base64, hashlib, json, os, queue, signal, socket, struct, sys, threading, time
from pathlib import Path
from human_error_text import append

ROOT=Path(__file__).resolve().parents[1]
RUN=ROOT/'h1'


def identity_matches(item):
    pid=item['pid']
    try:
        fields=Path(f'/proc/{pid}/stat').read_text().rsplit(')',1)[1].split()
        return fields[0]!='Z' and fields[19]==item['start_ticks'] and str(Path(f'/proc/{pid}/exe').readlink())==item['exe'] and os.getpgid(pid)==item['pgid']
    except (FileNotFoundError,ProcessLookupError):return False


def main(op):
    os.umask(0o077)
    info=json.loads((RUN/'handoff.json').read_text())
    if op=='attach':
        assert identity_matches(info['runtime']), 'prepared owner is no longer current; do not auto restart'
        env={'PATH':'/usr/local/bin:/usr/bin:/bin','LANG':'C.UTF-8','TERM':os.environ.get('TERM','xterm-256color'),
             'HOME':str(RUN/'h'),'CODEX_HOME':str(RUN/'h/.cutex/codex-home'),
             'CUTEX_TEST_PRIVATE_HOME':str(RUN/'h'),'TMPDIR':str(ROOT/'tmp')}
        os.chdir(RUN)
        os.execve(ROOT/'bin/cutex',[str(ROOT/'bin/cutex'),'session','stock-attach',info['durable']],env)
    elif op=='observe':
        assert identity_matches(info['runtime'])
        base=ROOT/'fixtures/base-fixture.py'
        assert hashlib.sha256(base.read_bytes()).hexdigest()=='ed1ebdb57d413db2f3f382aa65c1bf014e4ccb8b8483b63dd85e774717d9728e'
        nodes=[n for n in ast.parse(base.read_text()).body if isinstance(n,ast.ClassDef) and n.name=='RPC']
        scope=dict(globals());exec(compile(ast.Module(body=nodes,type_ignores=[]),str(base),'exec'),scope)
        record=json.loads((RUN/'h/.cutex/cutex-sessions.json').read_text())['sessions'][info['durable']]
        assert record['runtime_generation']==info['generation'] and record['codex_session_id']==info['native']
        assert record['app_server_runtime']['pid']==info['runtime']['pid']
        rpc=scope['RPC'](record['app_server_runtime']['endpoint'].removeprefix('unix://'))
        rpc.call('initialize',{'clientInfo':{'name':'human-fixed-error-observer','version':'1'},'capabilities':{'experimentalApi':True}})
        rpc.notify('initialized');rpc.call('thread/resume',{'threadId':info['native']})
        (RUN/'observer-ready').touch(mode=0o600)
        for event in rpc.events:append(RUN/'human-errors.jsonl',event)
        while True:append(RUN/'human-errors.jsonl',rpc.messages.get())
    elif op=='cleanup':
        if input(f'Type CLEAN after exiting the CLI; stop only {ROOT.name}/h1 and remove its guest auth: ')!='CLEAN':return
        for item in [info['runtime'],info.get('observer'),*reversed(info['services'])]:
            if not item:continue
            if not identity_matches(item):
                # Never signal a reused PID or unknown process.
                if Path(f"/proc/{item['pid']}/exe").exists():raise RuntimeError('identity changed; stop cleanup, no signal sent to this PID')
                continue
            assert item['pid']==item['pgid'], 'not an owned process-group leader'
            os.killpg(item['pid'],signal.SIGTERM)
            deadline=time.monotonic()+20
            while identity_matches(item) and time.monotonic()<deadline:threading.Event().wait(.1)
            if identity_matches(item):raise RuntimeError('owned process did not exit; no forced kill or credential removal')
        for path in [RUN/'h/.cutex/codex-home/auth.json',ROOT/'staged-auth.json']:
            if path.exists():
                assert not path.is_symlink() and path.stat().st_uid==os.getuid()
                path.unlink()
        print(f'Only {ROOT.name}/h1 owned processes stopped and exact temporary guest auth removed; history retained; host and other fixtures untouched.')
    else:raise RuntimeError('choose attach, observe or cleanup')


if __name__=='__main__':
    try:
        assert len(sys.argv)==2
        main(sys.argv[1])
    except KeyboardInterrupt:pass
    except Exception as error:
        # Never dump protocol replies/config/auth on preparation failures.
        print('Operation stopped:',type(error).__name__,'; inspect private stage with owner, do not retry automatically.',file=sys.stderr)
        sys.exit(1)
