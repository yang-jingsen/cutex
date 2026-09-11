"""Read-only independent catalog/prefix/receipt oracle after a private probe.

Only metadata is retained. Does not start a native process, repair an index,
read authentication, or export history bodies.
"""
import hashlib,json,os,sqlite3,sys
from pathlib import Path

owner=Path(__file__).resolve().parents[2]
root=Path(sys.argv[1]).resolve(strict=True)
assert root.is_relative_to(owner) and root.name.startswith('history-r3-')
plan=json.loads((owner/'rehearsal-r2-db5WwG/plan.json').read_text())
home=root/'p0/.cutex/codex-home'
store=json.loads((root/'p0/.cutex/cutex-sessions.json').read_text())
assert len(store['sessions'])==1
record=next(iter(store['sessions'].values()))
row=next(r for r in plan['rows'] if r['durable_id']==record['cutex_session_id'])
receipt=next(v['receipt'] for v in store['explicit_launch_receipts'].values() if v['kind']=='runtime')
assert receipt['stage'] in ('spawned','ready') and receipt['expected_generation']==1
db=sqlite3.connect('file:'+str(home/'state_5.sqlite')+'?mode=ro',uri=True)
db.execute('pragma query_only=on')
columns=('id','memory_mode','history_mode','model','reasoning_effort','sandbox_policy','approval_mode','cwd','model_provider')
values=db.execute('select '+','.join(columns)+' from threads where id=?',(row['native_id'],)).fetchone();db.close()
catalog=dict(zip(columns,values,strict=True))
assert catalog['id']==row['native_id'] and catalog['memory_mode']=='enabled'
assert catalog['history_mode']==row['history']['mode']
assert catalog['model']==row['model'] and catalog['reasoning_effort']==row['effort']
assert catalog['approval_mode']==row['approval'] and catalog['cwd']==row['cwd']
assert catalog['model_provider']==('GLM' if row['effective_profile']=='GLM' else 'openai')
policy=json.loads(catalog['sandbox_policy'])
if row['sandbox']=='read-only':
    assert policy['type']=='managed' and policy['network']=='restricted'
    assert policy['file_system']=={'type':'restricted','entries':[{'path':{'type':'special','value':{'kind':'root'}},'access':'read'}]}
else:
    assert row['sandbox']=='danger-full-access'
    # Native protocol/models.rs PermissionProfile::Disabled maps exactly to
    # SandboxPolicy::DangerFullAccess; this is its persisted tagged form.
    assert policy=={'type':'disabled'}
path=home/'sessions'/Path(row['history']['source_path']).name
sha=hashlib.sha256()
with path.open('rb') as f:
    for chunk in iter(lambda:f.read(1048576),b''):sha.update(chunk)
assert sha.hexdigest()==row['history']['sha256']
assert path.stat().st_size==row['history']['complete_prefix_bytes']
assert path.stat().st_nlink==1
pty_path=root/('pty-'+row['durable_id']+'.json')
pty=json.loads(pty_path.read_text()) if pty_path.exists() else None
result={'durable_id':row['durable_id'],'native_id':row['native_id'],'catalog':catalog,
        'profile':row['effective_profile'],'inherited':row['configured_profile'] is None,
        'generation':1,'runtime_stage':receipt['stage'],'prefix_sha256':sha.hexdigest(),'appended_bytes':0,
        'pty_normal_exit':(pty['exit_code']==0 and pty['terminal_restored'] and not pty['forced_exit']) if pty else None,
        'pty_exit_code':pty['exit_code'] if pty else None,'pty_restored':pty['terminal_restored'] if pty else None,
        'scope':'synthetic private account; metadata/custody proof, not provider acceptance'}
fd=os.open(root/'native-oracle.json',os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
with os.fdopen(fd,'w') as out:json.dump(result,out,indent=2)
print(json.dumps({'name':row['formal_name'],'model':catalog['model'],'effort':catalog['reasoning_effort'],
                  'history':catalog['history_mode'],'sandbox':row['sandbox'],'runtime_stage':receipt['stage'],
                  'prefix_equal':True,'pty_normal_exit':result['pty_normal_exit']}))
