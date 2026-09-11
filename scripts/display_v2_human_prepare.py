"""New pv2 preparation only: exact accepted v2 bytes, no model or Job turn."""
from pathlib import Path

_entry = Path(__file__).with_name('presentation_human_prepare.py')
_text = _entry.read_text()
for _old, _new in [
    ('3d8a73a747cf5b957a7ca0491c28d1517f6d7722', 'cc4a080df1df4433f6fd67fee9c1c4fa4a42baab'),
    ('range(24920,24999)', 'range(24940,24999)'),
    ("cfg['private_job_presentation']={'version':1,'recipients':[durable]}",
     "cfg['private_job_presentation']={'version':2,'recipients':[durable]}"),
]:
    assert _text.count(_old) == 1, _old
    _text = _text.replace(_old, _new)
exec(compile(_text, str(_entry), 'exec'), dict(globals(), __file__=str(_entry)))
