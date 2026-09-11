"""Finite offline protocol/phase regressions; no native, services or requests."""
import copy
import json
import unittest
from pathlib import Path
from job_view_v4_protocol import *
from job_view_v4_driver import Driver

ROOT=Path(__file__).resolve().parents[2]
LITE=json.loads((ROOT/'jv03/model-requests.json').read_text())[0]
STANDARD={'tools':LITE['input'][0]['tools']}
CALL={'type':'custom_tool_call','namespace':'functions','name':'exec','call_id':'call','input':'text(1)'}
JOB={'jobId':'job_one','request':{'actionId':'v3-private-job'},'state':'exited','exitCode':0}
RECEIPT={'status':'committed','deduplicated':False,'job':JOB}
PAGE={'jobId':'job_one','stream':'stdout','fromOffset':0,'nextOffset':13,'gap':False,'truncated':False,'bytesHex':EXPECTED.hex()}
ENVELOPE={'ownerId':'owner','threadId':'thread','view':{'data':{'jobId':'job_one'}},
    'message':{'source':{'kind':'service','id':'cutex-job-service'},'type':'job_completion','text':'Job completed. jobId: job_one'}}

def req(call,text):
    return {**STANDARD,'input':[{'type':'custom_tool_call_output','call_id':call,'output':text}]}

class MemoryPath:
    def __init__(self,path,files): self.path=path; self.files=files
    def __truediv__(self,key): return MemoryPath(self.path+'/'+key,self.files)
    def __str__(self): return self.path
    def exists(self): return self.path in self.files
    def read_text(self): return self.files[self.path]
    def write_text(self,text): self.files[self.path]=text

def driver_fixture():
    files={'run/job-state/state.json':json.dumps({'jobs':{'job_one':JOB}}),'conf/runtime/management-v2/agent-bus-message-state.json':'{}'}
    snap={'externalInput':ENVELOPE,'externalInputReceipt':{'receiptId':'offline-only'}}
    g={'RUN':MemoryPath('run',files),'CONF':MemoryPath('conf',files),'durable':'owner','thread':'thread',
        'ledger':lambda:{'messages':{'m':{'snapshot':snap}}},'wait_for':lambda f,t:f()}
    return Driver(g),files

