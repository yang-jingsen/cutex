"""Bounded standard/Lite fixture checks; no service or protocol simulator."""
import json
import re
EXPECTED=b'v3-job-output'

def declarations(request):
    blocks=[i for i in request.get('input',[]) if i.get('type')=='additional_tools']; top=request.get('tools')
    assert not (top is not None and blocks), 'mixed declarations'
    if blocks:
        assert len(blocks)==1 and blocks[0].get('role')=='developer'
        raw=blocks[0]['tools']
    else:
        assert isinstance(top,list), 'missing declarations'
        raw=top
    result={}
    def add(t,ns):
        assert t.get('type') in ('custom','function') and isinstance(t.get('name'),str)
        key=(ns,t['name']); assert key not in result, 'duplicate declaration'
        result[key]=t
    for t in raw:
        if t.get('type')=='namespace':
            for nested in t['tools']: add(nested,t['name'])
        elif t.get('type') in ('custom','function'): add(t,None)
    return result

def assert_advertised(item,request):
    assert isinstance(item.get('call_id'),str) and 0<len(item['call_id'])<=64
    kind={'custom_tool_call':'custom','function_call':'function'}[item['type']]
    tool=declarations(request).get((item.get('namespace'),item['name']))
    assert tool and tool['type']==kind, 'unadvertised qualified tool'
    if kind=='custom': assert isinstance(item.get('input'),str) and 'arguments' not in item
    else:
        args=json.loads(item['arguments']); schema=tool.get('parameters',{})
        assert isinstance(args,dict) and set(schema.get('required',[]))<=args.keys()
        if schema.get('additionalProperties') is False: assert args.keys()<=schema.get('properties',{}).keys()

def response_events(item,request,rid):
    assert isinstance(rid,str) and 0<len(rid)<=64
    if item['type'] in ('custom_tool_call','function_call'): assert_advertised(item,request)
    else:
        assert item['type']=='message' and item['role']=='assistant' and item.get('id')
        assert all(c['type']=='output_text' and isinstance(c['text'],str) for c in item['content'])
    return [{'type':'response.created','response':{'id':rid}}, {'type':'response.output_item.done','item':item},
        {'type':'response.completed','response':{'id':rid,'usage':{'input_tokens':0,'output_tokens':0,'total_tokens':0}}}]

def output_text(value):
    if isinstance(value,str): return value
    assert isinstance(value,list) and all(i.get('type') in ('input_text','text') and isinstance(i.get('text'),str) for i in value)
    return '\n'.join(i['text'] for i in value)

def correlated_output(request,call_id):
    found=[i for i in request.get('input',[]) if i.get('call_id')==call_id and i.get('type') in ('custom_tool_call_output','function_call_output')]
    assert len(found)==1, 'missing/duplicate correlated output'
    text=output_text(found[0]['output'])
    assert 'Script error:' not in text and not text.startswith(('Script failed','Script terminated')), text[:400]
    if text.startswith('Script running with cell ID '):
        cell=text.splitlines()[0].removeprefix('Script running with cell ID ')
        assert re.fullmatch(r'[A-Za-z0-9_.:-]{1,128}',cell)
        return 'running',cell,text
    assert text.startswith('Script completed\n'), 'unknown script terminal status'
    return 'completed',None,text

def marker(texts,label):
    values=[line[len(label)+1:] for text in texts for line in text.splitlines() if line.startswith(label+' ')]
    assert len(values)==1, 'missing/duplicate marker '+label
    value=json.loads(values[0]); assert isinstance(value,dict)
    return value

def submit_result(receipt,jobs,action):
    assert receipt.get('status')=='committed' and receipt.get('deduplicated') is False
    j=receipt['job']; jid=j['jobId']
    assert len(jobs)==1 and jid in jobs and j['request']['actionId']==action
    assert jobs[jid]['request']==j['request'] and jobs[jid]['jobId']==jid
    return jid

def output_result(query,page,jid):
    assert query['jobId']==jid and query['state']=='exited' and query['exitCode']==0
    assert page['jobId']==jid and page['stream']=='stdout'
    assert page['fromOffset']==0 and page['nextOffset']==len(EXPECTED)
    assert page['gap'] is False and page['truncated'] is False
    assert bytes.fromhex(page['bytesHex'])==EXPECTED

def completion_projection(request,envelope,receipt,jid):
    assert receipt and envelope['view']['data']['jobId']==jid
    expected={'source':envelope['message']['source'],'type':envelope['message']['type'],'text':envelope['message']['text']}
    assert expected['source']=={'kind':'service','id':'cutex-job-service'} and expected['type']=='job_completion'
    rows=[json.loads(i['output']) for i in request.get('input',[]) if i.get('type')=='function_call_output' and i.get('name')=='external_event']
    assert rows.count(expected)==1, 'missing/duplicate exact canonical completion'
    return expected

def fresh_prompt(screen,start,needle):
    position=screen.find(needle,start)
    assert position>=start, 'new approval prompt absent'
    return position+len(needle)
