"""Fixed-cohort private materializer. Never reads auth files or writes live state.

capture writes nonsecret, allowlisted configuration/identity evidence. apply is
filesystem preparation only; the separate isolated consumer uses product APIs.
Existing destinations, including interrupted ones, are never reused or removed.
"""
import argparse
import base64
import datetime
import hashlib
import json
import os
from pathlib import Path
import stat
import tomllib
import uuid

SOURCE = Path(__file__).resolve().parents[1]
OWNER = SOURCE.parent
LIVE = Path('/home/senxiu/.cutex')
FROZEN = Path('/mnt/mambo/PersonaProjects/cutex-light-core-r1/artifacts/selected-k-history-r1')
IDS = {'aemeath': 'cd6a39eb-3997-45c6-9824-5113fe36a4b8',
       'octobre': 'b341adf8-7af9-432c-a12b-0e9a674458ed',
       'GLM': '3f38c782-bac6-403d-b11d-c801326e0bb1'}
PREREQUISITE = 'cutex.01a05aca-2487-7472-b950-2b854922938e'
PROFILE_KEYS = set('approvals_reviewer cli_auth_credentials_store model model_reasoning_effort service_tier mcp_servers notice projects shell_environment_policy skills tui model_auto_compact_token_limit model_catalog_json model_provider model_providers plugins memories'.split())
SHARED_KEYS = set('approvals_reviewer model model_reasoning_effort plan_mode_reasoning_effort sandbox_mode service_tier projects shell_environment_policy skills tui'.split())

def digest(data): return hashlib.sha256(data).hexdigest()
def encoded(value): return json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2).encode()
def read(path, limit=32*1024*1024):
    path = Path(path)
    assert path.is_absolute() and path.resolve(strict=True) == path, 'noncanonical source'
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, 'rb') as f:
        before = os.fstat(f.fileno())
        assert stat.S_ISREG(before.st_mode) and before.st_size <= limit, 'source size/type'
        data = f.read(limit+1)
        after = os.fstat(f.fileno())
    assert (before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns,before.st_ctime_ns) == (after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns,after.st_ctime_ns), 'source changed'
    return data

def create(path, data):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as f:
        f.write(data); f.flush(); os.fsync(f.fileno())

