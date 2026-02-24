#!/bin/bash

# restore_defaults.sh - Reverts kernel and iptables changes made by run_optimized.sh

set -e

echo "--- Reverting Kernel Optimizations ---"

# Restore standard buffer sizes (conservative defaults)
sudo sysctl -w net.core.rmem_max=212992
sudo sysctl -w net.core.wmem_max=212992
sudo sysctl -w net.core.rmem_default=212992
sudo sysctl -w net.core.wmem_default=212992

# Restore standard backlog and somaxconn
sudo sysctl -w net.core.netdev_max_backlog=1000
sudo sysctl -w net.core.somaxconn=4096

# Restore standard conntrack limits
sudo sysctl -w net.netfilter.nf_conntrack_max=262144
sudo sysctl -w net.netfilter.nf_conntrack_buckets=65536
sudo sysctl -w net.netfilter.nf_conntrack_udp_timeout=30
sudo sysctl -w net.netfilter.nf_conntrack_udp_timeout_stream=120

# Restore standard UDP memory (usually system calculated, 4GB is a safe high default)
sudo sysctl -w net.ipv4.udp_mem="381183 508246 762366"

# Restore port range
sudo sysctl -w net.ipv4.ip_local_port_range="32768 60999"

# Restore VMA limit
sudo sysctl -w vm.max_map_count=65530

echo "--- Cleaning up iptables NOTRACK rules ---"
# Remove the NOTRACK rules (using -D to delete)
sudo iptables -t raw -D PREROUTING -s 10.5.0.0/16 -j NOTRACK 2>/dev/null || true
sudo iptables -t raw -D PREROUTING -d 10.5.0.0/16 -j NOTRACK 2>/dev/null || true
sudo iptables -t raw -D OUTPUT -s 10.5.0.0/16 -j NOTRACK 2>/dev/null || true
sudo iptables -t raw -D OUTPUT -d 10.5.0.0/16 -j NOTRACK 2>/dev/null || true

# Remove the FORWARD rules
sudo iptables -D FORWARD -s 10.5.0.0/16 -j ACCEPT 2>/dev/null || true
sudo iptables -D FORWARD -d 10.5.0.0/16 -j ACCEPT 2>/dev/null || true

echo "--- System Restored to Safe Defaults ---"
echo "Note: Some sysctl values (like udp_mem) were set to generic defaults. For a perfect restore, a reboot is recommended."
