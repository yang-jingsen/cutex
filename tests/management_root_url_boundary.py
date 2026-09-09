"""Real private CLI -> root Management review route; no native/Agent launch.

Arguments: new owned evidence directory name, optional exact Cutex binary.
The deliberately absent durable subject must produce the provider review error,
not a router 404. This proves routing/authentication, not successful activation.
"""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parents[2]
run = ROOT / sys.argv[1]
assert run.parent == ROOT and not run.exists()
assert os.statvfs(ROOT).f_bavail * os.statvfs(ROOT).f_frsize >= 100 * 1024**3
run.mkdir(mode=0o700)
home = run / 'home'
home.mkdir()
(home / '.cutex-test-private-home').touch()
conf = home / '.cutex'
conf.mkdir()
token = 'private-root-url-fixture-only'
(conf / 'config.json').write_text(json.dumps({'management_api_token': token}))
env = {'PATH': '/usr/bin:/bin', 'HOME': str(home),
       'TMPDIR': str(ROOT / 'tmp'), 'CUTEX_TEST_PRIVATE_HOME': str(home)}
guard = run / 'connect-guard.so'
subprocess.run(['/usr/bin/cc', '-shared', '-fPIC', '-Wall', '-Werror',
                str(Path(__file__).with_name('stock_probe_connect_guard.c')),
                '-ldl', '-o', str(guard)], env=env, check=True, capture_output=True)
env.update(LD_PRELOAD=str(guard), S4_TEST_ALLOWED_PORTS='')
tripwire = subprocess.run(['/usr/bin/python3', '-c',
                          "import socket; socket.socket().connect(('127.0.0.1',1))"],
                         env=env, capture_output=True)
assert tripwire.returncode == 97
ports = []
for candidate in range(24800, 24999):
    with socket.socket() as reservation:
        try:
            reservation.bind(('127.0.0.1', candidate))
        except OSError:
            continue
        ports.append(candidate)
    if len(ports) == 2:
        break
assert len(ports) == 2
port, bus_port = ports
env['S4_TEST_ALLOWED_PORTS'] = f'{port},{bus_port}'
(conf / 'config.json').write_text(json.dumps({
    'management_api_token': token, 'agent_bus_enabled': True,
    'agent_bus_port': bus_port, 'agent_bus_token': 'private-url-bus-only'}))
binary = Path(sys.argv[2]) if len(sys.argv) > 2 else ROOT / 'target/debug/cutex'
request = run / 'review.json'
request.write_text(json.dumps({'operation': 'review_runtime',
                              'cutex_session_id': 'cutex.private-url-absent',
                              'restart': False}))
def ready(child, endpoint):
    deadline = time.monotonic() + 15
    while True:
        assert child.poll() is None, 'private service exited'
        try:
            with socket.create_connection(('127.0.0.1', endpoint), .1):
                return
        except OSError:
            assert time.monotonic() < deadline, 'private readiness timeout'
            threading.Event().wait(.02)

with (run / 'management.log').open('wb') as log, (run / 'bus.log').open('wb') as bus_log:
    bus = subprocess.Popen([str(binary), 'agent', 'serve', '--port', str(bus_port)],
                           env=env, cwd=run, stdout=bus_log, stderr=bus_log)
    child = None
    try:
        ready(bus, bus_port)
    except BaseException:
        bus.terminate()
        bus.wait(timeout=10)
        raise
    child = subprocess.Popen([str(binary), 'management', 'serve', '--port', str(port)],
                             env=env, cwd=run, stdout=log, stderr=log)
    try:
        ready(child, port)
        outcomes = []
        for suffix in ['/', '']:
            result = subprocess.run([str(binary), 'session', 'stock', '--request', str(request),
                                     '--management-url', f'http://127.0.0.1:{port}{suffix}'],
                                    env=env, cwd=run, capture_output=True, text=True, timeout=15)
            assert token not in result.stdout + result.stderr
            (run / ('cli-root-slash.log' if suffix else 'cli-root.log')).write_text(result.stderr)
            assert result.returncode != 0
            assert 'stock durable record missing' in result.stderr, result.stderr
            assert '404' not in result.stderr
            outcomes.append({'base_trailing_slash': bool(suffix),
                             'provider_error': 'stock durable record missing', 'not_404': True})
        (run / 'result.json').write_text(json.dumps({
            'boundary': 'real default CLI and root-authenticated Management provider; absent fixture subject',
            'route': '/v2/agent-management/explicit-launch', 'outcomes': outcomes,
            'tripwire': 'unowned TCP connect rejected before syscall',
            'omitted': 'successful activation/Create/pilot remain Release; no native/model or live state'}, indent=2))
        print(json.dumps(outcomes))
    finally:
        child.terminate()
        child.wait(timeout=10)
        bus.terminate()
        bus.wait(timeout=10)
