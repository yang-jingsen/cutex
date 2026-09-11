"""Synthetic PTY drain/restore oracle. No native owner, stores or model."""
import ast,fcntl,os,select,subprocess,sys,termios,time
from pathlib import Path
tree=ast.parse(Path(__file__).with_name('selected_history_composition.py').read_text())
functions=[n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name in ('drain_exit','json_terminal')]
exec(compile(ast.Module(body=functions,type_ignores=[]),'drain-helper','exec'))
master,slave=os.openpty();before=termios.tcgetattr(slave)
code="import os,termios,tty;before=termios.tcgetattr(0);tty.setraw(0);os.write(1,b'READY');os.read(0,1);data=b'x'*1048576;\nwhile data:\n n=os.write(1,data);data=data[n:]\ntermios.tcsetattr(0,termios.TCSANOW,before)"
child=subprocess.Popen([sys.executable,'-B','-c',code],stdin=slave,stdout=slave,stderr=slave)
try:
    assert select.select([master],[],[],5)[0]
    assert os.read(master,5)==b'READY'
    os.write(master,b'\x03');chunks=[]
    assert drain_exit(child,master,chunks.append,10)
    while select.select([master],[],[],.05)[0]:chunks.append(os.read(master,65536))
    assert child.returncode==0 and termios.tcgetattr(slave)==before
    assert sum(map(len,chunks))==1048576
    print('PASS: drain 1MiB during normal exit; code0 and exact termios restoration')
finally:
    if child.poll() is None:child.kill();child.wait()
    os.close(master);os.close(slave)
