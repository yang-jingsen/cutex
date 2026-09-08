"""Actual native interactive Resume through TerminalShell; saved fixture only.
No prompt or model turn is submitted. Parent sends only terminal query replies
and Ctrl+C. Invoke with private fixture env and absolute Rust test executable.
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
env = dict(os.environ, TERM="xterm-256color")
child = subprocess.Popen([sys.argv[1], "ui_contract_d20_real_native_resume_terminal_child",
    "--ignored", "--nocapture", "--test-threads=1"], stdin=slave, stdout=slave,
    stderr=slave, env=env, start_new_session=True)
output = b""
started = None
last_interrupt = 0
try:
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline and child.poll() is None:
        if select.select([master], [], [], .1)[0]:
            data = os.read(master, 65536)
            output += data
            if b"\x1b[6n" in data:
                os.write(master, b"\x1b[1;1R")
            if b"\x1b[c" in data:
                os.write(master, b"\x1b[?1;2c")
        if b"D2_NATIVE_READY" in output and started is None:
            started = time.monotonic()
        if started and time.monotonic() - started > 2 and time.monotonic() - last_interrupt > 1:
            os.write(master, b"\x03")
            last_interrupt = time.monotonic()
    assert child.poll() == 0, output[-4000:]
    while select.select([master], [], [], .1)[0]:
        output += os.read(master, 65536)
    assert b"D2_NATIVE_RETURNED" in output and b"D2_NATIVE_COOKED" in output, output[-4000:]
    assert termios.tcgetattr(slave) == baseline, "terminal not restored"
    print("PASS real native saved-session interactive child returned; private Cutex snapshots unchanged; final terminal restored")
finally:
    if child.poll() is None:
        os.killpg(child.pid, signal.SIGTERM)
        child.wait(timeout=3)
    os.close(master)
    os.close(slave)
