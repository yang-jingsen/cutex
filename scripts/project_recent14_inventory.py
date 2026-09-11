"""Read-only metadata selection; no content fields retained or emitted."""
import collections,datetime,json,os,sqlite3,stat
from pathlib import Path
from zoneinfo import ZoneInfo

ROOT=Path('/home/senxiu/.cutex')
END=datetime.datetime.fromisoformat('2026-09-11T18:48:15+00:00')
START=END-datetime.timedelta(days=14)
def load(p): return json.loads(p.read_text())
m=load(ROOT/'runtime/agent-management/v1/agent-management-v1.json')
ds=load(ROOT/'cutex-sessions.json'); sessions=ds['sessions']
db=sqlite3.connect('file:'+str(ROOT/'codex-home/state_5.sqlite')+'?mode=ro',uri=True)
db.execute('pragma query_only=ON')
rows=[]; unassigned=0
tasks=load(ROOT/'runtime/task-service/task-worker-actions-v1/task-service/provider-v2/task-service-provider-v2.json')
active_tasks=collections.defaultdict(list)
for a in tasks['assignments'].values():
 if a['state']!='closed':active_tasks[a['assignee_cutex_session']].append(a['assignment_id'])
bindings=collections.Counter(r.get('codex_session_id') for r in sessions.values() if r.get('codex_session_id'))
tools={'function_call','function_call_output','custom_tool_call','custom_tool_call_output','local_shell_call','web_search_call'}
for ident,a in m['agents'].items():
 retired=bool(a.get('retired_at'))
 override=m.get('current_project_memberships',{}).get(ident)
 project=override.get('project_id') if override is not None else a.get('project_id')
 if not project: unassigned+=1;continue
 r=sessions.get(ident,{})
 row={'durable_id':ident,'formal_name':r.get('formal_agent_name') or a['spec']['name'],
      'project_id':project,'project_label':m.get('project_presentations',{}).get(project,{}).get('display_name') or project,
      'membership':'current_override' if override is not None else ('retired_historical_only' if retired else 'managed_project'),
      'native_id':r.get('codex_session_id'),'profile':r.get('profile'),'lifecycle':r.get('lifecycle'),
      'retired':retired or r.get('lifecycle')=='retired','archived':bool(r.get('archived_at') or r.get('archive_state')),
      'director':m.get('projects',{}).get(project,{}).get('authorized_director_session')==ident}
 row['active_assignments']=active_tasks[ident]
 b=r.get('app_server_runtime') or {};pid=b.get('pid');row['runtime_pid_present']=bool(pid and Path('/proc',str(pid),'exe').exists())
 row['status']='retired' if row['retired'] else ('archived' if row['archived'] else ('online-observed' if row['runtime_pid_present'] else 'offline'))
 row['classification']='unknown';row['reason']=None
 try:
  assert r and row['native_id'],'missing durable/native record'
  assert a.get('native_session_id')==row['native_id'],'managed/durable native mismatch'
  assert bindings[row['native_id']]==1,'duplicate durable binding'
  found=db.execute('select rollout_path,source,history_mode from threads where id=?',(row['native_id'],)).fetchall()
  assert len(found)==1,'missing/ambiguous canonical catalog mapping'
  path=Path(found[0][0]);row.update(rollout_path=str(path),catalog_source=found[0][1],history_mode=found[0][2])
  assert path.is_relative_to(ROOT) and path.resolve()==path,'outside owner root or symlink'
  assert path.suffix=='.jsonl','non-JSONL history requires separate reader'
  fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW);initial=os.fstat(fd)
  assert stat.S_ISREG(initial.st_mode),'not regular history'
  last=None;external=None;types=collections.Counter();subtypes=collections.Counter();meta=None;malformed=[]
  with os.fdopen(fd,'rb') as f:
   line_no=0
   while f.tell()<initial.st_size:
    offset=f.tell();line=f.readline(min(initial.st_size-offset,16*1024*1024+1));line_no+=1
    assert len(line)<=16*1024*1024,'oversized record'
    if not line.endswith(b'\n'):break # concurrent incomplete append is not a record
    try:d=json.loads(line)
    except (ValueError,UnicodeError):malformed.append({'line':line_no,'offset':offset});continue
    kind=d.get('type');p=d.get('payload',{});types[kind]+=1
    if not isinstance(p,dict):continue
    sub=p.get('type');subtypes[str(kind)+':'+str(sub)]+=1
    if kind=='session_meta':
     assert meta is None,'multiple session metadata records'
     meta={k:p.get(k) for k in ('id','session_id','parent_thread_id','forked_from_id','history_base','history_mode','cli_version','source')}
    ts=d.get('timestamp')
    try:t=datetime.datetime.fromisoformat(ts.replace('Z','+00:00'))
    except (AttributeError,ValueError):continue
    if t>END:continue
    anchor={'timestamp':ts,'line':line_no,'ordinal':d.get('ordinal'),'offset':offset,'kind':kind,'subtype':sub}
    actual=kind=='response_item' and ((sub=='message' and p.get('role') in ('user','assistant')) or (sub in tools and p.get('name')!='external_event'))
    if actual and (last is None or t>last[0]):last=(t,anchor)
    if kind in ('inter_agent_communication','inter_agent_communication_metadata','external_input') and (external is None or t>external[0]):external=(t,anchor)
   final=os.fstat(f.fileno());assert final.st_ino==initial.st_ino and final.st_size>=initial.st_size,'history replaced/truncated'
  assert meta and meta['id']==row['native_id'],'missing/mismatched session metadata'
  assert not meta.get('history_base') and not meta.get('forked_from_id'),'inherited fork history needs own-record attribution'
  row['malformed_record_anchors']=malformed
  assert not malformed,'malformed history records'
  row.update(metadata=meta,captured_bytes=initial.st_size,record_types=dict(types),payload_types=dict(subtypes),last_actual_record=last[1] if last else None,last_external_record=external[1] if external else None)
  if last:row['local_date']=last[0].astimezone(ZoneInfo('Australia/Sydney')).isoformat()
  row['classification']=('qualifying_retired_or_archived' if row['retired'] or row['archived'] else 'selected') if last and START<=last[0]<=END else 'excluded_no_actual_record_in_window'
 except (AssertionError,OSError,ValueError) as e:row['reason']=str(e)
 rows.append(row)
result={'reference_utc':END.isoformat(),'inclusive_lower_utc':START.isoformat(),'timezone':'Australia/Sydney','management_revision':m.get('store_revision'),'durable_revision':ds.get('storeRevision'),'counts':dict(collections.Counter(x['classification'] for x in rows)),'unassigned_managed_excluded':unassigned,'durable_total':len(sessions),'rows':sorted(rows,key=lambda x:(x['project_id'],x['formal_name']))}
print(json.dumps(result,ensure_ascii=False,indent=2))
