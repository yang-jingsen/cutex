"""Owned PTY attach/exit only: no prompt, Enter, approval or model turn."""
import fcntl,json,os,re,select,signal,struct,subprocess,termios,time
from pathlib import Path
root=Path(__file__).resolve().parents[1];run=root/'h1'
master,slave=os.openpty()
fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack('HHHH',32,120,0,0))
original=termios.tcgetattr(slave)
child=subprocess.Popen(['/usr/bin/python3','-B',str(root/'fixtures/fixed_human_entry.py'),'attach'],
 stdin=slave,stdout=slave,stderr=slave,start_new_session=True,
 preexec_fn=lambda:fcntl.ioctl(0,termios.TIOCSCTTY,0),
 env={'PATH':'/usr/local/bin:/usr/bin:/bin','TERM':'xterm-256color','LANG':'C.UTF-8'},cwd=run)
screen=b'';ready=False;forced=False
try:
 until=time.monotonic()+50
 while time.monotonic()<until:
  assert child.poll() is None,'attach exited before screen'
  if select.select([master],[],[],.2)[0]:
   part=os.read(master,65536);screen=(screen+part)[-262144:]
   if b'\x1b[6n' in part:os.write(master,b'\x1b[1;1R')
   if b'\x1b[c' in part:os.write(master,b'\x1b[?1;2c')
   plain=re.sub(rb'\x1b\[[0-9;?<>=]*[ -/]*[@-~]',b'',screen)
   if b'gpt-5.6-terra' in plain:
    ready=True;break
 assert ready,'model label not observed; no input sent'
finally:
 if child.poll() is None:
  os.write(master,b'\x03\x03')
  try:child.wait(timeout=10)
  except subprocess.TimeoutExpired:
   forced=True;os.killpg(child.pid,signal.SIGTERM);child.wait(timeout=10)
 restored=termios.tcgetattr(slave)==original
 os.close(master);os.close(slave)
 result={'sameOwnerAttachScreen':ready,'modelLabel':'gpt-5.6-terra' if ready else None,
         'textOrEnterSent':False,'forcedCliTermination':forced,'exit':child.returncode,'termiosRestored':restored}
 (run/'cli-smoke.json').write_text(json.dumps(result,indent=2));print(json.dumps(result))
assert ready and not forced and child.returncode==0 and restored
