"""Fixed read-only native catalog projection; no history bodies or auth access.

Invoked only by the Linux Human maintenance boundary, with an isolated Python
interpreter and bounded stdin. No arbitrary SQL, imports, script paths or hooks.
The Rust caller fences the exact database/WAL identities before and after.
"""
import json
import sqlite3
import sys
from urllib.parse import quote

try:
    request = json.loads(sys.stdin.buffer.read(8193))
    if set(request) != {'path', 'native_id'}:
        raise ValueError('invalid catalog request')
    # Rust rejects a nonempty WAL first. Immutable mode cannot create/modify a
    # source SHM file or acquire a write read-mark; never ignore a pending WAL.
    connection = sqlite3.connect('file:' + quote(request['path'], safe='/') + '?mode=ro&immutable=1',
                                 uri=True, timeout=0.2)
    connection.execute('PRAGMA query_only=ON')
    connection.execute('PRAGMA trusted_schema=OFF')
    row = connection.execute(
        'SELECT id, rollout_path, memory_mode, history_mode FROM threads WHERE id=?',
        (request['native_id'],)).fetchall()
    connection.close()
    if len(row) != 1:
        raise ValueError('catalog identity missing or ambiguous')
    result = dict(zip(('native_id', 'rollout_path', 'memory_mode', 'history_mode'), row[0]))
    output = json.dumps(result).encode()
    if len(output) > 8192:
        raise ValueError('catalog projection exceeds bound')
    sys.stdout.buffer.write(output)
except Exception as error:
    # Never retain SQL diagnostics, environment or database contents.
    sys.stderr.write('native catalog read failed: ' + type(error).__name__ + '\n')
    sys.exit(1)
