"""Local Human diagnostic only. Never read credentials or forward messages."""
import json,re,os

def redact(value):
    if not isinstance(value,str):return None
    text=value[:16384]
    # Discard embedded dumps, rather than attempting to retain their structure.
    text=re.split(r'(?i)(?:request body|request headers|request config|environment dump)\s*[:=]',text,maxsplit=1)[0]
    text=re.sub(r'(?im)^\s*(?:authorization|proxy-authorization|cookie|set-cookie|content-type|content-length|x-[\w-]+)\s*:.*$', '[header removed]',text)
    text=re.sub(r'(?i)\b(?:bearer|basic)\s+[^\s,;"\x27]+','[credential removed]',text)
    text=re.sub(r'(?i)(["\x27]?(?:access_token|refresh_token|id_token|api_key|apikey|password|secret|token|authorization|cookie)["\x27]?\s*[:=]\s*)(?:"[^"]*"|\x27[^\x27]*\x27|[^\s,;}]+)',r'\1[redacted]',text)
    text=re.sub(r'https?://[^\s<>"\x27]+','[url removed]',text)
    text=re.sub(r'\b(?:sk-|rk-|sess-)[A-Za-z0-9_-]+','[credential removed]',text)
    text=re.sub(r'\b[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b','[token-like value removed]',text)
    text=re.sub(r'\b[A-Za-z0-9_+/=-]{24,}\b','[opaque value removed]',text)
    return ''.join(c for c in text if c in '\n\t' or ord(c)>=32)

def extract(event):
    p=event.get('params') or {};method=event.get('method')
    if method=='error':error=p.get('error');retry=p.get('willRetry')
    elif method=='turn/completed':error=(p.get('turn') or {}).get('error');retry=False
    else:return None
    if not isinstance(error,dict):return None
    from real_provider_error_observer import observe
    out=observe(event)
    out['message']=redact(error.get('message'))
    # additionalDetails may contain full requests; intentionally never retain.
    return out

def append(path,event):
    item=extract(event)
    if item is None:return
    fd=os.open(path,os.O_WRONLY|os.O_APPEND|os.O_CREAT|os.O_NOFOLLOW,0o600)
    try:
        st=os.fstat(fd)
        if st.st_uid!=os.getuid() or st.st_mode&0o077:raise RuntimeError('private diagnostic custody required')
        os.write(fd,(json.dumps(item,ensure_ascii=False)+'\n').encode())
    finally:os.close(fd)

if __name__=='__main__':
    explanation='Unsupported external event output representation; expected a call identifier.'
    cases=[explanation,'Authorization: Bearer SENTINEL_A','{"access_token":"SENTINEL_B"}',
           'api_key=SENTINEL_C password="SENTINEL_D"','URL https://user:SENTINEL_E@example.test/?token=SENTINEL_F',
           'sk-SENTINEL_G eyJhbGciOiJIUzI1NiJ9.eyJzZWNyZXQiOiJTRU5USU5FTCif.signature',
           'connection failed. request body: SENTINEL_H']
    for text in cases:
        out=redact(text);assert 'SENTINEL' not in out
    assert redact(explanation)==explanation
    e=extract({'method':'error','params':{'error':{'message':explanation,'codexErrorInfo':'other','additionalDetails':'SENTINEL_PRIVATE'},'willRetry':True}})
    assert e['message']==explanation and e['willRetry'] is True and 'SENTINEL' not in json.dumps(e)
    print('8 synthetic redaction/extraction checks passed; no runtime/auth access')
