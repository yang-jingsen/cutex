"""S5c fixed-stock, model-free persistence barrier proof (Linux x86_64).

No Cutex process/provider, API credentials, Responses server, turn/start, sleeps,
rollout polling, or duplicate create. Each named scenario creates exactly once.
Stock and its children inherit a seccomp ban on creating IPv4/IPv6 sockets.
"""
import ctypes
import json
import os
import platform
import queue
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
STOCK = Path('/mnt/mambo/PersonaProjects/cutex-upstream-lightweight-r1/stock/bin/codex')


class Filter(ctypes.Structure):
    _fields_ = [('code', ctypes.c_ushort), ('jt', ctypes.c_ubyte),
                ('jf', ctypes.c_ubyte), ('k', ctypes.c_uint)]


class Program(ctypes.Structure):
    _fields_ = [('len', ctypes.c_ushort), ('filter', ctypes.POINTER(Filter))]


def no_ip_sockets():
    # Linux seccomp_data: arch offset 4, nr offset 0, args[0] offset 16.
    # Wrong architecture kills; socket(AF_INET/AF_INET6) returns EACCES.
    # Mask the x32 syscall bit too. No inherited network descriptors are passed.
    rules = (Filter * 11)(
        Filter(0x20, 0, 0, 4), Filter(0x15, 1, 0, 0xc000003e),
        Filter(0x06, 0, 0, 0x80000000), Filter(0x20, 0, 0, 0),
        Filter(0x54, 0, 0, 0x3fffffff), Filter(0x15, 0, 4, 41),
        Filter(0x20, 0, 0, 16), Filter(0x15, 1, 0, 2),
        Filter(0x15, 0, 1, 10), Filter(0x06, 0, 0, 0x50000 | 13),
        Filter(0x06, 0, 0, 0x7fff0000))
    program = Program(len(rules), rules)
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(38, 1, 0, 0, 0) or libc.prctl(22, 2, ctypes.byref(program), 0, 0):
        os._exit(97)


class Owner:
    def __init__(self, root, name, env):
        self.log = open(root / (name + '.log'), 'wb')
        self.child = subprocess.Popen([str(STOCK), 'app-server'], cwd=root, env=env,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
            start_new_session=True, close_fds=True, preexec_fn=no_ip_sockets)
        self.messages = queue.Queue()
        self.serial = 0
        self.events = []
        def read():
            try:
                for line in self.child.stdout:
                    assert len(line) <= 1024 * 1024
                    self.messages.put(json.loads(line))
            finally:
                self.messages.put({'eof': True})
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()
        try:
            self.call('initialize', {'clientInfo': {'name': 's5c-neutral', 'version': '1'},
                                   'capabilities': {'experimentalApi': True}})
            self.send({'method': 'initialized'})
        except BaseException:
            self.close(abrupt=True)
            raise

    def send(self, value):
        self.child.stdin.write((json.dumps(value) + '\n').encode())
        self.child.stdin.flush()

    def call(self, method, params, allow_error=False):
        assert method in ['initialize', 'thread/start', 'thread/read', 'thread/resume']
        self.serial += 1
        ident = self.serial
        self.send({'id': ident, 'method': method, 'params': params})
        deadline = time.monotonic() + 30
        while True:
            item = self.messages.get(timeout=max(.001, deadline - time.monotonic()))
            assert 'eof' not in item, ('owner exited', self.child.poll(), method)
            if item.get('id') == ident:
                if 'error' in item:
                    assert allow_error, item['error']
                    return item
                return item['result']
            self.events.append(item)

    def close(self, abrupt=False):
        if self.child.poll() is None:
            if abrupt:
                os.killpg(self.child.pid, signal.SIGKILL)
            else:
                self.child.stdin.close()
        code = self.child.wait(timeout=30)
        self.reader.join(timeout=5)
        self.log.close()
        if not abrupt:
            assert code == 0, ('normal EOF shutdown failed', code)
        return code


def recover_existing(run):
    # Resolve the already-returned ID once, after the failed probe reaped its
    # creator. This is NOT another create or a persistence success assertion.
    assert run.parent == ROOT and (run/'outcomes.json').is_file()
    destination=run/'RECOVERY.json'
    assert not destination.exists()
    facts=json.loads((run/'outcomes.json').read_text())
    assert len(facts)==1 and facts[0]['create_calls']==1
    root=run/facts[0]['scenario']; ident=facts[0]['native_id']
    env={'PATH':'/usr/bin:/bin','HOME':str(root/'home'),'CODEX_HOME':str(root/'native'),
         'TMPDIR':str(root/'tmp'),'LANG':'C.UTF-8','TERM':'dumb'}
    owner=Owner(root,'exact-id-recovery',env)
    result={'native_id':ident,'new_create_calls':0,'original_creation_acknowledged':False}
    try:
        result['resume']=owner.call('thread/resume',{'threadId':ident,'excludeTurns':True},allow_error=True)
        if 'error' not in result['resume']:
            assert result['resume']['thread']['id']==ident
            result['read']=owner.call('thread/read',{'threadId':ident,'includeTurns':True},allow_error=True)
        result['exit']=owner.close()
    finally:
        if owner.child.poll() is None: owner.close(abrupt=True)
        destination.write_text(json.dumps(result,indent=2))


