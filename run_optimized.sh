#!/bin/bash

# run_optimized.sh - Tuned Launcher for High-Concurrency UDP/QUIC
# This script applies kernel optimizations and runs the server/loadgen with max capacity.

set -e

# 1. Tuning OS Limits (Requires sudo for sysctl)
echo "--- Applying Kernel Optimizations ---"

# Increase file descriptor limits
ulimit -n 1048576

# UDP Receive/Send buffers (32MB)
# This prevents packet loss during handshake bursts
sudo sysctl -w net.core.rmem_max=33554432
sudo sysctl -w net.core.wmem_max=33554432
sudo sysctl -w net.core.rmem_default=33554432
sudo sysctl -w net.core.wmem_default=33554432

# Increase the number of packets the kernel can queue
sudo sysctl -w net.core.netdev_max_backlog=50000

# Increase UDP hash table size for many concurrent connections
# (Calculation: 65536 * number of cores)
sudo sysctl -w net.ipv4.udp_rmem_min=16384
sudo sysctl -w net.ipv4.udp_wmem_min=16384

# Allow more local ports for the client load generator
sudo sysctl -w net.ipv4.ip_local_port_range="1024 65535"

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
    
    echo "Starting $PROCS client processes, each with $CLIENTS_PER_PROC clients ($((PROCS * CLIENTS_PER_PROC)) total)..."
    
    for i in $(seq 1 $PROCS); do
        ID="worker-$i"
        echo "Launching $ID..."
        cargo run --release -p client -- --target "$TARGET" --clients "$CLIENTS_PER_PROC" --id "$ID" > "log-$ID.txt" 2>&1 &
    done
    
    echo "All workers launched. Check log-*.txt for details."
    wait
else
    echo "Usage: ./run_optimized.sh [server|client] [target_url] [clients_per_proc] [num_procs]"
    echo "Example Server: ./run_optimized.sh server"
    echo "Example Client: ./run_optimized.sh client http://172.16.1.24:4433 15000 4"
fi
