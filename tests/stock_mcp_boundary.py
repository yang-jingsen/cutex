"""Private stock -> MCP -> real Cutex boundary. No paid model/live stores.
S1 Unix RPC/PTY adapter reused from d57cfe4; actual Cutex replaces fake Bus.
"""
import base64, fcntl, hashlib, http.client, http.server, json, os, queue, re
import select, signal, socket, struct, subprocess, termios, threading, time
from pathlib import Path
ROOT = Path(__file__).resolve().parents[2]
RUN = ROOT / "run"
assert not RUN.exists(), "preserve earlier run before authorized retry"
assert os.statvfs(ROOT).f_bavail * os.statvfs(ROOT).f_frsize > 100*1024**3
RUN.mkdir(mode=0o700)
HOME = RUN / "home"; HOME.mkdir()
(HOME / ".cutex-test-private-home").touch()
(HOME / ".cutex").mkdir()
STOCK = Path("/mnt/mambo/PersonaProjects/cutex-upstream-lightweight-r1/stock/bin/codex")
CUTEX = ROOT / "target/debug/cutex"
MCP = ROOT / "target/debug/cutex-mcp"
SOCK = ROOT / "native.sock"
BUS_TOKEN = "s2-private-bus-not-production-0123456789"
ROOT_TOKEN = "s2-private-human-not-production-9876543210"
RUNTIME = "stock-s2-director-runtime"
shared = {"thread":None,"target":None,"calls":0,"outputs":[]}
children=[]; logs=[]
env={"PATH":"/usr/local/bin:/usr/bin:/bin","HOME":str(HOME),"CODEX_HOME":str(HOME/"native"),"CUTEX_TEST_PRIVATE_HOME":str(HOME),"TMPDIR":str(ROOT/"tmp"),"TERM":"xterm-256color","LANG":"C.UTF-8"}
def spawn(args, **kw):
    child=subprocess.Popen([str(x) for x in args],env=env,start_new_session=True,**kw); children.append(child); return child
def owner(args,name):
    log=open(RUN/(name+".log"),"wb"); logs.append(log)
    return spawn(args,stdout=log,stderr=log,cwd=RUN)
def port():
    for p in range(24800,24999):
        s=socket.socket()
        try: s.bind(("127.0.0.1",p)); return p
        except OSError: pass
        finally: s.close()
    raise RuntimeError("no private port available")
def api(p,path,body=None,token=BUS_TOKEN,headers=None):
    c=http.client.HTTPConnection("127.0.0.1",p,timeout=30)
    h={"Authorization":"Bearer "+token}
    h.update(headers or {})
    c.request("POST" if body is not None else "GET",path,None if body is None else json.dumps(body),h)
    r=c.getresponse(); raw=r.read(); status=r.status; c.close()
    try: result=json.loads(raw)
    except ValueError: result=raw.decode(errors="replace")
    return status,result
def ready(p,child):
    until=time.monotonic()+20
    while time.monotonic()<until:
        assert child.poll() is None, "private service exited; inspect its log"
        try:
            with socket.create_connection(("127.0.0.1",p),.1): return
        except OSError: threading.Event().wait(.02)
    raise RuntimeError("service did not listen")
