"""Offline proposed fixture-only extractor. Never persists raw error text/headers."""
import json
CODES=set('contextWindowExceeded sessionBudgetExceeded usageLimitExceeded rateLimitExceeded serverOverloaded cyberPolicy misalignmentPolicyViolation httpConnectionFailed responseStreamConnectionFailed internalServerError unauthorized badRequest threadRollbackFailed sandboxError responseStreamDisconnected responseTooManyFailedAttempts activeTurnNotSteerable other'.split())
MARKERS=('function_call_output','call_id','no tool call found','invalid parameter','invalid request','rate limit','connection','timeout','context length','unauthorized','stream disconnected')
def observe(event):
    method=event.get('method');p=event.get('params') or {}
    if method=='error': e=p.get('error');retry=p.get('willRetry')
    elif method=='turn/completed':e=(p.get('turn') or {}).get('error');retry=False
    else:return None
    if not isinstance(e,dict):return {'method':method,'schemaMismatch':True}
    info=e.get('codexErrorInfo'); code=info if isinstance(info,str) else next(iter(info),'unknown') if isinstance(info,dict) else None
    status=None
    if isinstance(info,dict) and code in CODES and isinstance(info[code],dict):
        v=info[code].get('httpStatusCode')
        if isinstance(v,int) and not isinstance(v,bool) and 100<=v<=599:status=v
    # Fixed literal matches only: never copy body, URL, header, or dynamic identifier.
    texts=[e.get('message'),e.get('additionalDetails')]
    joined='\n'.join(t for t in texts if isinstance(t,str)).lower()
    return {'method':method,'willRetry':retry if isinstance(retry,bool) else None,
      'code':code if code in CODES else ('unknown' if code is not None else None),
      'httpStatusCode':status,'messagePresent':isinstance(e.get('message'),str),
      'additionalDetailsPresent':isinstance(e.get('additionalDetails'),str),
      'markers':[m for m in MARKERS if m in joined],
      'misalignmentPresent':isinstance(e.get('misalignment'),dict)}
def should_stop(event, observation):
    """Only the authorized fixture decision; never changes native retry state."""
    if observation is None:return False
    if observation.get('schemaMismatch'):return True
    if observation.get('code')=='badRequest' or 'function_call_output' in observation.get('markers',[]):return True
    return observation.get('willRetry') is not True
if __name__=='__main__':
    tests=[
      ({'method':'error','params':{'error':{'message':'Invalid request function_call_output call_id SECRET Authorization Bearer PRIVATE','codexErrorInfo':'badRequest'},'willRetry':False}},'badRequest',False),
      ({'method':'error','params':{'error':{'message':'connection timeout','codexErrorInfo':{'responseStreamConnectionFailed':{'httpStatusCode':503}},'additionalDetails':'cookie SECRET'},'willRetry':True}},'responseStreamConnectionFailed',True),
      ({'method':'turn/completed','params':{'turn':{'error':{'message':'unknown PRIVATE','codexErrorInfo':None}}}},None,False)]
    results=[]
    for event,code,retry in tests:
        r=observe(event);assert r['code']==code and r['willRetry']==retry
        assert 'SECRET' not in json.dumps(r) and 'PRIVATE' not in json.dumps(r)
        results.append(r)
    assert results[1]['httpStatusCode']==503
    assert observe({'method':'error','params':{'codexErrorInfo':'badRequest'}})['schemaMismatch']
    # Intermediate retry notification must be persisted without triggering
    # teardown. No retry/start/interrupt operation is issued by this decision.
    assert not should_stop(tests[1][0],results[1])
    assert should_stop(tests[0][0],results[0])
    assert should_stop(tests[2][0],results[2])
    print(json.dumps({'synthetic_only':True,'passed':5,'results':results,'intermediateRetryDoesNotCleanup':True},indent=2))
