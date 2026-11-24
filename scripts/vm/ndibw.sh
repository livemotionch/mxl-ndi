#!/usr/bin/env bash
# NDI bytes on the host (loopback included) over N seconds, by nftables
# counters on NDI's ports (TCP/UDP 5959-5999 and 6960-8000; OMT's 6400-6431
# are outside). The NDI SDK has no byte counters. Needs sudo; removes its
# table afterwards.  ndibw.sh [seconds]
set -euo pipefail
secs="${1:-10}"
sudo nft add table inet ndibw
trap 'sudo nft delete table inet ndibw' EXIT
sudo nft add chain inet ndibw out '{ type filter hook output priority 0; policy accept; }'
# Each packet counted once: by its source port, else by its destination port.
sudo nft add rule inet ndibw out th sport '{ 5959-5999, 6960-8000 }' counter
sudo nft add rule inet ndibw out th sport != '{ 5959-5999, 6960-8000 }' th dport '{ 5959-5999, 6960-8000 }' counter
sleep "$secs"
sudo nft list chain inet ndibw out | awk -v s="$secs" '/counter/ { for (i=1;i<=NF;i++) if ($i=="bytes") t+=$(i+1) } END { printf "NDI: %.1f Mbit/s over %d s\n", t*8/s/1e6, s }'
