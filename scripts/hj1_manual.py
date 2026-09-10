"""Explicit single operations for the fixed Human-owned hj1 fixture; no setup loop.
Never starts services, copies auth, creates threads, submits turns or retries.
"""
import ast
import base64
import hashlib
import http.client
import json
import os
from pathlib import Path
import queue
import socket
import struct
import sys
import threading
import time
import uuid

from human_error_text import append, redact

ROOT = Path('/home/cutex-linux-test/acceptance-upload/hj1')
RUN = ROOT / 'h203236'
CONF = RUN / 'h/.cutex'
ID = 'cutex.01a08d05-bbb5-7f32-a849-02cceb9b2564'
THREAD = ID.removeprefix('cutex.')


def store():
    return json.loads((CONF / 'cutex-sessions.json').read_text())


def save(name, value):
    fd = os.open(RUN / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as out:
        json.dump(value, out, indent=2)


def request(body, path='/v2/agent-management/explicit-launch'):
    # Fixed owned endpoint only; no proxy, redirect, default endpoint or retry.
    token = json.loads((CONF / 'config.json').read_text())['management_api_token']
    connection = http.client.HTTPConnection('127.0.0.1', 24801, timeout=240)
    print('Sending ONE request to private Management :24801; timeout 240s; no retry.', flush=True)
    done = threading.Event()
    def progress():
        while not done.wait(15):
            print('Still waiting for this request; do not launch another.', flush=True)
    threading.Thread(target=progress, daemon=True).start()
    try:
        connection.request('POST', path, json.dumps(body), {
            'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
        response = connection.getresponse()
        raw = response.read(2 * 1024 * 1024 + 1)
        if len(raw) > 2 * 1024 * 1024:
            raise RuntimeError('oversized response; outcome unknown; stop')
        value = json.loads(raw)
        if response.status != 200:
            error = value.get('error', {}) if isinstance(value, dict) else {}
            message = error.get('message') if isinstance(error, dict) else None
            raise RuntimeError('HTTP ' + str(response.status) + ': ' + (redact(message) or 'No safe message field; stop and report status'))
        return value
    finally:
        done.set()
        connection.close()


def main(operation):
    os.umask(0o077)
    assert RUN.resolve() == RUN and RUN.stat().st_uid == os.getuid()
    current = store()
    record = current['sessions'][ID]
    if operation == 'status':
        print(json.dumps({k: record.get(k) for k in (
            'cutex_session_id', 'runtime_generation', 'app_server_launch_claim_id',
            'current_runtime_agent_id')}, indent=2))
        for key, wrapped in current['explicit_launch_receipts'].items():
            receipt = wrapped['receipt']
            print(key, receipt.get('stage', 'activation'))
        return
    if operation == 'review':
        assert not (RUN / 'manual-review.json').exists(), 'review exists; inspect, do not overwrite'
        assert not record.get('app_server_runtime') and not record.get('app_server_launch_claim_id'), 'existing owner/claim: stop and inspect'
        descriptor = current['explicit_launch_receipts']['configured-job-launch']['receipt']['review']['job_mcp']['descriptor']
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as peer:
            peer.settimeout(5)
            peer.connect(descriptor['endpoint'])
            pid, uid, _ = struct.unpack('3i', peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
            assert uid == os.getuid()
            descriptor['daemon_pid'] = pid
            descriptor['daemon_start_ticks'] = int(Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[19])
        # Actual provider revalidates artifact, custody, peer and current config.
        review = request({'operation': 'review_runtime', 'cutex_session_id': ID,
                          'restart': False, 'job_mcp': descriptor})
        save('manual-review.json', review)
        print('Review saved locally: manual-review.json. Inspect model/low/readOnly/on-request/Job paths before run; do not publish full file.')
    elif operation == 'run':
        assert not (RUN / 'manual-run-started').exists(), 'request already attempted; inspect outcome, NO automatic replay'
        review = json.loads((RUN / 'manual-review.json').read_text())
        if input('Type RUN to confirm this exact reviewed existing-thread launch: ') != 'RUN':
            return
        save('manual-run-started', {'action_id': 'human-hj1-manual-launch'})
        receipt = request({'operation': 'run', 'action_id': 'human-hj1-manual-launch', 'review': review})
        save('manual-receipt.json', receipt)
        error = receipt.get('error')
        message = error.get('message') if isinstance(error, dict) else error if isinstance(error, str) else None
        print('stage:', receipt.get('stage'), 'error:', redact(message))
        if receipt.get('stage') != 'ready':
            raise RuntimeError('not ready; stop, no create/retry/attach')
    elif operation == 'offline':
        if input('Type STOP to stop only this fixture runtime through private Management: ') != 'STOP':
            return
        result = request({'requestId': str(uuid.uuid4()), 'method': 'cutex/runtime/offline',
                          'params': {'expectedRuntimeGeneration': record['runtime_generation'],
                                     'reason': 'human_hj1_cleanup', 'force': False}},
                         '/v2/sessions/' + ID + '/cutex/requests')
        print('offline response received; requestId:', result.get('requestId'),
              'status:', result.get('cutex', {}).get('result', {}).get('status', 'not exposed'))
        print('Check status before stopping service terminals. An accepted response is not proof of process exit.')
    elif operation == 'observe':
        receipt = json.loads((RUN / 'manual-receipt.json').read_text())
        assert receipt['stage'] == 'ready' and receipt['binding'] == record['app_server_runtime']
        source = ROOT / 'fixtures/base-fixture.py'
        assert hashlib.sha256(source.read_bytes()).hexdigest() == 'ed1ebdb57d413db2f3f382aa65c1bf014e4ccb8b8483b63dd85e774717d9728e'
        nodes = [n for n in ast.parse(source.read_text()).body if isinstance(n, ast.ClassDef) and n.name == 'RPC']
        assert len(nodes) == 1
        scope = dict(globals())
        exec(compile(ast.Module(body=nodes, type_ignores=[]), str(source), 'exec'), scope)
        rpc = scope['RPC'](receipt['binding']['endpoint'].removeprefix('unix://'))
        rpc.call('initialize', {'clientInfo': {'name': 'human-error-observer', 'version': '1'}, 'capabilities': {'experimentalApi': True}})
        rpc.notify('initialized')
        rpc.call('thread/resume', {'threadId': THREAD})
        print('Observing errors only; no turn/approval/retry. Ctrl+C stops observer.', flush=True)
        for event in rpc.events:
            append(RUN / 'human-errors.jsonl', event)
        while True:
            append(RUN / 'human-errors.jsonl', rpc.messages.get())
    else:
        raise RuntimeError('choose status, review, run, observe or offline')


if __name__ == '__main__':
    try:
        assert len(sys.argv) == 2
        main(sys.argv[1])
    except KeyboardInterrupt:
        print('Stopped this client only; no service cleanup or retry performed.')
    except AssertionError:
        print('Fixture/identity/RPC assertion failed; stop. Raw protocol payload is not printed.', file=sys.stderr)
        sys.exit(1)
    except Exception as error:
        print(redact(str(error)), file=sys.stderr)
        sys.exit(1)
