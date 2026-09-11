"""Worker-observed pv3 preparation; no turn, Job or approval."""
from pathlib import Path
import sys

assert not sys.argv[1:]
path=Path(__file__).with_name('fixed_human_prepare.py')
source=path.read_text()
for old,new in [
 ('cac03d6d1b77e4d3681a2d71b25435a537bc257807c28ea0f5aff491c622b69d','7bc7f3d73a80981b1d77b0af40a685cfebec5258527156a8801e601cbe098dee'),
 ('ca580a783fc1ab34613be4f81ceab96ef4d393a2','f8c33add01bf9ef8cea04f531fa1319090751cb2'),
 ('range(24900,24999)','range(24960,24999)'),
 ("ast.parse(helper.read_text())", "ast.parse(helper.read_text().replace('d98d5e33322c7200eb9b149bc39d99da3bb169c994fe14c7f401d26c06d7b1f2','ba1a8d4f3e0b5f739e666e3f515d40b0c543e9e75953759ff181f1e71b29c521').replace(\"'--completion'\",\"'--completion-v2'\"))"),
]:
 assert source.count(old)==1,old
 source=source.replace(old,new)
at='lines=source.splitlines()'
assert source.count(at)==1
source=source.replace(at,'''source=source.replace('459861225d5bfb73bb4c3896edb489169637424be410be346f955a39596da7e9','c2a54d598c816cfd402829b3855ecbfdd7857c6dbad51d95564f6dcc7fbd1d12')
source=source.replace("    durable = ids[0]", "    durable = ids[0]\\n    cfg=json.loads((CONF/'config.json').read_text())\\n    cfg['private_job_presentation']={'version':2,'recipients':[durable]}\\n    (CONF/'config.json').write_text(json.dumps(cfg))")
'''+at)
exec(compile(source,str(path),'exec'),dict(globals(),__file__=str(path)))
