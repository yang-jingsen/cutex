"""Explicit isolated launcher for the ignored Rust fixture; no real credentials."""
import os
import pathlib
import subprocess
import sys
import tempfile

root = pathlib.Path('/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1')
fixture = pathlib.Path(tempfile.mkdtemp(prefix='selected-r2-', dir=root))
os.chmod(fixture, 0o700)
binary = pathlib.Path(sys.argv[1]).resolve(strict=True)
assert binary.is_relative_to(root / 'target')
native = pathlib.Path('/mnt/mambo/PersonaProjects/cutex-light-core-r1/artifacts/independent-auth-file-r1/bundle/codex-app-server')
cmd = ['/usr/bin/bwrap', '--die-with-parent', '--unshare-net', '--unshare-pid', '--tmpfs', '/']
for p in ['/usr', '/bin', '/lib', '/lib64', '/etc', '/mnt']:
    cmd += ['--ro-bind', p, p]
cmd += ['--bind', str(fixture), '/tmp', '--proc', '/proc', '--dev', '/dev', '--chdir', '/tmp', str(binary), '--ignored', '--nocapture', '--test-threads=1']
env = {'PATH': '/usr/bin:/bin', 'HOME': '/tmp', 'TMPDIR': '/tmp', 'SELECTED_PRIVATE_ROOT': '/tmp', 'SELECTED_NATIVE_SERVER': str(native), 'RUST_LOG': 'off'}
result = subprocess.run(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=120)
(fixture / 'result.log').write_bytes(result.stdout)
print('Retained private fixture:', fixture)
print(result.stdout.decode(errors='replace'))
sys.exit(result.returncode)
