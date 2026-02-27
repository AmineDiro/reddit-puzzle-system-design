# High-End Benchmarking Guide for Bare-Metal Canvas Server

A kernel-engineer-level guide to pushing this QUIC/UDP canvas server to its absolute limits.

---

## 1. Provider & Hardware Selection

### Recommended: Hetzner Dedicated (Best Price/Performance)

| Role | Machine | Spec | Why | Monthly Cost |
|------|---------|------|-----|-------------|
| **Server** | [AX162-R](https://www.hetzner.com/dedicated-rootserver/ax162-r/) | AMD EPYC 9454P (48C/96T), 256GB DDR5 ECC, 2×NVMe | Single-socket EPYC = one NUMA domain, no cross-socket penalties. 48 physical cores means 47 workers + 1 master. Huge L3 cache (256MB) keeps `quiche` connection state hot. | ~€269 |
| **Client** | [AX52](https://www.hetzner.com/dedicated-rootserver/ax52/) ×2 | AMD Ryzen 7 7700 (8C/16T), 64GB DDR5, 1Gbit | Each client machine runs 8-16 loadgen processes. Two machines on the same Hetzner VLAN gives ~1Gbit internal bandwidth with <0.5ms RTT. | ~€69 each |

> [!TIP]
> **Why Hetzner over AWS/GCP?** Dedicated hardware means no hypervisor overhead, no noisy neighbors, no virtualized NICs. You get raw access to `io_uring`, `SO_REUSEPORT`, and kernel tuning. AWS Nitro cards add ~5μs per packet — that adds up at 1M+ msg/s. Hetzner gives you bare metal at 1/10th the cost.

### Alternative Providers

| Provider | Option | Notes |
|----------|--------|-------|
| **OVH** | [Advance-2](https://www.ovhcloud.com/en/bare-metal/) (EPYC 7543P, 128GB) | Good alternative, similar pricing. Internal VLAN support. |
| **Vultr** | [Bare Metal](https://www.vultr.com/products/bare-metal/) (EPYC 7443P, 128GB) | Hourly billing if you just want a weekend test. |
| **AWS** | `c7gn.16xlarge` (Graviton) or `c6in.32xlarge` | Only if you must. Use Placement Groups + ENA Express for lowest latency. No `io_uring` SQPOLL on Nitro. |

> [!IMPORTANT]
> **Network topology matters more than raw CPU.** Put the server and all client machines in the **same datacenter** and on the **same VLAN/private network**. You're testing server throughput, not WAN latency. Cross-datacenter adds 1-10ms RTT which completely changes connection behavior and hides real bottlenecks.

---

## 2. Ideal Test Topology

```
┌──────────────────────────────────────────────────────────────────┐
│  Hetzner VLAN (10Gbit internal, same DC)                         │
│                                                                  │
│  ┌─────────────────────┐       ┌───────────────────────────────┐ │
│  │  CLIENT-1 (AX52)    │       │  SERVER (AX162-R)             │ │
│  │  8C/16T, 64GB       │       │  48C/96T, 256GB               │ │
│  │                     │       │                               │ │
│  │  16× loadgen procs  │──────▶│  1 Master + 47 Workers        │ │
│  │  15000 clients each │       │  SO_REUSEPORT on :4433        │ │
│  │  = 240,000 conns    │       │  io_uring per worker          │ │
│  └─────────────────────┘       │                               │ │
│                                │  Target:                      │ │
│  ┌─────────────────────┐       │   500k+ connections           │ │
│  │  CLIENT-2 (AX52)    │       │   1M+ messages/sec            │ │
│  │  8C/16T, 64GB       │       │                               │ │
│  │                     │       └───────────────────────────────┘ │
│  │  16× loadgen procs  │──────▶                                  │
│  │  15000 clients each │                                         │
│  │  = 240,000 conns    │                                         │
│  └─────────────────────┘                                         │
│                                                                  │
│  Total: 480,000 connections, ~1-2M msg/s                         │
└──────────────────────────────────────────────────────────────────┘
```

> [!CAUTION]
> **Do NOT run server and clients on the same machine.** You'll contend on the loopback stack, kernel socket buffers, and CPU. It makes the benchmark meaningless. Even Docker bridge networking adds overhead vs. real network I/O.

---

## 3. Server Setup (AX162-R)

### 3.1 OS Setup

```bash
# Ubuntu 24.04 LTS recommended (kernel 6.8+ with io_uring improvements)
# Or Debian 12 with backported kernel

# Verify kernel version (need 6.1+ for multishot recvmsg)
uname -r

# Verify io_uring support
cat /proc/config.gz | gunzip | grep IO_URING
# Should show CONFIG_IO_URING=y
```

### 3.2 Kernel Tuning Script (run on server host, NOT in Docker)

```bash
#!/bin/bash
# tune_server.sh — Run as root BEFORE starting the server

set -e
echo "=== Kernel Tuning for Max Throughput ==="

# ── File Descriptors ──
ulimit -n 4194304
echo "4194304" > /proc/sys/fs/file-max
echo "4194304" > /proc/sys/fs/nr_open

# ── UDP/Socket Buffers (128MB — the big lever) ──
sysctl -w net.core.rmem_max=134217728         # 128MB
sysctl -w net.core.wmem_max=134217728         # 128MB
sysctl -w net.core.rmem_default=67108864      # 64MB default per socket
sysctl -w net.core.wmem_default=67108864
sysctl -w net.ipv4.udp_rmem_min=65536
sysctl -w net.ipv4.udp_wmem_min=65536

# ── UDP Global Memory (pages) ──
# 64GB max for 500k+ connections
sysctl -w net.ipv4.udp_mem="8388608 16777216 33554432"

# ── Network Backlog ──
sysctl -w net.core.netdev_max_backlog=300000   # Queue depth before kernel drops
sysctl -w net.core.somaxconn=65535
sysctl -w net.core.optmem_max=2048000          # Ancillary data buffer

# ── Conntrack: DISABLE for test traffic ──
# This is the #1 silent killer above 60k connections
sysctl -w net.netfilter.nf_conntrack_max=2097152
iptables -t raw -A PREROUTING -p udp --dport 4433 -j NOTRACK
iptables -t raw -A OUTPUT -p udp --sport 4433 -j NOTRACK

# ── Port Range ──
sysctl -w net.ipv4.ip_local_port_range="1024 65535"

# ── Memory-mapped regions (io_uring needs many) ──
sysctl -w vm.max_map_count=16777216

# ── MEMLOCK for io_uring buffers ──
ulimit -l unlimited

# ── Disable ASLR (marginal but measurable at this scale) ──
echo 0 > /proc/sys/kernel/randomize_va_space

# ── CPU frequency: lock to max ──
for cpu in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
    echo "performance" > "$cpu" 2>/dev/null || true
done

# ── Disable irqbalance (we'll pin IRQs manually) ──
systemctl stop irqbalance 2>/dev/null || true

echo "=== Tuning Complete ==="
```

### 3.3 NIC IRQ Affinity (Critical for io_uring throughput)

```bash
#!/bin/bash
# pin_irqs.sh — Pin NIC interrupts to specific cores
# This ensures network interrupts don't preempt your io_uring workers

NIC="eth0"  # Change to your actual NIC (check `ip link`)
NUM_QUEUES=$(ls /sys/class/net/$NIC/queues/rx-* -d 2>/dev/null | wc -l)

echo "NIC $NIC has $NUM_QUEUES RX queues"

# Pin each NIC RX queue IRQ to a dedicated core
# Leave core 0 for the master thread
for i in $(seq 0 $((NUM_QUEUES - 1))); do
    IRQ=$(grep "${NIC}-${i}\|${NIC}:.*-${i}" /proc/interrupts | awk '{print $1}' | tr -d ':' | head -1)
    if [ -n "$IRQ" ]; then
        # Pin to cores 48+ (SMT siblings) to keep physical cores free for workers
        CORE=$((48 + (i % 48)))
        echo "$CORE" > /proc/irq/$IRQ/smp_affinity_list 2>/dev/null || true
        echo "  IRQ $IRQ (queue $i) -> core $CORE"
    fi
done
```

### 3.4 `const_settings.rs` Tuning for 48-Core Machine

```diff
- pub const MAX_CONNECTIONS_PER_WORKER: usize = 65_536;
+ pub const MAX_CONNECTIONS_PER_WORKER: usize = 65_536;
  // Keep at 65536 — this is already optimal.
  // 47 workers × 65536 = ~3M theoretical max.
  // Real ceiling will be ~500k-1M (quiche + crypto overhead)

- pub const SOCKET_RECV_BUF_SIZE: usize = 32 * 1024 * 1024;
- pub const SOCKET_SEND_BUF_SIZE: usize = 32 * 1024 * 1024;
+ pub const SOCKET_RECV_BUF_SIZE: usize = 64 * 1024 * 1024;  // 64MB
+ pub const SOCKET_SEND_BUF_SIZE: usize = 64 * 1024 * 1024;  // 64MB
  // With 47 workers each holding 64MB, that's ~6GB in socket buffers alone.
  // The 256GB machine can easily handle this.

- pub const SPSC_CAPACITY: usize = 65_536;
+ pub const SPSC_CAPACITY: usize = 262_144;  // 256k
  // At 1M+ msg/s across 47 workers, each queue sees ~21k msg/s.
  // 256k gives ~12s of buffer at peak — more headroom for master stalls.

- pub const MASTER_BATCH_DRAIN: usize = 4096;
+ pub const MASTER_BATCH_DRAIN: usize = 16384;
  // Master needs to drain faster with 47 workers feeding it.
  // 47 × 16384 = 770k pixels per master iteration at max.

- pub const BROADCAST_INTERVAL_MS: u64 = 100;
+ pub const BROADCAST_INTERVAL_MS: u64 = 50;
  // Broadcast 20× per second instead of 10×.
  // Reduces diff sizes per broadcast, reducing per-connection TX burst.
```

### 3.5 Running the Server (Bare Metal, No Docker)

```bash
# Build with native CPU optimizations
RUSTFLAGS="-C target-cpu=native -C opt-level=3 -C lto=fat -C codegen-units=1" \
  cargo build --release -p server

# Run with 47 workers (reserve core 0 for master)
sudo ./tune_server.sh
sudo ./target/release/server -w 47
```

> [!WARNING]
> **Do NOT use Docker for the server in the final benchmark.** Docker's bridge networking adds ~10-20μs per packet from veth pair + netfilter traversal. The `io_uring` SQEs go through the container's network namespace, which adds overhead. Run native on the host.

---

## 4. Client Setup (AX52 Machines)

### 4.1 Kernel Tuning (run on each client machine)

```bash
#!/bin/bash
# tune_client.sh

# Clients need lots of ephemeral ports and UDP buffers
sysctl -w net.ipv4.ip_local_port_range="1024 65535"
sysctl -w net.core.rmem_max=67108864
sysctl -w net.core.wmem_max=67108864
sysctl -w net.ipv4.udp_mem="4194304 8388608 16777216"
sysctl -w fs.file-max=4194304
ulimit -n 1048576

# Disable conntrack on client side too
iptables -t raw -A PREROUTING -p udp --sport 4433 -j NOTRACK
iptables -t raw -A OUTPUT -p udp --dport 4433 -j NOTRACK
```

### 4.2 Build & Run Loadgen

```bash
# Build on each client machine (or cross-compile and scp)
RUSTFLAGS="-C target-cpu=native" cargo build --release -p client

# Run 16 loadgen processes, each with 15,000 clients = 240,000 per machine
SERVER_IP="10.0.0.10"  # Private VLAN IP of server

for i in $(seq 1 16); do
  ./target/release/client \
    --target "https://${SERVER_IP}:4433" \
    --clients 15000 \
    --id "machine1-worker-${i}" \
    --max-conn-jitter 60000 \
    --min-pixel-wait 20 \
    --max-pixel-wait 50 \
    > "log-worker-${i}.txt" 2>&1 &
done

echo "Launched 240,000 clients. Logs in log-worker-*.txt"
wait
```

> [!NOTE]
> **Why 15,000 clients per loadgen process?** Each `quinn` connection holds ~40KB of state (crypto contexts, congestion control, buffers). 15,000 × 40KB = 600MB per process. Each AX52 has 64GB, so 16 processes × 600MB = ~10GB — leaving plenty of headroom. Going above 20k per process risks tokio scheduler contention.

### 4.3 Connection Ramp Strategy

```
Timeline for 480,000 total connections:

  0s-60s:    Jittered ramp — 480k connections establish over 60 seconds
             (max-conn-jitter=60000 spreads TLS handshakes)
  
  60s-120s:  Warmup — all connections active, server caches warm up,
             quiche connection state stabilizes
  
  120s-600s: Steady state measurement window — collect metrics here
```

---

## 5. What to Measure & How

### 5.1 Server-Side Metrics

```bash
# Terminal 1: Watch CPU per-core utilization
# This shows if one worker is bottlenecked while others are idle
htop  # Press F2 → Display → Detailed CPU time → Enable

# Terminal 2: Watch network stats
watch -n1 'cat /proc/net/snmp | grep Udp'
# Key fields:
#   InDatagrams  — total UDP packets received by kernel
#   InErrors     — packets dropped (buffer overflow)
#   RcvbufErrors — dropped due to socket buffer full (THE critical metric)
#   SndBufErrors — dropped on send side

# Terminal 3: Watch socket buffer pressure
ss -u -a -e | head -20
# Shows Recv-Q and Send-Q depths per socket

# Terminal 4: io_uring stats (if kernel has ftrace enabled)
cat /proc/$(pgrep server)/fdinfo/* | grep -A5 IoUring 2>/dev/null

# Terminal 5: Memory
watch -n5 'ps -o pid,rss,vsz,comm -p $(pgrep server)'
```

### 5.2 Key Bottleneck Indicators

| Symptom | Cause | Fix |
|---------|-------|-----|
| `RcvbufErrors` increasing | Kernel socket buffer overflow | Increase `SOCKET_RECV_BUF_SIZE` and `net.core.rmem_max` |
| One CPU core at 100%, others idle | Uneven `SO_REUSEPORT` distribution | Add more client source ports (`num_endpoints` in client) |
| `conntrack: table full` in dmesg | Conntrack not disabled properly | Verify `NOTRACK` iptables rules |
| Server RSS growing continuously | Connection state leak in `quiche` | Check `cleanup_connections` is running; verify timing wheel eviction |
| Client `failed` count climbing | Server at `MAX_CONNECTIONS_PER_WORKER` | Increase workers or reduce clients per machine |
| TX throughput plateaus | `io_uring` SQ full, causing extra `submit()` syscalls | Increase `IO_URING_SQ_DEPTH` |

### 5.3 Automated Monitoring Script

```bash
#!/bin/bash
# monitor.sh — Run alongside the server

OUTPUT="server_metrics_$(date +%s).csv"
echo "timestamp,udp_in,udp_out,udp_in_err,rcvbuf_err,sndbuf_err,rss_mb,connections" > "$OUTPUT"

while true; do
  TS=$(date +%s)
  
  # UDP counters
  UDP_LINE=$(cat /proc/net/snmp | grep '^Udp:' | tail -1)
  IN=$(echo "$UDP_LINE" | awk '{print $2}')
  OUT=$(echo "$UDP_LINE" | awk '{print $5}')
  IN_ERR=$(echo "$UDP_LINE" | awk '{print $4}')
  RCVBUF=$(echo "$UDP_LINE" | awk '{print $6}')
  SNDBUF=$(echo "$UDP_LINE" | awk '{print $7}')
  
  # Memory
  RSS=$(ps -o rss= -p $(pgrep -x server) 2>/dev/null | awk '{sum+=$1} END {printf "%.0f", sum/1024}')
  
  echo "$TS,$IN,$OUT,$IN_ERR,$RCVBUF,$SNDBUF,$RSS" >> "$OUTPUT"
  sleep 1
done
```

---

## 6. Advanced Tuning Checklist

### 6.1 NUMA Awareness (EPYC Specific)

```bash
# Check NUMA topology
numactl --hardware
# EPYC 9454P is single-socket so all cores are NUMA node 0.
# If you ever move to dual-socket, pin server to one NUMA node:
# numactl --cpunodebind=0 --membind=0 ./target/release/server -w 23

# Verify all memory is local
numastat -p $(pgrep server)
# "other_node" should be 0 or near-zero
```

### 6.2 Huge Pages (Reduces TLB misses for large connection tables)

```bash
# Allocate 4096 × 2MB huge pages = 8GB
echo 4096 > /proc/sys/vm/nr_hugepages

# Verify
grep Huge /proc/meminfo

# Enable transparent huge pages for the process
echo "always" > /sys/kernel/mm/transparent_hugepage/enabled

# Run with explicit huge page support via jemalloc:
# Add to Cargo.toml: tikv-jemallocator = { version = "0.5", features = ["unprefixed_malloc_on_default"] }
# Add to main.rs:
# #[global_allocator]
# static GLOBAL: tikv_jemalloc::Jemalloc = tikv_jemalloc::Jemalloc;
# Environment: MALLOC_CONF="thp:always,oversize_threshold:2097152"
```

### 6.3 RFS (Receive Flow Steering)

```bash
# Steer packets to the same CPU that's reading the socket
# This avoids inter-core cache bouncing on the skb
echo 65536 > /proc/sys/net/core/rps_sock_flow_entries

for rxq in /sys/class/net/eth0/queues/rx-*/rps_flow_cnt; do
    echo 4096 > "$rxq"
done
```

### 6.4 BPF Socket Dispatch (Nuclear Option)

If `SO_REUSEPORT` distribution is uneven, attach a BPF program:

```bash
# This requires writing a small eBPF program that hashes the 4-tuple
# and dispatches to a specific worker socket. Gives perfect distribution.
# Only worth it if you see >2× imbalance between workers in htop.
```

---

## 7. Expected Results at Scale

Based on your architecture (io_uring + quiche + SO_REUSEPORT):

| Metric | Conservative | Aggressive | Theoretical Max |
|--------|-------------|------------|-----------------|
| **Connections** | 200k | 500k | ~3M (47 × 65536) |
| **RX Messages/sec** | 400k | 1.2M | ~3M |
| **TX Broadcasts/sec** | 10/s (100ms) | 20/s (50ms) | 100/s (10ms) |
| **Server RSS** | 8GB | 20GB | ~100GB |
| **CPU Utilization** | 30% | 70% | 95% |

> [!NOTE]
> The real ceiling is almost always **quiche's per-connection crypto overhead** (AEAD decrypt on every incoming packet, AEAD encrypt on every outgoing datagram). At 1M msg/s across 47 workers, each worker processes ~21k packets/sec — each requiring an AES-128-GCM decrypt. On Zen 4 with VAES, that's about 50% of one core.

---

## 8. Quick-Start Checklist

- [ ] Provision server (AX162-R) + 2 client machines (AX52) in **same Hetzner DC**
- [ ] Set up private VLAN between all 3 machines
- [ ] Run `tune_server.sh` on server
- [ ] Run `tune_client.sh` on both client machines
- [ ] Run `pin_irqs.sh` on server
- [ ] Bump `SOCKET_RECV_BUF_SIZE` to 64MB in `const_settings.rs`
- [ ] Bump `SPSC_CAPACITY` to 262144
- [ ] Build server with `RUSTFLAGS="-C target-cpu=native -C lto=fat"`
- [ ] Build client with `RUSTFLAGS="-C target-cpu=native"`
- [ ] Start server: `sudo ./target/release/server -w 47`
- [ ] Start clients: 16 loadgen procs per machine, 15k clients each
- [ ] Monitor `RcvbufErrors` — if increasing, your buffers are too small
- [ ] Wait 2 minutes for ramp + warmup, then measure for 8 minutes
- [ ] Collect CSVs from loadgen, plot with `visualize_load.py`
