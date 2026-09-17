#!/usr/bin/env bash
# Example nftables rules for transparent-proxy (TPROXY) mode.
# Requires Linux + CAP_NET_ADMIN. Portable proxy mode does NOT need this.
# Topology: client -> netfilter -> TPROXY -> edgewarden -> original destination.
set -euo pipefail

PROXY_PORT="${PROXY_PORT:-18080}"
TABLE="edgewarden"

echo "Flushing/creating nft table 'inet $TABLE'..."
sudo nft flush ruleset 2>/dev/null || true

sudo nft add table inet "$TABLE"
sudo nft add chain inet "$TABLE" prerouting '{ type filter hook prerouting priority mangle; policy accept; }'
sudo nft add chain inet "$TABLE" output '{ type route hook output priority mangle; policy accept; }'

# Divert TCP port 80/443 to the local proxy via TPROXY.
sudo nft add rule inet "$TABLE" prerouting tcp dport '{ 80, 443 }' tproxy to 127.0.0.1:"$PROXY_PORT"
# Locally-generated traffic that should also be proxied:
sudo nft add rule inet "$TABLE" output tcp dport '{ 80, 443 }' mark set 1

# Policy routing for marked packets (run once):
#   sudo ip rule add fwmark 1 lookup 100
#   sudo ip route add local 0.0.0.0/0 dev lo table 100

echo "TPROXY divert to 127.0.0.1:$PROXY_PORT installed."
echo "Run edge-proxy with --transparent (see docs/proxy-datapath.md)."
echo "To clean up: sudo nft flush ruleset"