def main():
    assert platform.system() == 'Linux' and platform.machine() == 'x86_64'
    run = ROOT / sys.argv[1]
    if len(sys.argv)==3 and sys.argv[2]=='--recover-existing':
        recover_existing(run)
        return
    assert run.parent == ROOT and not run.exists(), 'new owned evidence directory required'
    assert os.statvfs(ROOT).f_bavail * os.statvfs(ROOT).f_frsize > 100 * 1024**3
    run.mkdir(mode=0o700)
    outcomes = []
    owners = []
    try:
        for mode in ['ack-normal-exit', 'ack-creator-death', 'death-before-barrier']:
            root = run / mode
            root.mkdir()
            home = root / 'home'; home.mkdir()
            native = root / 'native'; native.mkdir()
            temporary = root / 'tmp'; temporary.mkdir()
            env = {'PATH': '/usr/bin:/bin', 'HOME': str(home), 'CODEX_HOME': str(native),
                   'TMPDIR': str(temporary), 'LANG': 'C.UTF-8', 'TERM': 'dumb'}
            # No real/dummy auth needed. There is no reachable provider: sockets
            # are prohibited before exec; no model turn is ever submitted.
            (native / 'config.toml').write_text('''model="gpt-5.4"
model_provider="s5c-disabled"
[model_providers.s5c-disabled]
name="S5c never-called provider"
base_url="http://127.0.0.1:1/disabled"
wire_api="responses"
requires_openai_auth=false
[analytics]
enabled=false
''')
            if not outcomes:
                check = subprocess.run(['/usr/bin/python3', '-c',
                    'import socket\nfor family in [socket.AF_INET,socket.AF_INET6]:\n'
                    ' try: socket.socket(family); raise AssertionError("IP socket allowed")\n'
                    ' except PermissionError: pass\n'
                    'socket.socket(socket.AF_UNIX).close()\nprint("IP socket creation denied; Unix allowed")'],
                    env=env, cwd=root, capture_output=True, text=True,
                    preexec_fn=no_ip_sockets, timeout=10)
                assert check.returncode == 0, check.stderr
                (run / 'network-oracle.txt').write_text(check.stdout)
            creator = Owner(root, 'creator', env); owners.append(creator)
            started = creator.call('thread/start', {'cwd': str(root), 'ephemeral': False,
                'historyMode': 'paginated', 'sandbox': 'read-only', 'approvalPolicy': 'on-request'})
            thread = started['thread']
            assert thread['historyMode'] == 'paginated' and not thread['ephemeral'], thread
            ident = thread['id']
            fact = {'scenario': mode, 'native_id': ident, 'history_mode': thread['historyMode'],
                    'source': thread.get('source'), 'create_calls': 1,
                    'creation_state': 'ID_returned_not_persistence_acknowledged'}
            outcomes.append(fact)
            (run / 'outcomes.json').write_text(json.dumps(outcomes, indent=2))
            if mode != 'death-before-barrier':
                barrier = creator.call('thread/read', {'threadId': ident, 'includeTurns': True}, allow_error=True)
                fact['barrier_response']=barrier
                (run / 'outcomes.json').write_text(json.dumps(outcomes, indent=2))
                assert 'error' not in barrier,barrier
                assert barrier['thread']['id'] == ident and barrier['thread']['turns'] == []
                fact['creation_state'] = 'read_includeTurns_success_persistence_acknowledged'
                fact['barrier'] = barrier
                (run / 'outcomes.json').write_text(json.dumps(outcomes, indent=2))
            fact['creator_exit'] = creator.close(abrupt=mode != 'ack-normal-exit')
            # The first owner is reaped BEFORE another writer is started.
            fresh = Owner(root, 'fresh-owner', env); owners.append(fresh)
            resumed = fresh.call('thread/resume', {'threadId': ident}, allow_error=True)
            fact['fresh_resume'] = resumed
            if mode != 'death-before-barrier':
                assert 'error' not in resumed and resumed['thread']['id'] == ident, resumed
                read = fresh.call('thread/read', {'threadId': ident, 'includeTurns': True})
                assert read['thread']['turns'] == [] and read['thread']['id'] == ident, read
                fact['fresh_neutral_read'] = read
            else:
                fact['creation_state'] = 'unacknowledged_exact_ID_recovery_only_no_recreate'
                # Recovery observation does not retroactively acknowledge the
                # original create. A missing rollout stays an explicit gap.
            fact['fresh_exit'] = fresh.close()
            (run / 'outcomes.json').write_text(json.dumps(outcomes, indent=2))
        (run / 'PASS.json').write_text(json.dumps({'scenarios': len(outcomes),
            'barrier': 'paginated thread/read includeTurns=true success',
            'network': 'seccomp denies IPv4/IPv6 socket creation', 'model_turns': 0,
            'Cutex_invocations': 0, 'scope': 'process death/restart, NOT fsync/power loss'}, indent=2))
    finally:
        (run / 'outcomes.json').write_text(json.dumps(outcomes, indent=2))
        for owner in reversed(owners):
            if owner.child.poll() is None:
                owner.close(abrupt=True)


if __name__ == '__main__':
    main()
