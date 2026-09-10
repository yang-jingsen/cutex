"""Human-run only: host stages authorized auth; guest opens the same-owner CLI.
No automatic prompt, turn, retry, grant or business action is generated.
"""
import ast,os,sys,subprocess,threading,queue,json,signal
from pathlib import Path

SSH=['ssh','-F','/mnt/mambo/vmstore/cutex-linux-acceptance/ssh/config','cutex-linux-acceptance']
REMOTE='/home/cutex-linux-test/acceptance-upload/hj1'

def host():
    if not sys.stdin.isatty():raise SystemExit('Run interactively from a terminal.')
    print('This starts a NEW private terra-low/readOnly diagnostic; model calls happen only when you type.\nNo r3/r4 hold will be resumed. Guest auth is removed on exit.')
    if input('Type START to copy the authorized aemeath auth and open CLI: ')!='START':return
    # New run only; never overwrite existing evidence/staged credentials.
    import datetime
    name='h'+datetime.datetime.now(datetime.timezone.utc).strftime('%H%M%S')
    subprocess.run(SSH+[f'test ! -e {REMOTE}/{name} && test ! -e {REMOTE}/staged-auth.json'],check=True)
    try:
        subprocess.run(['scp','-q','-F',SSH[2],'/home/senxiu/.cutex/profiles/cd6a39eb-3997-45c6-9824-5113fe36a4b8/auth.json',f'cutex-linux-acceptance:{REMOTE}/staged-auth.json'],check=True)
        subprocess.run(SSH[:1]+['-tt']+SSH[1:]+[f'chmod 600 {REMOTE}/staged-auth.json && python3 -B {REMOTE}/fixtures/human_job_terminal.py --guest {name}'],check=True)
    finally:
        # Only exact new copied files; no source credential writes/syncback.
        cleanup=subprocess.run(SSH+[f'rm -f -- {REMOTE}/staged-auth.json {REMOTE}/{name}/h/.cutex/codex-home/auth.json'],check=False)
        if cleanup.returncode:print('WARNING: guest credential cleanup was not confirmed. Reconnect and remove ONLY '+REMOTE+'/staged-auth.json and '+REMOTE+'/'+name+'/h/.cutex/codex-home/auth.json',file=sys.stderr)

def human_cli(g):
    import human_error_text
    rpc,_=g['native_rpc'](g['current'])
    rpc.call('thread/resume',{'threadId':g['thread']}) # subscribe, not a turn
    stopped=threading.Event()
    path=g['RUN']/'human-errors.jsonl'
    def observe():
        for event in rpc.events:human_error_text.append(path,event)
        rpc.events.clear()
        while not stopped.is_set():
            try:event=rpc.messages.get(timeout=.2)
            except queue.Empty:continue
            human_error_text.append(path,event)
            # Read-only listener: no approval/turn/retry/ACK responses here.
    watcher=threading.Thread(target=observe,daemon=True);watcher.start()
    args={'actionId':'human-fixed-job','argv':['/bin/sh','-c','cat probe-readable; printf real-job-output'],'cwd':str(g['RUN'])}
    print('\nPaste this once in the CLI (normal approval is yours):\n')
    print('Use tool_search/CodeMode only to invoke tools.mcp__cutex_job__submit once with '+json.dumps(args)+'. Do not use shell/exec_command directly, poll, or retry. End with Submitted. On the normal completion notification use read_output once for stdout and report it. No other files/tools.\n')
    print('Exit CLI with Ctrl+C as needed. Do not release/retry held inputs. Local redacted error file:',path,flush=True)
    try:
        # Actual supported adapter resolves exact durable ID -> bound native
        # thread/owner and launches pinned CLI --remote, not a second writer.
        subprocess.run([str(g['CUTEX']),'session','stock-attach',g['durable']],env=g['env'],cwd=g['RUN'],check=True)
    finally:
        stopped.set();watcher.join(timeout=2)
    print('\nCLI returned. Owned services/auth will now be cleaned. Inspect locally with:\ncat '+str(path),flush=True)
    return {'human_interactive':True,'error_file':str(path),'no_automated_turn':True}

def guest(name):
    if not name.isalnum() or len(name)>12:raise SystemExit('invalid private run name')
    os.umask(0o077)
    def interrupted(signum,frame):raise KeyboardInterrupt()
    signal.signal(signal.SIGHUP,interrupted)
    signal.signal(signal.SIGTERM,interrupted)
    sys.argv=[sys.argv[0],name]
    # Reuse frozen setup/cleanup; replace only the automatic model campaign.
    source=Path(__file__).with_name('reviewed_aemeath_r4.py')
    tree=ast.parse(source.read_text())
    tree.body=[n for n in tree.body if not(isinstance(n,ast.FunctionDef) and n.name=='run_job')]
    scope=dict(globals(),run_job=human_cli,__file__=str(source))
    exec(compile(tree,str(source),'exec'),scope)

if __name__=='__main__':
    if sys.argv[1:2]==['--guest']:guest(sys.argv[2])
    elif len(sys.argv)==1:host()
    else:raise SystemExit('usage: python3 human_job_terminal.py')