class Offline(unittest.TestCase):
    def reject(self,fn):
        with self.assertRaises((AssertionError,KeyError,TypeError,ValueError)): fn()

    def test_actual_lite_and_standard(self):
        for request in (LITE,STANDARD):
            assert_advertised(CALL,request)
            events=response_events(CALL,request,'response')
            self.assertEqual(json.loads(json.dumps(events)),events)
        assert_advertised({k:v for k,v in CALL.items() if k!='namespace'}, {'tools':[{'type':'custom','name':'exec'}]})

    def test_declaration_and_response_negatives(self):
        for delta in ({'namespace':'wrong'},{'name':'functions.exec'},{'type':'function_call','arguments':'{}'},
                      {'input':{}},{'call_id':''},{'arguments':'{}'}):
            self.reject(lambda delta=delta:response_events(CALL|delta,LITE,'r'))
        for request in ({'input':[]},LITE|STANDARD): self.reject(lambda request=request:declarations(request))
        bad=copy.deepcopy(LITE); bad['input'][0]['role']='user'; self.reject(lambda:declarations(bad))
        bad=copy.deepcopy(STANDARD); bad['tools'][0]['tools']*=2; self.reject(lambda:declarations(bad))
        self.reject(lambda:response_events(CALL,LITE,''))
        self.reject(lambda:response_events({'type':'message','role':'assistant','id':'m','content':[{'type':'input_text','text':'bad'}]},LITE,'r'))

    def test_script_errors_and_exact_correlation(self):
        for text in ('Script failed\n','Script terminated\n','Script completed\nScript error:\nMCP error','not a status'):
            self.reject(lambda text=text:correlated_output(req('c',text),'c'))
        self.reject(lambda:correlated_output(req('old','Script completed\n'),'new'))
        bad=req('c','Script completed\n'); bad['input']*=2
        self.reject(lambda:correlated_output(bad,'c'))
        self.assertEqual(correlated_output(req('c','Script running with cell ID 7\n'),'c')[:2],('running','7'))

    def test_receipt_and_marker_negatives(self):
        self.assertEqual(submit_result(RECEIPT,{'job_one':JOB},'v3-private-job'),'job_one')
        for delta in ({'status':'failed'},{'deduplicated':True},{'job':{**JOB,'request':{'actionId':'other'}}}):
            self.reject(lambda delta=delta:submit_result(RECEIPT|delta,{'job_one':JOB},'v3-private-job'))
        self.reject(lambda:submit_result(RECEIPT,{},'v3-private-job'))
        self.reject(lambda:submit_result(RECEIPT,{'job_one':JOB,'duplicate':JOB},'v3-private-job'))
        self.reject(lambda:marker(['Script completed\n'],'JV_SUBMIT'))
        self.reject(lambda:marker(['JV_SUBMIT {}\nJV_SUBMIT {}'],'JV_SUBMIT'))

    def test_output_page_negatives(self):
        output_result(JOB,PAGE,'job_one')
        for delta in ({'jobId':'wrong'},{'stream':'stderr'},{'fromOffset':1},{'nextOffset':12},
                      {'gap':True},{'truncated':True},{'bytesHex':'00'},{'bytesHex':'invalid'}):
            self.reject(lambda delta=delta:output_result(JOB,PAGE|delta,'job_one'))
        self.reject(lambda:output_result(JOB|{'exitCode':2},PAGE,'job_one'))

    def test_completion_and_old_prompt_negatives(self):
        good={'source':ENVELOPE['message']['source'],'type':'job_completion','text':ENVELOPE['message']['text']}
        request={'input':[{'type':'function_call_output','name':'external_event','output':json.dumps(good)}]}
        completion_projection(request,ENVELOPE,{'receiptId':'r'},'job_one')
        self.reject(lambda:completion_projection({'input':[{'type':'message','content':'Job completed.'}]},ENVELOPE,{'r':1},'job_one'))
        self.reject(lambda:completion_projection(request,ENVELOPE,None,'job_one'))
        old=copy.deepcopy(request); old['input'][0]['output']=json.dumps(good|{'text':'old job'})
        self.reject(lambda:completion_projection(old,ENVELOPE,{'r':1},'job_one'))
        self.reject(lambda:fresh_prompt(b'old approval',12,b'approval'))
        self.assertEqual(fresh_prompt(b'old approval NEW approval',12,b'approval'),25)

    def test_correlated_yield_wait_complete_all_phases(self):
        d,files=driver_fixture(); call=d.next(LITE); response_events(call,LITE,'r1')
        wait=d.next(req(call['call_id'],'Script running with cell ID 7\nOutput:\n'))
        self.assertEqual(json.loads(wait['arguments'])['cell_id'],'7'); response_events(wait,LITE,'r2')
        submit=d.next(req(wait['call_id'],'Script completed\nOutput:\nJV_SUBMIT '+json.dumps(RECEIPT)))
        self.assertEqual(submit['content'][0]['text'],'Submitted')
        ext={'type':'function_call_output','name':'external_event','output':json.dumps(ENVELOPE['message'])}
        out=d.next({**STANDARD,'input':[ext]}); self.assertEqual(out['call_id'],'jv4-output')
        ack=d.next(req(out['call_id'],'Script completed\nOutput:\nJV_QUERY '+json.dumps(JOB)+'\nJV_OUTPUT '+json.dumps(PAGE)))
        self.assertEqual(d.phase,'done'); self.assertIn('acknowledged',ack['content'][0]['text'])
        self.assertEqual((d.submit_count,d.output_count),(1,1))
        self.reject(lambda:d.next(LITE))

    def test_failed_script_never_submitted(self):
        d,_=driver_fixture(); c=d.next(LITE)
        self.reject(lambda:d.next(req(c['call_id'],'Script failed\nScript error:\nMCP error')))
        self.assertEqual(d.phase,'submit')

if __name__=='__main__': unittest.main()
