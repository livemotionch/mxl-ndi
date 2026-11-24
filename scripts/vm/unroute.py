#!/usr/bin/env python3
"""unroute.py "<receiver label>" ...: disable IS-05 receivers (master_enable false), to undo route.py."""
import json, sys, urllib.request

Q = "http://127.0.0.1:3211/x-nmos/query/v1.3"


def get(u):
    return json.load(urllib.request.urlopen(u, timeout=5))


devices = {d["id"]: d for d in get(f"{Q}/devices?paging.limit=1000")}
receivers = {r["label"]: r for r in get(f"{Q}/receivers?paging.limit=1000")}
for label in sys.argv[1:]:
    r = receivers[label]
    hrefs = [c["href"].rstrip("/") for c in devices[r["device_id"]]["controls"] if "/x-nmos/connection/" in c["href"]]
    href = max(hrefs, key=lambda h: tuple(int(x) for x in h.rsplit("/v", 1)[1].split(".")))
    body = json.dumps({"sender_id": None, "master_enable": False, "activation": {"mode": "activate_immediate"}}).encode()
    req = urllib.request.Request(f"{href}/single/receivers/{r['id']}/staged", body, {"content-type": "application/json"}, method="PATCH")
    out = json.load(urllib.request.urlopen(req, timeout=5))
    print(label, "disabled:", out.get("master_enable") is False)
