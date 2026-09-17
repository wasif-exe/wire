#!/usr/bin/env bash
set -euo pipefail

IFACE=tap0
HOST_IP=192.168.99.1
NETMASK=24


sudo ip link del "$IFACE" 2>/dev/null || true


sudo ip tuntap add mode tap name "$IFACE" user "$USER"
sudo ip addr add "$HOST_IP/$NETMASK" dev "$IFACE"
sudo ip link set "$IFACE" up


sudo sysctl -w "net.ipv6.conf.$IFACE.disable_ipv6=1" > /dev/null
sudo sysctl -w "net.ipv4.conf.$IFACE.rp_filter=0" > /dev/null
sudo sysctl -w "net.ipv4.conf.all.rp_filter=0" > /dev/null

echo "✅ tap0 initialized: Host IP $HOST_IP. Your userspace stack should use 192.168.99.2"
