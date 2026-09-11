"""One submit / one query+read, correlated by call and cell, for this fixture."""
import json
from job_view_v4_protocol import correlated_output, marker, submit_result, output_result, completion_projection

UNWRAP='''function value(r) {
 if (!r || r.isError === true || r.error || r.ok === false) throw Error("MCP error");
 let v = r.structuredContent;
 if (v === undefined && Array.isArray(r.content)) {
   const t=r.content.filter(x=>x.type==="text");
   if(t.length!==1) throw Error("MCP text shape"); v=JSON.parse(t[0].text);
 }
 if(v===undefined) v=r;
 if(!v || typeof v!=="object" || v.error || v.ok===false) throw Error("MCP result shape");
 return v;
}
'''

class Driver:
    def __init__(self,g):
        self.g=g; self.phase='new'; self.pending=None; self.cell=None
        self.parts=[]; self.waits=0; self.jid=None; self.submit_count=0; self.output_count=0

    def exec(self,call,code):
        self.pending=call; self.parts=[]; self.cell=None
        return {'type':'custom_tool_call','namespace':'functions','name':'exec','call_id':call,'input':UNWRAP+code}

    def reply(self,text,ident):
        return {'type':'message','role':'assistant','id':ident,'content':[{'type':'output_text','text':text}]}

    def next(self,request):
        g=self.g
        if self.phase=='new':
            self.phase='submit'; self.submit_count+=1
            args={'actionId':'v3-private-job','argv':['/bin/sh','-c','printf v3-job-output; /usr/bin/python3 -c "import time; time.sleep(0.2)"'],'cwd':str(g['RUN'])}
            return self.exec('jv4-submit','const r=value(await tools.mcp__cutex_job__submit('+json.dumps(args)+'));\n'
                'if(r.status!=="committed" || r.deduplicated!==false || !r.job || r.job.request.actionId!=="v3-private-job") throw Error("submit receipt failed");\n'
                'text("JV_SUBMIT "+JSON.stringify(r));')
        if self.phase in ('submit','output'):
            state,cell,text=correlated_output(request,self.pending); self.parts.append(text)
            if state=='running':
                assert self.cell is None or cell==self.cell, 'changed yielded cell'
                self.cell=cell; self.waits+=1; assert self.waits<=10, 'wait budget'
                self.pending=f'jv4-wait-{self.waits}'
                return {'type':'function_call','namespace':'functions','name':'wait','call_id':self.pending,
                    'arguments':json.dumps({'cell_id':cell,'yield_time_ms':10000,'max_tokens':4000})}
            if self.phase=='submit':
                receipt=marker(self.parts,'JV_SUBMIT')
                jobs=json.loads((g['RUN']/'job-state/state.json').read_text())['jobs']
                self.jid=submit_result(receipt,jobs,'v3-private-job')
                (g['RUN']/'actual-submit-receipt.json').write_text(json.dumps(receipt))
                self.phase='completion'
                return self.reply('Submitted','jv4-submitted')
            query=marker(self.parts,'JV_QUERY'); page=marker(self.parts,'JV_OUTPUT')
            output_result(query,page,self.jid)
            actual=json.loads((g['RUN']/'job-state/state.json').read_text())['jobs'][self.jid]
            assert actual['state']=='exited' and actual['exitCode']==query['exitCode']==0
            (g['RUN']/'actual-output-page.json').write_text(json.dumps(page))
            (g['RUN']/'actual-query.json').write_text(json.dumps(query))
            self.phase='done'
            return self.reply('v3-job-output acknowledged','jv4-acknowledged')
        if self.phase=='completion':
            def committed():
                path=g['CONF']/'runtime/management-v2/agent-bus-message-state.json'
                if not path.exists(): return None
                matches=[m['snapshot'] for m in g['ledger']()['messages'].values()
                    if m['snapshot'].get('externalInput',{}).get('view',{}).get('data',{}).get('jobId')==self.jid]
                assert len(matches)<=1
                return matches[0] if matches and matches[0].get('externalInputReceipt') else None
            snap=g['wait_for'](committed,30)
            e=snap['externalInput']; assert e['ownerId']==g['durable'] and e['threadId']==g['thread']
            completion_projection(request,e,snap['externalInputReceipt'],self.jid)
            self.phase='output'; self.output_count+=1
            ident=json.dumps(self.jid)
            return self.exec('jv4-output','const q=value(await tools.mcp__cutex_job__query({jobId:'+ident+'}));\n'
                'if(q.jobId!=='+ident+' || q.state!=="exited" || q.exitCode!==0) throw Error("query failed");\n'
                'text("JV_QUERY "+JSON.stringify({jobId:q.jobId,state:q.state,exitCode:q.exitCode}));\n'
                'const p=value(await tools.mcp__cutex_job__read_output({jobId:'+ident+',stream:"stdout"}));\n'
                'if(p.jobId!=='+ident+' || p.stream!=="stdout" || p.fromOffset!==0 || p.nextOffset!==13 || p.gap!==false || p.truncated!==false || p.bytesHex!=="76332d6a6f622d6f7574707574") throw Error("output mismatch");\n'
                'text("JV_OUTPUT "+JSON.stringify(p));')
        raise AssertionError('unexpected request after verified completion')
