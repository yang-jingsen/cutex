"""Fixed accepted generated-schema contract, no build/network/dependency."""
import hashlib
import json
from pathlib import Path

path=Path('/mnt/mambo/PersonaProjects/cutex-light-core-r1/artifacts/s6c1-schema-handoff/codex_app_server_protocol.schemas.json')
raw=path.read_bytes()
assert hashlib.sha256(raw).hexdigest()=='00e035e34ac1034ee34473f8f68b7704d6058c5b180ff4f4b6cad9fadab3a86d'
schema=json.loads(raw)['definitions']['v2']
binding={'version','ownerId','threadId','runtimeGeneration'}
shapes={
    'ExternalInputSubmitParams':binding|{'message','semanticSha256'},
    'ExternalInputStatusParams':binding|{'messages'},
    'ExternalInputRetryParams':binding|{'messageId','semanticSha256','expectedAttemptId','retryId'},
    'ExternalInputResponse':binding|{'statuses'},
    'ExternalInputStatusChangedNotification':{'threadId','messageId'},
    'ExternalInputReceipt':{'schema','receiptId','ownerId','threadId','messageId','semanticSha256','responseItemId','turnId','ordinal'},
}
for name,fields in shapes.items():
    shape=schema[name]
    assert set(shape['properties'])==fields, name
    assert set(shape['required'])==fields-({'expectedAttemptId'} if name=='ExternalInputRetryParams' else set()),name
    assert shape['additionalProperties'] is False,name
print('PASS: six exact native request/response/receipt/hint shapes; policy absent from sender RPCs')
