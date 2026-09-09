"""Owned Unix WebSocket relay retaining method/id/error-code metadata only."""
import json, socket, struct, threading

def relay(path, upstream, output):
    listener=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM)
    listener.bind(str(path)); listener.listen(1)
    lock=threading.Lock()
    def serve():
        client,_=listener.accept()
        server=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM); server.connect(str(upstream))
        def copy(source,target,direction):
            pending=b''; upgraded=False
            try:
                while True:
                    data=source.recv(65536)
                    if not data: break
                    target.sendall(data); pending+=data
                    if not upgraded:
                        if b'\r\n\r\n' not in pending: continue
                        _,pending=pending.split(b'\r\n\r\n',1); upgraded=True
                    while len(pending)>=2:
                        n=pending[1]&127; offset=2
                        if n==126:
                            if len(pending)<4: break
                            n=struct.unpack('!H',pending[2:4])[0]; offset=4
                        elif n==127:
                            if len(pending)<10: break
                            n=struct.unpack('!Q',pending[2:10])[0]; offset=10
                        masked=bool(pending[1]&128); end=offset+(4 if masked else 0)+n
                        if len(pending)<end: break
                        mask=pending[offset:offset+4] if masked else b''
                        payload=pending[offset+(4 if masked else 0):end]
                        opcode=pending[0]&15; pending=pending[end:]
                        if masked: payload=bytes(x^mask[i%4] for i,x in enumerate(payload))
                        if opcode!=1: continue
                        value=json.loads(payload)
                        meta={'direction':direction,'id':value.get('id'),'method':value.get('method')}
                        if 'error' in value: meta['error_code']=value['error'].get('code')
                        with lock:
                            output.write(json.dumps(meta)+'\n'); output.flush()
            except (OSError,ValueError): pass
            finally:
                try: target.shutdown(socket.SHUT_WR)
                except OSError: pass
        a=threading.Thread(target=copy,args=(client,server,'request'),daemon=True)
        b=threading.Thread(target=copy,args=(server,client,'response'),daemon=True)
        a.start(); b.start(); a.join(); b.join(); client.close(); server.close()
    threading.Thread(target=serve,daemon=True).start()
    return listener
