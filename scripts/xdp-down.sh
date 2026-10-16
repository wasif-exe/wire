#!/usr/bin/env bash
sudo ip link del veth-host 2>/dev/null || true
echo " veth interface cleaned."
