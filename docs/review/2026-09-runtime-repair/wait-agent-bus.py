#!/usr/bin/python3
"""Complete systemd Bus startup only once its authenticated HTTP service is ready."""
import json,time,urllib.request
from pathlib import Path
config=json.loads((Path.home()/'.cutex/config.json').read_text())
request=urllib.request.Request('http://127.0.0.1:24260/',headers={'Authorization':'Bearer '+config['agent_bus_token']})
for attempt in range(100):
    try:
        with urllib.request.urlopen(request,timeout=.5) as response:
            if response.status==200:
                raise SystemExit(0)
    except (OSError,ValueError):
        pass
    time.sleep(.1)
raise SystemExit('Agent Bus did not become ready during systemd startup')
