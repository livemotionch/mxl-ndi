#!/usr/bin/env bash
# A/V sync through the bridge: mxl-avsync generator -> NDI Out N -> NDI In N -> analyzer.
#   avs.sh N [seconds]   (generator on WEB 8212, analyzer on 8213; see docs/test-results.md)
set -euo pipefail
n="$1"; secs="${2:-60}"
curl -s -X PUT -H content-type:application/json -d "{\"source\":\"MXL-TEST (NDI Out $n)\"}" localhost:8210/api/inputs/$n >/dev/null
python3 /opt/mxl-test/route.py "AVSync Test Video=NDI Out $n Video" "AVSync Test Audio=NDI Out $n Audio" \
  "NDI In $n Video=AVSync In Video" "NDI In $n Audio=AVSync In Audio" | sed 's/{.*//'
sleep 15
curl -s -X POST localhost:8213/api/v1/statistics/reset >/dev/null
sleep "$secs"
curl -s localhost:8213/api/v1/measurement | python3 -c '
import json,sys; m=json.load(sys.stdin)
print({k: m.get(k) for k in ("av_sync_ms","events","grade","sentence","input_state")})'
curl -s localhost:8213/api/v1/statistics | python3 -c '
import json,sys; s=json.load(sys.stdin)
def walk(d,p=""):
    for k,v in d.items():
        if isinstance(v,dict): walk(v,p+k+".")
        elif isinstance(v,(int,float)) and any(x in k for x in ("min","max","mean","p50","p95","std","count","events")): print(" ", p+k, round(v,3) if isinstance(v,float) else v)
walk(s)' | head -40
curl -s localhost:8210/api/status | python3 -c "
import json,sys; i=json.load(sys.stdin)['inputs'][$n-1]
print({k:i[k] for k in ['state','timing','latency_ms','transit_ms','dropped','repeated','resyncs','audio_resyncs','audio_silence','stale']})"