def capture():
    selection = json.loads(read(SOURCE/'docs/project-recent14-candidates.json'))
    selected = [r for r in selection['rows'] if r['classification'] == 'selected']
    assert len(selected) == 34
    sessions = json.loads(read(LIVE/'cutex-sessions.json'))['sessions']
    management = json.loads(read(LIVE/'runtime/agent-management/v1/agent-management-v1.json'))
    # Accepted selection assessment fixes the four inherited subjects to aemeath.
    # Do not open the live general config (which may also contain service secrets).
    default = 'aemeath'
    prefixes = {r['native_id']:r for r in json.loads(read(FROZEN/'prefix-manifest.json'))['rows']}
    metadata = {r['native_id']:r for r in json.loads(read(FROZEN/'metadata-results.json'))}
    rows = []
    for frozen in selected:
        r = sessions[frozen['durable_id']]
        a = management['agents'][frozen['durable_id']]
        formal = r.get('formal_agent_name') or a['spec']['name']
        assert r['codex_session_id'] == frozen['native_id'] and formal == frozen['formal_name'], 'identity drift'
        assert r.get('profile') == frozen['profile'], 'profile intent drift'
        assert not r.get('default_cli_args') and r.get('runtime_backend') == 'cute_alden'
        override = management.get('current_project_memberships',{}).get(frozen['durable_id'])
        project = override.get('project_id') if override else a.get('project_id')
        assert project == frozen['project_id'], 'project drift'
        assert not a.get('retired_at') and r.get('lifecycle') != 'retired'
        permission = r['permission_defaults'].removeprefix(':')
        assert permission in ('read-only','danger-full-access') and r['approval_policy'] == 'never'
        assert r.get('sandbox_mode') in (None,permission)
        history = prefixes[frozen['native_id']]
        meta = metadata[frozen['native_id']]
        assert meta['catalog_memory_mode'] == 'enabled'
        rows.append({'durable_id':frozen['durable_id'],'native_id':frozen['native_id'],
                     'formal_name':frozen['formal_name'],'project_id':project,
                     'membership_origin':frozen['membership'],'source_status':frozen['status'],
                     'configured_profile':r.get('profile'),'effective_profile':r.get('profile') or default,
                     'model':r['model_defaults'],'effort':r['reasoning_defaults'],
                     'permission_alias':r['permission_defaults'],'sandbox':permission,'approval':'never',
                     'cwd':r.get('managed_cwd') or r['cwd'],'groups':r.get('agent_groups',[]),
                     'source_backend':'cute_alden','private_target_backend':'host',
                     'history':history,'catalog':meta,'source_revision':r['revision'],
                     'apply_runtime':'offline; no implicit launch'})
    profiles = {}
    for name, ident in IDS.items():
        root = LIVE/'profiles'/ident
        raw = read(root/'config.toml'); config = tomllib.loads(raw.decode())
        assert set(config) <= PROFILE_KEYS, 'unknown profile keys'
        # No credential-bearing headers/env/config permitted in captured projection.
        for provider in config.get('model_providers',{}).values():
            assert set(provider) == {'name','base_url','wire_api','requires_openai_auth','env_key'}
        for key, mcp in config.get('mcp_servers',{}).items():
            assert key == 'cutex_job' and set(mcp) <= {'command','args','env_vars'}
            assert all(not any(w in arg.lower() for w in ('bearer','token=','key=')) for arg in mcp.get('args',[]))
        assets = {}
        if config.get('model_catalog_json'):
            path = Path(config['model_catalog_json']); assert path == root/'models.json'
            asset = read(path)
            assets['models.json'] = {'source':str(path),'sha256':digest(asset),'bytes_b64':base64.b64encode(asset).decode()}
        status = read(root/'custom-status-items.json')
        profiles[name] = {'id':ident,'source':str(root/'config.toml'),'sha256':digest(raw),
                          'toml':raw.decode(),'status':json.loads(status),'status_sha256':digest(status),'assets':assets}
    shared = read(LIVE/'codex-home/config.toml')
    assert set(tomllib.loads(shared.decode())) <= SHARED_KEYS, 'unknown shared keys'
    director = sessions[PREREQUISITE]
    projects = {}
    for project in sorted({r['project_id'] for r in rows}):
        authority = management['projects'][project]
        projects[project] = {'director_id':authority['authorized_director_session'],
                             'source_epoch':authority['authority_epoch'],
                             'private_authority':'fresh simulated reconstruction; not original epoch'}
    assert projects['vce2026']['director_id'] == PREREQUISITE
    return {'version':1,'capture_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'selection_sha256':digest(read(SOURCE/'docs/project-recent14-candidates.json')),
            'selection_reference':selection['reference_utc'],'rows':rows,'profiles':profiles,
            'default_source':'accepted aa782 assessment; live general config not opened',
            'shared':{'toml':shared.decode(),'sha256':digest(shared)},'default_profile':default,
            'projects':projects,'prerequisite':{'durable_id':PREREQUISITE,
             'native_id':director['codex_session_id'],'formal_name':director.get('formal_agent_name') or management['agents'][PREREQUISITE]['spec']['name'],
             'project_id':'vce2026','outside_cohort':True,'never_launch':True},
            'limits':['synthetic accounts only','no live migration','four damaged histories quarantined',
                      'original offline records remain offline; runtime probes use separate stores']}

def private_parent(path):
    path = Path(path)
    assert path.is_absolute() and path.resolve(strict=True) == path, 'parent path/symlink'
    assert path.is_relative_to(OWNER), 'outside owned task root'
    st = path.stat()
    assert st.st_uid == os.getuid() and stat.S_IMODE(st.st_mode) == 0o700, 'parent custody'
    return path

