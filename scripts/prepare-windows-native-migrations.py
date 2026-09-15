"""Prepare Windows native SQL assets using the official Windows CRLF convention.

Run on the Windows build staging tree before cargo. This changes source line
endings only; it never edits databases or disables SQLx migration validation.
Linux source/builds retain LF. Git/tar transfer must not decide runtime checksums.
"""
import pathlib, sys
root = pathlib.Path(sys.argv[1]) / 'codex-rs' / 'state'
folders = ('migrations', 'logs_migrations', 'queue_migrations', 'goals_migrations', 'memory_migrations', 'thread_history_migrations')
count = 0
for folder in folders:
    files = list((root / folder).glob('*.sql'))
    if not files:
        raise SystemExit(f'Missing migration sources: {folder}')
    for path in files:
        before = path.read_bytes()
        after = before.replace(b'\r\n', b'\n').replace(b'\n', b'\r\n')
        if after != before:
            path.write_bytes(after)
            count += 1
print(f'Prepared {count} Windows migration source files (CRLF).')
