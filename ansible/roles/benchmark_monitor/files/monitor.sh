#!/bin/bash
# monitor.sh — Zero-overhead server metrics collector
# Reads kernel counters from /proc and /sys every second.
# Output: CSV to stdout (redirect to file).
#
# Usage: ./monitor.sh <nic_iface> > server_metrics.csv &
# Stop:  kill %1  (or kill $PID)

set -euo pipefail

NIC="${1:-eth0}"
INTERVAL=1

# ── CSV Header ──────────────────────────────────────────────────────
echo "timestamp,\
cpu_user,cpu_nice,cpu_system,cpu_idle,cpu_iowait,cpu_irq,cpu_softirq,\
udp_in_dgrams,udp_out_dgrams,udp_in_errors,udp_rcvbuf_errors,udp_sndbuf_errors,\
net_rx_bytes,net_tx_bytes,net_rx_packets,net_tx_packets,net_rx_drops,net_tx_drops,\
server_rss_kb,server_vsz_kb,\
ctx_switches,interrupts,\
tcp_mem_pages,udp_mem_pages,\
softnet_processed,softnet_dropped,softnet_time_squeeze"

# ── Previous values for delta calculation ────────────────────────────
prev_cpu_user=0; prev_cpu_nice=0; prev_cpu_system=0; prev_cpu_idle=0
prev_cpu_iowait=0; prev_cpu_irq=0; prev_cpu_softirq=0
prev_udp_in=0; prev_udp_out=0; prev_udp_in_err=0; prev_udp_rcvbuf=0; prev_udp_sndbuf=0
prev_rx_bytes=0; prev_tx_bytes=0; prev_rx_pkts=0; prev_tx_pkts=0
prev_rx_drops=0; prev_tx_drops=0
prev_ctxt=0; prev_intr=0
prev_softnet_proc=0; prev_softnet_drop=0; prev_softnet_squeeze=0

first_sample=1

while true; do
    TS=$(date +%s)

    # ── CPU (aggregate all cores) ────────────────────────────────────
    read -r _ cu cn cs ci cw cirq csirq _ < <(head -1 /proc/stat)

    # ── UDP counters from /proc/net/snmp ─────────────────────────────
    # Format: Udp: InDatagrams NoPorts InErrors OutDatagrams RcvbufErrors SndbufErrors ...
    udp_line=$(grep '^Udp:' /proc/net/snmp | tail -1)
    udp_in=$(echo "$udp_line" | awk '{print $2}')
    udp_out=$(echo "$udp_line" | awk '{print $5}')
    udp_in_err=$(echo "$udp_line" | awk '{print $4}')
    udp_rcvbuf=$(echo "$udp_line" | awk '{print $6}')
    udp_sndbuf=$(echo "$udp_line" | awk '{print $7}')

    # ── NIC counters ─────────────────────────────────────────────────
    rx_bytes=$(cat "/sys/class/net/${NIC}/statistics/rx_bytes" 2>/dev/null || echo 0)
    tx_bytes=$(cat "/sys/class/net/${NIC}/statistics/tx_bytes" 2>/dev/null || echo 0)
    rx_pkts=$(cat "/sys/class/net/${NIC}/statistics/rx_packets" 2>/dev/null || echo 0)
    tx_pkts=$(cat "/sys/class/net/${NIC}/statistics/tx_packets" 2>/dev/null || echo 0)
    rx_drops=$(cat "/sys/class/net/${NIC}/statistics/rx_dropped" 2>/dev/null || echo 0)
    tx_drops=$(cat "/sys/class/net/${NIC}/statistics/tx_dropped" 2>/dev/null || echo 0)

    # ── Server process memory ────────────────────────────────────────
    srv_pid=$(pgrep -x server 2>/dev/null | head -1 || echo "")
    if [ -n "$srv_pid" ]; then
        read -r rss vsz < <(ps -o rss=,vsz= -p "$srv_pid" 2>/dev/null || echo "0 0")
    else
        rss=0; vsz=0
    fi

    # ── Context switches & interrupts ────────────────────────────────
    ctxt=$(grep '^ctxt' /proc/stat | awk '{print $2}')
    intr=$(grep '^intr' /proc/stat | awk '{print $2}')

    # ── Memory pressure ──────────────────────────────────────────────
    tcp_mem=$(cat /proc/sys/net/ipv4/tcp_mem 2>/dev/null | awk '{print $2}' || echo 0)
    udp_mem=$(cat /proc/sys/net/ipv4/udp_mem 2>/dev/null | awk '{print $2}' || echo 0)

    # ── Softnet stats (aggregate across all CPUs) ────────────────────
    sn_proc=0; sn_drop=0; sn_squeeze=0
    while read -r proc drop squeeze _rest; do
        sn_proc=$((sn_proc + 0x$proc))
        sn_drop=$((sn_drop + 0x$drop))
        sn_squeeze=$((sn_squeeze + 0x$squeeze))
    done < /proc/net/softnet_stat

    # ── Emit delta-based CSV row (skip first sample) ─────────────────
    if [ "$first_sample" -eq 0 ]; then
        echo "${TS},\
$((cu - prev_cpu_user)),$((cn - prev_cpu_nice)),$((cs - prev_cpu_system)),$((ci - prev_cpu_idle)),\
$((cw - prev_cpu_iowait)),$((cirq - prev_cpu_irq)),$((csirq - prev_cpu_softirq)),\
$((udp_in - prev_udp_in)),$((udp_out - prev_udp_out)),$((udp_in_err - prev_udp_in_err)),\
$((udp_rcvbuf - prev_udp_rcvbuf)),$((udp_sndbuf - prev_udp_sndbuf)),\
$((rx_bytes - prev_rx_bytes)),$((tx_bytes - prev_tx_bytes)),\
$((rx_pkts - prev_rx_pkts)),$((tx_pkts - prev_tx_pkts)),\
$((rx_drops - prev_rx_drops)),$((tx_drops - prev_tx_drops)),\
${rss},${vsz},\
$((ctxt - prev_ctxt)),$((intr - prev_intr)),\
${tcp_mem},${udp_mem},\
$((sn_proc - prev_softnet_proc)),$((sn_drop - prev_softnet_drop)),$((sn_squeeze - prev_softnet_squeeze))"
    fi

    # ── Save previous values ─────────────────────────────────────────
    prev_cpu_user=$cu; prev_cpu_nice=$cn; prev_cpu_system=$cs; prev_cpu_idle=$ci
    prev_cpu_iowait=$cw; prev_cpu_irq=$cirq; prev_cpu_softirq=$csirq
    prev_udp_in=$udp_in; prev_udp_out=$udp_out; prev_udp_in_err=$udp_in_err
    prev_udp_rcvbuf=$udp_rcvbuf; prev_udp_sndbuf=$udp_sndbuf
    prev_rx_bytes=$rx_bytes; prev_tx_bytes=$tx_bytes
    prev_rx_pkts=$rx_pkts; prev_tx_pkts=$tx_pkts
    prev_rx_drops=$rx_drops; prev_tx_drops=$tx_drops
    prev_ctxt=$ctxt; prev_intr=$intr
    prev_softnet_proc=$sn_proc; prev_softnet_drop=$sn_drop; prev_softnet_squeeze=$sn_squeeze
    first_sample=0

    sleep "$INTERVAL"
done