def apply(plan, destination, interrupt_after=None):
    """Create once. All source history reads use held handles, bounded streaming.

    The destination parent fd is held; children are created relative to it.
    A partial directory is evidence and is intentionally not deleted/reused.
    """
    assert plan['version'] == 1 and len(plan['rows']) == 34
    frozen = json.loads(read(SOURCE/'docs/project-recent14-candidates.json'))
    selected = {r['durable_id']:r for r in frozen['rows'] if r['classification']=='selected'}
    assert len({r['durable_id'] for r in plan['rows']}) == 34
    for row in plan['rows']:
        assert str(uuid.UUID(row['native_id'])) == row['native_id']
        assert row['durable_id'] == 'cutex.'+row['native_id']
        original=selected[row['durable_id']]
        for key in ('native_id','formal_name','project_id'):
            assert row[key] == original[key], 'changed frozen identity'
    destination = Path(destination)
    parent = private_parent(destination.parent)
    assert destination.name not in ('','.','..')
    pfd = os.open(parent,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
    try:
        held_parent=os.fstat(pfd)
        assert (held_parent.st_dev,held_parent.st_ino)==(parent.stat().st_dev,parent.stat().st_ino), 'parent changed'
        assert held_parent.st_uid==os.getuid() and stat.S_IMODE(held_parent.st_mode)==0o700
        os.mkdir(destination.name,0o700,dir_fd=pfd)
        fd = os.open(destination.name,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=pfd)
        try:
            # Relative paths anchored to the held new directory, not a re-resolved parent.
            root = Path('/proc/self/fd')/str(fd)
            create(root/'plan.json',encoded(plan))
            (root/'prefixes').mkdir(mode=0o700)
            receipts=[]
            for row in plan['rows']:
                h=row['history']; source=Path(h['snapshot_path'])
                assert source == FROZEN/'prefixes'/(row['native_id']+'.jsonl')
                sf=os.open(source,os.O_RDONLY|os.O_NOFOLLOW)
                with os.fdopen(sf,'rb') as src:
                    before=os.fstat(src.fileno())
                    assert stat.S_ISREG(before.st_mode) and before.st_size==h['complete_prefix_bytes']
                    target=root/'prefixes'/(row['native_id']+'.jsonl')
                    out=os.open(target,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
                    sha=hashlib.sha256();count=0
                    with os.fdopen(out,'wb') as dst:
                        for chunk in iter(lambda:src.read(1024*1024),b''):
                            sha.update(chunk);dst.write(chunk);count+=len(chunk)
                        dst.flush();os.fsync(dst.fileno())
                    after=os.fstat(src.fileno())
                    assert (before.st_dev,before.st_ino,before.st_size,before.st_mtime_ns,before.st_ctime_ns)==(after.st_dev,after.st_ino,after.st_size,after.st_mtime_ns,after.st_ctime_ns)
                    assert sha.hexdigest()==h['sha256'] and count==h['complete_prefix_bytes'],'frozen prefix mismatch'
                    assert target.stat().st_ino!=before.st_ino or target.stat().st_dev!=before.st_dev
                receipts.append({'native_id':row['native_id'],'bytes':count,'sha256':sha.hexdigest()})
                if interrupt_after is not None and len(receipts)==interrupt_after:
                    raise RuntimeError('test interruption; retained partial destination')
            held=os.fstat(fd);visible=os.stat(destination.name,dir_fd=pfd,follow_symlinks=False)
            assert (held.st_dev,held.st_ino)==(visible.st_dev,visible.st_ino), 'destination replaced'
            assert (held_parent.st_dev,held_parent.st_ino)==(parent.stat().st_dev,parent.stat().st_ino), 'parent replaced'
            create(root/'prepared.json',encoded({'plan_sha256':digest(encoded(plan)),'histories':receipts,'runtime_launches':0}))
        finally:os.close(fd)
    finally:os.close(pfd)

if __name__ == '__main__':
    parser=argparse.ArgumentParser()
    sub=parser.add_subparsers(dest='op',required=True)
    p=sub.add_parser('capture');p.add_argument('output',type=Path)
    p=sub.add_parser('apply');p.add_argument('plan',type=Path);p.add_argument('destination',type=Path)
    p=sub.add_parser('plan');p.add_argument('snapshot',type=Path)
    args=parser.parse_args()
    if args.op=='capture':
        private_parent(args.output.parent);create(args.output,encoded(capture()))
        print('Fixed34 nonsecret plan captured; no authentication files read.')
    elif args.op=='plan':
        snapshot=json.loads(read(args.snapshot))
        keys=('durable_id','native_id','formal_name','project_id','configured_profile','effective_profile',
              'model','effort','sandbox','approval','source_status','source_backend','private_target_backend','apply_runtime')
        print(json.dumps({'plan_sha256':digest(encoded(snapshot)),
                          'subjects':[{k:r[k] for k in keys} for r in snapshot['rows']],
                          'outside_cohort_prerequisite':snapshot['prerequisite'],
                          'launch_authorized':False},ensure_ascii=False,sort_keys=True,indent=2))
    else:
        apply(json.loads(read(args.plan)),args.destination)
        print('Fresh private histories prepared; no runtime launched. Existing destinations always refuse.')
