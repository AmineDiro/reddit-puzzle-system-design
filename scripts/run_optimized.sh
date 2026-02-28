#!/bin/bash

# run_optimized.sh - Tuned Launcher for High-Concurrency UDP/QUIC
# This script applies kernel optimizations and runs the server/loadgen with max capacity.

set -e

# 1. Tuning OS Limits (Requires sudo for sysctl)
echo "--- Applying Kernel Optimizations ---"

# Increase file descriptor limits
ulimit -n 1048576

# UDP Receive/Send buffers (64MB for 600k connections)
# This prevents packet loss during handshake bursts
sudo sysctl -w net.core.rmem_max=67108864
sudo sysctl -w net.core.wmem_max=67108864
sudo sysctl -w net.core.rmem_default=33554432
sudo sysctl -w net.core.wmem_default=33554432

# Increase the number of packets the kernel can queue
sudo sysctl -w net.core.netdev_max_backlog=100000
sudo sysctl -w net.core.somaxconn=65535

# Increase connection tracking limits even if we use NOTRACK (as a safety for other traffic)
sudo sysctl -w net.netfilter.nf_conntrack_max=2097152
sudo sysctl -w net.netfilter.nf_conntrack_buckets=524288
# Reduce UDP timeouts to clear stale entries faster
sudo sysctl -w net.netfilter.nf_conntrack_udp_timeout=10
sudo sysctl -w net.netfilter.nf_conntrack_udp_timeout_stream=30

# Global UDP memory limits (pages: low/pressure/max)
# Increased to 32GB max for 600k connections
sudo sysctl -w net.ipv4.udp_mem="2097152 8388608 16777216"

# Per-socket UDP buffer minimums
sudo sysctl -w net.ipv4.udp_rmem_min=16384
sudo sysctl -w net.ipv4.udp_wmem_min=16384

# Allow more local ports
sudo sysctl -w net.ipv4.ip_local_port_range="1024 65535"

# io_uring VMA headroom
sudo sysctl -w vm.max_map_count=2097152

echo "--- Disabling nf_conntrack for Canvas Subnet ---"
# This is the "Magic" part: bypass the conntrack state machine for the bridge network
# This prevents the 'cliff' at 60k connections.
sudo iptables -t raw -C PREROUTING -s 10.5.0.0/16 -j NOTRACK 2>/dev/null || sudo iptables -t raw -A PREROUTING -s 10.5.0.0/16 -j NOTRACK
sudo iptables -t raw -C PREROUTING -d 10.5.0.0/16 -j NOTRACK 2>/dev/null || sudo iptables -t raw -A PREROUTING -d 10.5.0.0/16 -j NOTRACK
sudo iptables -t raw -C OUTPUT -s 10.5.0.0/16 -j NOTRACK 2>/dev/null || sudo iptables -t raw -A OUTPUT -s 10.5.0.0/16 -j NOTRACK
sudo iptables -t raw -C OUTPUT -d 10.5.0.0/16 -j NOTRACK 2>/dev/null || sudo iptables -t raw -A OUTPUT -d 10.5.0.0/16 -j NOTRACK

# Ensure the bridge doesn't drop untracked packets in FORWARD
sudo iptables -I FORWARD -s 10.5.0.0/16 -j ACCEPT 2>/dev/null || true
sudo iptables -I FORWARD -d 10.5.0.0/16 -j ACCEPT 2>/dev/null || true

echo "--- OS Tuned Successfully ---"


# 2. Determine Core Count
CORES=$(nproc)
WORKERS=$((CORES - 1))

if [ "$1" == "server" ]; then
    echo "Starting Server with $WORKERS workers (1 Master + $WORKERS Worker threads)..."
    # Ensure release mode for performance!
    cargo run --release -p server -- --workers $WORKERS
elif [ "$1" == "client" ]; then
    TARGET=${2:-"http://127.0.0.1:4433"}
    CLIENTS_PER_PROC=${3:-15000}
    PROCS=${4:-$CORES}
    MAX_JITTER=${5:-5000}
    MIN_WAIT=${6:-500}
    MAX_WAIT=${7:-1500}
    
    echo "Starting $PROCS client processes, each with $CLIENTS_PER_PROC clients ($((PROCS * CLIENTS_PER_PROC)) total)..."
    echo "Config: jitter=${MAX_JITTER}ms, wait=${MIN_WAIT}-${MAX_WAIT}ms"
    
    for i in $(seq 1 $PROCS); do
        ID="worker-$i"
        echo "Launching $ID..."
        cargo run --release -p client -- \
            --target "$TARGET" \
            --clients "$CLIENTS_PER_PROC" \
            --id "$ID" \
            --max-conn-jitter "$MAX_JITTER" \
            --min-pixel-wait "$MIN_WAIT" \
            --max-pixel-wait "$MAX_WAIT" > "log-$ID.txt" 2>&1 &
    done
    
    echo "All workers launched. Check log-*.txt for details."
    wait
else
    echo "Usage: ./run_optimized.sh [server|client] [target_url] [clients_per_proc] [num_procs] [jitter_ms] [min_wait_ms] [max_wait_ms]"
    echo "Example Server: ./run_optimized.sh server"
    echo "Example Client: ./run_optimized.sh client http://172.16.1.24:4433 15000 4 5000 500 1500"
fi
