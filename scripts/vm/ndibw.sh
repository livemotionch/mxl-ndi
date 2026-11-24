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
sudo nft add rule inet ndibw out udp sport '{ 5959-5999, 6960-8000 }' counter
sudo nft add rule inet ndibw out tcp sport '{ 5959-5999, 6960-8000 }' counter
sudo nft add rule inet ndibw out udp dport '{ 5959-5999, 6960-8000 }' counter
sudo nft add rule inet ndibw out tcp dport '{ 5959-5999, 6960-8000 }' counter
sleep "$secs"
sudo nft list chain inet ndibw out | awk -v s="$secs" '/counter/ { for (i=1;i<=NF;i++) if ($i=="bytes") b=$(i+1); n++; printf "%s %s %s: %.1f Mbit/s\n", $1, $2, $3, b*8/s/1e6; t+=b } END { printf "total (a packet can match a sport and a dport rule): see rows\n" }'
