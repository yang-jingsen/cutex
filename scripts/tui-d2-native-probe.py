"""Owned, model-free native protocol probe. No operator configuration is read.

Usage: python3 scripts/tui-d2-native-probe.py /absolute/cute-codex
No turn/start or prompt is sent. Temporary native state is retained for evidence.
"""
import hashlib
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import tempfile
import threading

root = Path(tempfile.mkdtemp(prefix="cutex-d2-native-"))
home = root / "home"
native = home / ".codex"
native.mkdir(parents=True)
workspace = root / "workspace"
workspace.mkdir()
env = {"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": str(home),
       "CODEX_HOME": str(native), "TMPDIR": str(root), "TERM": "dumb"}

def snapshot(path):
    return {str(p.relative_to(path)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in path.rglob("*") if p.is_file()}

class Endpoint:
    def __enter__(self):
        self.stderr = open(root / "native-stderr.log", "ab")
        self.child = subprocess.Popen([sys.argv[1], "app-server", "--stdio"],
            cwd=workspace, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=self.stderr, text=True)
        self.lines = queue.Queue()
        def read():
            for line in self.child.stdout:
                self.lines.put(line)
            self.lines.put(None)
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()
        self.sequence = 0
        self.request("initialize", {"clientInfo": {"name": "cutex_d2_probe", "version": "1"},
            "capabilities": {"experimentalApi": True}})
        self.child.stdin.write(json.dumps({"method": "initialized"}) + "\n")
        self.child.stdin.flush()
        return self
    def request(self, method, params):
        self.sequence += 1
        self.child.stdin.write(json.dumps({"id": self.sequence, "method": method, "params": params}) + "\n")
        self.child.stdin.flush()
        while True:
            line = self.lines.get(timeout=20)
            if line is None:
                raise RuntimeError("native EOF; see private stderr")
            value = json.loads(line)
            if value.get("id") == self.sequence:
                if "error" in value:
                    raise RuntimeError((method, value["error"]))
                return value["result"]
    def __exit__(self, *args):
        self.child.stdin.close()
        try:
            self.child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.child.terminate()
            self.child.wait(timeout=3)
        self.reader.join(timeout=1)
        self.stderr.close()

print("PRIVATE_ROOT", root, flush=True)
before = snapshot(home / ".cutex")
with Endpoint() as endpoint:
    created = endpoint.request("thread/start", {"cwd": str(workspace), "ephemeral": False,
        "approvalPolicy": "never", "sandbox": "read-only", "sessionStartSource": "startup"})
    thread_id = created["thread"]["id"]
    print("START", thread_id, "path", created["thread"].get("path"), flush=True)
    try:
        read = endpoint.request("thread/read", {"threadId": thread_id, "includeTurns": True})
        print("READ", read["thread"]["id"], flush=True)
    except RuntimeError as error:
        print("READ_RESULT", error, flush=True)
try:
    with Endpoint() as endpoint:
        resumed = endpoint.request("thread/resume", {"threadId": thread_id})
        assert resumed["thread"]["id"] == thread_id
finally:
    unchanged = snapshot(home / ".cutex") == before
    print("CUTEX_UNCHANGED", unchanged, "NATIVE_FILES", sorted(snapshot(native)), flush=True)
    assert unchanged, "native operation changed private Cutex state"
print("PASS model-free thread/start -> process exit -> exact thread/resume; Cutex state unchanged")
