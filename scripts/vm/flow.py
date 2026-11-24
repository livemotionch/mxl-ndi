#!/usr/bin/env python3
"""flow.py "<sender label>": prints "<domain dir> <flow id>" of an MXL sender
(from its IS-05 active parameters), for mxl-compare."""
import json, os, sys, urllib.request

Q = "http://127.0.0.1:3211/x-nmos/query/v1.3"


def get(u):
    return json.load(urllib.request.urlopen(u, timeout=5))


s = [x for x in get(f"{Q}/senders?paging.limit=1000") if x["label"] == sys.argv[1]][0]
d = get(f"{Q}/devices/{s['device_id']}")
hrefs = [c["href"].rstrip("/") for c in d["controls"] if "/x-nmos/connection/" in c["href"]]
href = max(hrefs, key=lambda h: tuple(int(x) for x in h.rsplit("/v", 1)[1].split(".")))
tp = get(f"{href}/single/senders/{s['id']}/active")["transport_params"][0]
for name in sorted(os.listdir("/Volumes/mxl")):
    try:
        if json.load(open(f"/Volumes/mxl/{name}/domain_def.json")).get("id") == tp["mxl_domain_id"]:
            print(f"/Volumes/mxl/{name}", tp["mxl_flow_id"])
            break
    except (OSError, ValueError):
        pass
else:
    sys.exit(f"domain {tp.get('mxl_domain_id')} not found")
