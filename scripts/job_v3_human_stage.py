"""One-time pv3 boundary verification and scoped guest credential staging."""
import hashlib,json,os,stat
from pathlib import Path

root=Path('/home/cutex-linux-test/acceptance-upload/pv3')
assert Path(__file__).resolve().parents[1]==root and root.resolve()==root
os.umask(0o077)
manifest=root/'build-manifest.json'
assert hashlib.sha256(manifest.read_bytes()).hexdigest()=='e1e0b16b4435d2b522ab0128b9ac2632428ecc4dd118a912645c7560af118d5d'
m=json.loads(manifest.read_text())
files={'bin/cutex':'cutex_sha256','bin/cutex-mcp':'cutex_mcp_sha256','bin/cutex-job-service':'job_sha256',
 'native/bin/codex':'native_cli_sha256','native/bin/codex-app-server':'native_server_sha256',
 'native/bin/codex-code-mode-host':'native_host_sha256',
 'native/schema/codex_app_server_protocol.schemas.json':'native_stable_schema_sha256'}
for name,key in files.items():
 p=root/name
 assert p.resolve()==p and p.stat().st_uid==os.getuid() and p.is_file()
 assert hashlib.sha256(p.read_bytes()).hexdigest()==m[key],name
assert not (root/'h1').exists()
source=Path('/home/cutex-linux-test/acceptance-upload/pv2/h1/h/.cutex/codex-home/auth.json')
assert source.resolve()==source
fd=os.open(source,os.O_RDONLY|os.O_NOFOLLOW)
try:
 before=os.fstat(fd)
 assert stat.S_ISREG(before.st_mode) and before.st_uid==os.getuid() and stat.S_IMODE(before.st_mode)==0o600
 assert 0<before.st_size<1024*1024
 with os.fdopen(os.dup(fd),'rb') as stream: data=stream.read(1024*1024)
 after=os.fstat(fd)
 assert (before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns)==(after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns)
 out=os.open(root/'staged-auth.json',os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
 with os.fdopen(out,'wb') as stream: stream.write(data)
finally: os.close(fd)
print('Exact transferred bundle verified; authorized guest auth staged 0600; source unchanged. No provider request.')
