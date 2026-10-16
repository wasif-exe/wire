#!/usr/bin/env bash
set -euo pipefail
HOST_IFACE="veth-host"
WIRE_IFACE="veth-wire"
HOST_IP="192.168.99.1"
WIRE_IP="192.168.99.2"
NUM_QUEUES=2
echo " Configuring Multi-Queue AF_XDP Interface with $NUM_QUEUES queues..."
sudo ip link del "$HOST_IFACE" 2>/dev/null || true
sudo ip link add "$HOST_IFACE" numtxqueues "$NUM_QUEUES" numrxqueues "$NUM_QUEUES" \
    type veth peer name "$WIRE_IFACE" numtxqueues "$NUM_QUEUES" numrxqueues "$NUM_QUEUES"
sudo ip addr add "$HOST_IP/24" dev "$HOST_IFACE"
sudo ip link set "$HOST_IFACE" up
sudo ip link set "$WIRE_IFACE" up
sudo ethtool -K "$HOST_IFACE" rx off tx off tso off gso off gro off 2>/dev/null || true
sudo ethtool -K "$WIRE_IFACE" rx off tx off tso off gso off gro off 2>/dev/null || true
sudo sysctl -w "net.ipv6.conf.$HOST_IFACE.disable_ipv6=1" > /dev/null
sudo sysctl -w "net.ipv4.conf.$HOST_IFACE.rp_filter=0" > /dev/null
sudo sysctl -w "net.ipv6.conf.$WIRE_IFACE.disable_ipv6=1" > /dev/null
sudo sysctl -w "net.ipv4.conf.$WIRE_IFACE.rp_filter=0" > /dev/null
echo " Multi-queue interface $WIRE_IFACE ready ($NUM_QUEUES queues)."
