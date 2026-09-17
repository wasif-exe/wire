#!/usr/bin/env bash
sudo ip link del tap0 2>/dev/null || true
echo "❌ tap0 deleted."