class Model(http.server.BaseHTTPRequestHandler):
    def log_message(self,*args): pass
    def do_POST(self):
        assert self.path=="/v1/responses"
        data=json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        assert self.headers.get("thread-id")==shared["thread"]
        shared["outputs"] += [i for i in data.get("input",[]) if i.get("type")=="function_call_output"]
        n=shared["calls"]; shared["calls"]+=1; assert n<8
        if n==0: item={"type":"tool_search_call","call_id":"search-s2","execution":"client","arguments":{"query":"Cutex query_managed send","limit":2}}
        elif n==7: item={"type":"message","role":"assistant","id":"done-s2","content":[{"type":"output_text","text":"stock Cutex boundary complete"}]}
        else:
            name="query_managed" if n in (1,5,6) else "send"
            args={"action_id":"s2-query","project_id":"s2-project"} if name=="query_managed" else {"to":shared["target"],"message":"private harmless S2 message","external_message_id":"s2-message","delivery_mode":"passive"}
            if n==4: args["message"]="changed replay must reject"
            if n==5: args.update(project_id="foreign-project",action_id="s2-foreign-query")
            if n==6: args["caller_cutex_session_id"]=shared["target"]
            item={"type":"function_call","call_id":f"call-s2-{n}","namespace":"mcp__cutex","name":name,"arguments":json.dumps(args)}
        events=[{"type":"response.created","response":{"id":f"s2-{n}"}},{"type":"response.output_item.done","item":item},{"type":"response.completed","response":{"id":f"s2-{n}","usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0}}}]
        body="".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events).encode()
        self.send_response(200); self.send_header("Content-Type","text/event-stream"); self.send_header("Content-Length",str(len(body))); self.end_headers(); self.wfile.write(body)

class RPC:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(10)
        self.sock.connect(str(path))
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall((f"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
        response = b""
        while not response.endswith(b"\r\n\r\n"):
            part = self.sock.recv(1)
            assert part and len(response) < 16384, "invalid WebSocket handshake"
            response += part
        assert response.startswith(b"HTTP/1.1 101"), response
        expected = base64.b64encode(hashlib.sha1((key+"258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest())
        assert expected.lower() in response.lower()
        self.sock.settimeout(None)
        self.messages = queue.Queue()
        self.events = []
        self.index = 0
        def exact(n):
            data = b""
            while len(data) < n:
                part = self.sock.recv(n-len(data))
                if not part: raise EOFError()
                data += part
            return data
        def reader():
            try:
                while True:
                    head = exact(2); opcode = head[0] & 15; length = head[1] & 127
                    if length == 126: length = struct.unpack("!H",exact(2))[0]
                    elif length == 127: length = struct.unpack("!Q",exact(8))[0]
                    assert not head[1] & 128 and head[0] & 128
                    payload = exact(length)
                    if opcode == 1: self.messages.put(json.loads(payload))
                    elif opcode == 9: self.send(payload,10)
                    elif opcode == 8: return
            except (EOFError, OSError): return
        threading.Thread(target=reader, daemon=True).start()
    def send(self, payload, opcode=1):
        mask = os.urandom(4); length = len(payload)
        size = bytes([length|128]) if length < 126 else bytes([126|128])+struct.pack("!H",length) if length < 65536 else bytes([127|128])+struct.pack("!Q",length)
        self.sock.sendall(bytes([128|opcode])+size+mask+bytes(b^mask[i%4] for i,b in enumerate(payload)))
    def call(self, method, params=None):
        self.index += 1
        self.send(json.dumps({"id": self.index, "method": method, "params": params}).encode())
        until = time.monotonic() + 40
        while time.monotonic() < until:
            msg = self.messages.get(timeout=max(.1, until-time.monotonic()))
            if msg.get("id") == self.index:
                assert "error" not in msg, (method, msg)
                return msg["result"]
            self.events.append(msg)
        raise RuntimeError("RPC timeout: " + method)
    def notify(self, method):
        self.send(json.dumps({"method": method}).encode())


model=http.server.ThreadingHTTPServer(("127.0.0.1",0),Model)
threading.Thread(target=model.serve_forever,daemon=True).start()
try:
    bp=port(); bus=owner([CUTEX,"agent","serve","--port",bp,"--token",BUS_TOKEN],"bus"); ready(bp,bus)
    mp=port()
    (HOME/".cutex/config.json").write_text(json.dumps({"agent_bus_enabled":True,"agent_bus_port":bp,"agent_bus_token":BUS_TOKEN,"management_api_token":ROOT_TOKEN}))
    management=owner([CUTEX,"management","serve","--port",mp,"--token",ROOT_TOKEN],"management"); ready(mp,management)
    native_home=HOME/"native"; native_home.mkdir()
    env.update(CUTEX_AGENT_ID=RUNTIME,CUTEX_RUNTIME_GENERATION="1",CUTEX_AGENT_BUS_URL=f"http://127.0.0.1:{bp}",CUTEX_AGENT_BUS_TOKEN=BUS_TOKEN)
    config=f'''model="gpt-5.4"
model_provider="s2-fake"
approval_policy="never"
sandbox_mode="danger-full-access"
[projects.{json.dumps(str(RUN))}]
trust_level="trusted"
[notice.model_migrations]
"gpt-5.4"="gpt-5.6-terra"
[model_providers.s2-fake]
name="s2-fake"
base_url="http://127.0.0.1:{model.server_port}/v1"
wire_api="responses"
requires_openai_auth=false
supports_websockets=false
[mcp_servers.cutex]
command={json.dumps(str(MCP))}
env_vars=["CUTEX_AGENT_ID","CUTEX_RUNTIME_GENERATION","CUTEX_AGENT_BUS_URL","CUTEX_AGENT_BUS_TOKEN"]
default_tools_approval_mode="approve"
[code_mode]
direct_only_tool_namespaces=["mcp__cutex"]
[analytics]
enabled=false
'''
    (native_home/"config.toml").write_text(config)
    native=owner([STOCK,"app-server","--listen","unix://"+str(SOCK)],"native")
    until=time.monotonic()+15
    while not SOCK.exists():
        assert native.poll() is None and time.monotonic()<until
        threading.Event().wait(.02)
    rpc=RPC(SOCK); rpc.call("initialize",{"clientInfo":{"name":"s2-private","version":"1"},"capabilities":{"experimentalApi":True}}); rpc.notify("initialized")
    def start(): return rpc.call("thread/start",{"cwd":str(RUN),"sandbox":"danger-full-access","approvalPolicy":"never","ephemeral":False})["thread"]["id"]
    shared["thread"]=start(); recipient_thread=start()
    def register(runtime,thread,name,cls="persistent"):
        status,result=api(bp,"/api/agents/register",{"id":runtime,"name":name,"baseName":name,"sessionId":thread,"profile":"","cwd":str(RUN),"pid":native.pid,"groups":["s2-private"],"registrationClass":cls})
        assert status==200 and result["ok"],(status,result)
    register(RUNTIME,shared["thread"],"S2 Director")
    register("stock-s2-recipient-runtime",recipient_thread,"S2 Recipient")
    status,candidates=api(mp,"/v2/agent-management/durable-candidates",token=ROOT_TOKEN); assert status==200,(status,candidates)
    assert len(candidates)==2,candidates
    byid={c["cutex_session_id"]:c for c in candidates}
    status,agents=api(bp,f"/api/agents?agent_id={RUNTIME}&all_hosts=false"); assert status==200
    director=next(a["cutex_session_id"] for a in agents if a["id"]==RUNTIME)
    target=next(a["cutex_session_id"] for a in agents if a["id"]=="stock-s2-recipient-runtime"); shared["target"]=target
    def mutation(action,kind,revision,**fields):
        return {"schema":"cutex/human-management-project-mutation/v1","action_id":action,"project_id":"s2-project","expected_authority_epoch":0 if kind=="create" else 1,"expected_project_revision":revision,"operation":{"kind":kind,**fields}}
    create=mutation("s2-create","create",0,director_cutex_session_id=director,presentation={"display_name":"S2 Private Project","badge_label":"S2","color":"cyan"})
    for durable,name,assignment in [(director,"S2 Director",create),(target,"S2 Recipient",None)]:
        # Fresh candidate CAS before each explicit Human import.
        status,cs=api(mp,"/v2/agent-management/durable-candidates",token=ROOT_TOKEN); assert status==200
        candidate=next(c for c in cs if c["cutex_session_id"]==durable)
        status,receipt=api(mp,"/v2/agent-management/durable-import",{"action_id":"s2-import-"+name.split()[-1],"candidate":candidate,"confirmed_formal_name":name,"assignment":assignment,"detach":None},token=ROOT_TOKEN)
        assert status==200 and receipt["complete"],(status,receipt)
    status,receipt=api(mp,"/v2/agent-management/project-mutations",mutation("s2-add","add_member",1,cutex_session_id=target),token=ROOT_TOKEN)
    assert status==200,(status,receipt)
    status,result=api(bp,"/api/agents/unregister",{"id":"stock-s2-recipient-runtime"}); assert status==200
    status,offline=api(bp,"/api/messages/send",{"to":target,"content":"offline probe","external_message_id":"s2-offline-probe","delivery_mode":"passive","from_agent_id":RUNTIME,"all_hosts":False})
    assert status==409 and offline["code"]=="target_unavailable",(status,offline)
    (RUN/"offline-gap.json").write_text(json.dumps(offline,indent=2))
    register("stock-s2-recipient-runtime",recipient_thread,"S2 Recipient")
    # Independent public-service authorization oracle, not the facade.
    query={"schema":"cutex/agent-management/v1","action_id":"s2-direct-query","operation":"query_managed","project_id":"s2-project"}
    headers={"X-Cutex-Agent-Id":RUNTIME,"X-Cutex-Mcp-Thread-Id":shared["thread"],"X-Cutex-Mcp-Generation":"1"}
    status,good=api(bp,"/api/agent-management/v1/actions",query,headers=headers)
    (RUN/"direct-query.json").write_text(json.dumps(good,indent=2))
    assert status==200 and "unauthorized" not in json.dumps(good).lower(),good
    status,replayed=api(bp,"/api/agent-management/v1/actions",query,headers=headers)
    assert replayed==good
    status,conflict=api(bp,"/api/agent-management/v1/actions",{**query,"project_id":"foreign-project"},headers=headers)
    assert conflict["outcome"]["code"]=="conflict",conflict
    direct_send={"to":target,"content":"independent private send","external_message_id":"s2-independent-send","delivery_mode":"passive","kind":"message","from_agent_id":RUNTIME,"all_groups":False,"all_hosts":False}
    status,sent=api(bp,"/api/messages/send",direct_send,headers=headers)
    (RUN/"direct-send.json").write_text(json.dumps({"status":status,"result":sent},indent=2))
    assert status==200,(status,sent)
    register(RUNTIME,shared["thread"],"S2 Director","ephemeral")
    status,result=api(bp,"/api/agent-management/v1/actions",query,headers=headers)
    assert "unauthorized" in json.dumps(result).lower()
    register(RUNTIME,shared["thread"],"S2 Director")
    denials=[]
    for label,changed,token in [
        ("wrong-token",{},"wrong-private-token"),
        ("spoof-runtime",{"X-Cutex-Agent-Id":"stock-s2-recipient-runtime"},BUS_TOKEN),
        ("foreign-thread",{"X-Cutex-Mcp-Thread-Id":recipient_thread},BUS_TOKEN),
        ("stale-generation",{"X-Cutex-Mcp-Generation":"2"},BUS_TOKEN),
        ("missing-generation",{"X-Cutex-Mcp-Generation":None},BUS_TOKEN)]:
        hs={**headers,**changed}; hs={k:v for k,v in hs.items() if v is not None}
        status,result=api(bp,"/api/agent-management/v1/actions",query,token=token,headers=hs)
        assert "unauthorized" in json.dumps(result).lower(),(label,status,result); denials.append(label)
    status,result=api(mp,"/v2/agent-management/durable-candidates",token=BUS_TOKEN); assert status==401
    forbidden={**query,"operation":"close","cutex_session_id":target}
    status,result=api(bp,"/api/agent-management/v1/actions",forbidden,headers=headers); assert "unauthorized" in json.dumps(result).lower()
    inventory=rpc.call("mcpServerStatus/list",{"threadId":shared["thread"],"detail":"toolsAndAuthOnly"})
    (RUN/"inventory.json").write_text(json.dumps(inventory,indent=2))
    assert "caller_cutex_session_id" not in json.dumps(inventory)
    raw=spawn([MCP],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    out,err=raw.communicate((json.dumps({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"query_managed","arguments":{"action_id":"missing-meta"}}})+"\n").encode(),timeout=10)
    assert raw.returncode==0 and json.loads(out)["error"]["code"]==-32602
    prompt="S2 fixed fake-model boundary proof"
    rpc.call("turn/start",{"threadId":shared["thread"],"input":[{"type":"text","text":prompt}]})
    until=time.monotonic()+45
    while not any(e.get("method")=="turn/completed" for e in rpc.events):
        assert time.monotonic()<until
        rpc.events.append(rpc.messages.get(timeout=10))
    assert shared["calls"]==8,shared
    outputs={o["call_id"]:o["output"] for o in shared["outputs"]}
    (RUN/"tool-outputs.json").write_text(json.dumps(outputs,indent=2))
    assert "s2-project" in str(outputs["call-s2-1"]),outputs
    for n in (2,3): assert "message_id" in str(outputs[f"call-s2-{n}"]),outputs
    assert "deduplicated" in str(outputs["call-s2-3"]),outputs
    def payload(n):
        value=outputs[f"call-s2-{n}"]
        if isinstance(value,list): return next(json.loads(x["text"]) for x in value if x.get("text","").startswith("{"))
        return json.loads(value.split("Output:\n",1)[1])
    first,replay,changed=payload(2),payload(3),payload(4)
    assert first["id"]==replay["id"] and replay["deduplicated"] is True
    assert changed["id"]!=first["id"] and changed["deduplicated"] is False
    for value in (first,replay,changed):
        assert value["to_cutex_session_id"]==target and value["from_cutex_session_id"]==director
        assert value["delivery_mode"]=="passive" and value["queueDurability"]=="durable_v2" and value["deliveryState"]=="pending"
    assert BUS_TOKEN not in json.dumps(outputs) and ROOT_TOKEN not in json.dumps(outputs)
    persisted=json.loads((HOME/".cutex/runtime/management-v2/agent-bus-message-state.json").read_text())
    assert first["id"] in json.dumps(persisted) and changed["id"] in json.dumps(persisted)
    assert "rejected" in str(outputs["call-s2-6"]).lower(),outputs
    assert "project_not_authorized" in str(outputs["call-s2-5"]),outputs
    before=rpc.call("thread/read",{"threadId":shared["thread"],"includeTurns":True})
    native_pid=rpc.call("server/diagnostics",{})["process"]["id"]; assert native_pid==native.pid
    master,slave=os.openpty(); fcntl.ioctl(slave,termios.TIOCSWINSZ,struct.pack("HHHH",30,120,0,0)); original=termios.tcgetattr(slave)
    cli=spawn([STOCK,"resume","--remote","unix://"+str(SOCK),shared["thread"],"--no-alt-screen","--cd",RUN,"-c",'tui.resume_cwd="current"'],stdin=slave,stdout=slave,stderr=slave,cwd=RUN,preexec_fn=lambda:fcntl.ioctl(0,termios.TIOCSCTTY,0))
    screen=b""; exited=False; until=time.monotonic()+25
    while time.monotonic()<until and cli.poll() is None:
        if select.select([master],[],[],.1)[0]:
            chunk=os.read(master,65536); screen+=chunk
            if b"\x1b[6n" in chunk: os.write(master,b"\x1b[1;1R")
            if b"\x1b[c" in chunk: os.write(master,b"\x1b[?1;2c")
        plain=re.sub(rb"\x1b\[[0-9;?<>=]*[ -/]*[@-~]",b"",screen).replace(b" ",b"")
        if b"stockCutexboundarycomplete" in plain and not exited: os.write(master,b"\x03\x03"); exited=True
    (RUN/"cli.bin").write_bytes(screen)
    assert exited and cli.poll()==0 and termios.tcgetattr(slave)==original
    os.close(master); os.close(slave)
    after=rpc.call("thread/read",{"threadId":shared["thread"],"includeTurns":True})
    assert before["thread"]["turns"]==after["thread"]["turns"] and shared["calls"]==8
    assert rpc.call("server/diagnostics",{})["process"]["id"]==native_pid
    register("stock-s2-successor-runtime",shared["thread"],"S2 Director")
    status,result=api(bp,"/api/agent-management/v1/actions",query,headers=headers)
    assert "unauthorized" in json.dumps(result).lower()
    status,result=api(bp,"/api/agent-management/v1/actions",query,headers={**headers,"X-Cutex-Agent-Id":"stock-s2-successor-runtime"})
    assert "unauthorized" in json.dumps(result).lower()
    (RUN/"PASS.json").write_text(json.dumps({"director":director,"recipient":target,"nativeThread":shared["thread"],"realProvider":True,"fakeModelRequests":8,"independentDenials":denials,"cliOwnerUnchanged":True,"historyUnchanged":True,"generationReplacementRejected":True,"exactReplayMessageId":first["id"],"changedPayloadMessageId":changed["id"],"offlineTargetRejected":True,"inboundDeliveryClaim":False},indent=2))
    print("PASS real private Cutex registration/authority/send and stock MCP/CLI")
finally:
    for child in reversed(children):
        if child.poll() is None:
            os.killpg(child.pid,signal.SIGTERM)
            try: child.wait(timeout=3)
            except subprocess.TimeoutExpired: os.killpg(child.pid,signal.SIGKILL); child.wait(timeout=3)
    model.shutdown(); model.server_close()
    for log in logs: log.close()
