"""S6c3 real root recovery, using the accepted private S6c2 modes entrance."""
import ast
from pathlib import Path

def recovery(g):
    import copy,json,time
    api=g['api'];mp=g['mp'];model=g['Model'];rpc=g['rpc'];params=g['params']
    endpoint='/v2/agent-management/native-recovery'
    request={'operation':'review','cutex_session_id':g['durable'],'message_id':g['empty']}
    assert api(mp,endpoint,request,token=g['BUS_TOKEN'])[0]==401
    assert api(mp,endpoint,{**request,'caller':'root'})[0]==400
    before=model.calls
    code,review=api(mp,endpoint,request);assert code==200,(code,review)
    assert review['status']['processing']['reason']=='no_output' and review['status']['receipt']
    assert api(mp,endpoint,request)==(200,review) and model.calls==before
    confirmation={'operation':'confirm','action_id':'s6c3-retry-one','retry_id':'s6c3-native-retry-one','review':review,'confirm_repeat':True}
    assert api(mp,endpoint,{**confirmation,'confirm_repeat':False})[0]==409
    forged=copy.deepcopy(confirmation);forged['review']['binding']['runtimeGeneration']+=1
    assert api(mp,endpoint,forged)[0]==409
    assert api(mp,endpoint,confirmation,token=g['BUS_TOKEN'])[0]==401
    assert api(mp,endpoint,{'operation':'status','action_id':'s6c3-retry-one'})==(200,None)
    assert model.calls==before
    model.empty=False
    (g['RUN']/'recovery-review.json').write_text(json.dumps(review,indent=2))
    code,receipt=api(mp,endpoint,confirmation)
    if code!=200:
        _,fresh=api(mp,endpoint,request)
        (g['RUN']/'recovery-review-diff.json').write_text(json.dumps({'changed_fields':[k for k in review if review[k]!=fresh.get(k)],'response':receipt},indent=2))
    assert code==200,(code,receipt)
    assert receipt['result']['retryId']=='s6c3-native-retry-one'
    assert api(mp,endpoint,confirmation)==(200,receipt)
    assert api(mp,endpoint,{**confirmation,'retry_id':'changed'})[0]==409
    assert api(mp,endpoint,{'operation':'status','action_id':'s6c3-retry-one'})==(200,receipt)
    deadline=time.monotonic()+30
    while True:
        state=rpc.call('thread/externalInput/status',params)['statuses'][0]
        if state['processing']['state']=='output_observed':break
        assert time.monotonic()<deadline,state
    assert model.calls==before+1
    assert state['receipt']==review['status']['receipt']
    assert g['ledger']()['version']==4
    (g['RUN']/'PASS-RECOVERY.json').write_text(json.dumps({'durable':g['durable'],'native':g['thread'],'message':g['empty'],'review':review,'receipt':receipt,'model_calls_before':before,'model_calls_after':model.calls},indent=2))

tree=ast.parse(Path(__file__).with_name('external_delivery_boundary.py').read_text())
class Inject(ast.NodeTransformer):
    def visit_If(self,node):
        if "'modes'" in ast.unparse(node.test):
            node.body.insert(-1,ast.Expr(value=ast.Call(func=ast.Name(id='recovery',ctx=ast.Load()),args=[ast.Call(func=ast.Name(id='globals',ctx=ast.Load()),args=[],keywords=[])],keywords=[])))
            return node
        return self.generic_visit(node)
tree=ast.fix_missing_locations(Inject().visit(tree))
exec(compile(tree,'S6c2-private-modes-with-root-recovery','exec'),globals())
