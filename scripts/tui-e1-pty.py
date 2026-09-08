"""Private Linux PTY: production key routing, resize and read-only detail return.
No runtime/catalog/provider is launched. Takes the Rust test executable path.
"""
import fcntl
import os
import select
import signal
import struct
import subprocess
import sys
import termios
import time

master, slave = os.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
baseline = termios.tcgetattr(slave)
inspect = "--inspector" in sys.argv[2:]
child_env = dict(os.environ, TERM="xterm-256color")
if inspect:
    child_env["CUTEX_UI_PTY_INSPECTOR"] = "1"
child = subprocess.Popen([sys.argv[1], "ui_contract_e1_terminal_resize_detail_child", "--ignored", "--nocapture", "--test-threads=1"], stdin=slave, stdout=slave, stderr=slave, env=child_env, start_new_session=True)
output = b""
sent = set()
try:
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline and child.poll() is None:
        if select.select([master], [], [], .1)[0]:
            output += os.read(master, 65536)
        for marker, key in [(b"E1_READY", b"\x1bi" if inspect else b"\x1b[12~"), (b"E1_DETAILS", b""), (b"E1_RESIZED", b"\x1b[F"), (b"E1_SCROLLED", b"\x1b")]:
            if marker in output and marker not in sent:
                sent.add(marker)
                if marker == b"E1_DETAILS":
                    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 18, 60, 0, 0))
                    os.kill(child.pid, signal.SIGWINCH)
                os.write(master, key)
    assert child.poll() == 0, output[-3000:]
    while select.select([master], [], [], .1)[0]:
        output += os.read(master, 65536)
    assert b"E1_COOKED" in output, output[-3000:]
    assert termios.tcgetattr(slave) == baseline
    print(f"PASS real private PTY: {'Alt+I Inspector' if inspect else 'F2 details'}/End/Esc, 120x30 -> 60x18 resize, stable selection and termios restoration; no runtime")
finally:
    if child.poll() is None:
        os.killpg(child.pid, signal.SIGTERM)
        child.wait(timeout=3)
    os.close(master)
    os.close(slave)
