#!/usr/bin/env bash
set -euo pipefail
HOST_IFACE="veth-host"
WIRE_IFACE="veth-wire"
HOST_IP="192.168.99.1"
WIRE_IP="192.168.99.2"
sudo ip link del "$HOST_IFACE" 2>/dev/null || true
sudo ip link add "$HOST_IFACE" type veth peer name "$WIRE_IFACE"
sudo ip addr add "$HOST_IP/24" dev "$HOST_IFACE"
sudo ip link set "$HOST_IFACE" up
sudo ip link set "$WIRE_IFACE" up
sudo sysctl -w "net.ipv6.conf.$HOST_IFACE.disable_ipv6=1" > /dev/null
sudo sysctl -w "net.ipv4.conf.$HOST_IFACE.rp_filter=0" > /dev/null
clang -O2 -g -target bpf -c wire-xdp/bpf/xdp_prog.c -o wire-xdp/bpf/xdp_prog.o
echo " Interface veth-wire and BPF object ready."
