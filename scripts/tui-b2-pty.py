"""Linux PTY oracle for the actual TerminalShell adapter, not live Cutex services.

Usage: python3 scripts/tui-b2-pty.py /absolute/path/to/cutex-test-executable
Run with a scrubbed environment/private HOME. The child is a mock foreground
process; this does not establish cute-codex/runtime interoperability.
"""
import os
import fcntl
import struct
import select
import subprocess
import sys
import termios
import time

master, slave = os.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 120, 0, 0))
baseline = termios.tcgetattr(slave)
env = dict(os.environ, CUTEX_TUI_PTY_CHILD="1", TERM="dumb")
process = subprocess.Popen(
    [sys.argv[1], "ui_contract_b2_terminal_process_child", "--nocapture", "--test-threads=1"],
    stdin=slave, stdout=slave, stderr=slave, env=env,
)
output = b""


def until(marker):
    global output
    deadline = time.monotonic() + 15
    while marker not in output:
        assert time.monotonic() < deadline, (marker, output[-2000:])
        ready, _, _ = select.select([master], [], [], 0.2)
        if ready:
            output += os.read(master, 65536)


def canonical(expected):
    flags = termios.tcgetattr(slave)[3]
    assert bool(flags & termios.ICANON) == expected, flags
    assert bool(flags & termios.ECHO) == expected, flags


try:
    for step in range(60):
        until(f"B2_SWITCH_{step}".encode())
        canonical(False)
        os.write(master, [b"\x1br", b"\x1bp", b"\x1bm"][step % 3])
    until(b"B2_RAW_READY")
    assert output.count(b"\x1b[?1049h") == 1
    assert output.count(b"\x1b[?1049l") == 0
    canonical(False)
    os.write(master, b"a")
    until(b"B2_CHILD_READY")
    canonical(True)
    os.write(master, b"child\n")
    until(b"B2_RESUMED")
    canonical(False)
    os.write(master, b"b")
    until(b"B2_ERROR_RESUMED")
    canonical(False)
    os.write(master, b"c")
    until(b"B2_COOKED_DONE")
    assert process.wait(timeout=10) == 0, output[-2000:]
    assert termios.tcgetattr(slave) == baseline
    assert output.count(b"\x1b[?1049h") == 3, output
    assert output.count(b"\x1b[?1049l") == 3, output
    print("PASS real PTY: 20 three-page round trips / one terminal lifetime; raw/cooked mock-child handoff, single input owner, error resume, final termios and alternate-screen balance")
finally:
    if process.poll() is None:
        process.kill()
        process.wait()
    os.close(master)
    os.close(slave)
