# Ansible: Hetzner Benchmark Automation

Automates the full setup from `hetzner_setup.md` — one command to go from fresh Hetzner machines to a tuned, built, verified benchmark environment.

## Prerequisites (on your local machine)

```bash
pip install ansible ansible-lint
ansible-galaxy collection install ansible.posix community.general
```

## Step 0: Fill in Inventory

Edit `inventory.ini` and replace the public IPs:

```ini
[server]
canvas-server ansible_host=YOUR_SERVER_PUBLIC_IP ansible_user=root

[clients]
canvas-client1 ansible_host=YOUR_CLIENT1_PUBLIC_IP ansible_user=root
canvas-client2 ansible_host=YOUR_CLIENT2_PUBLIC_IP ansible_user=root
```

Also set `vlan_parent_iface` to the vSwitch interface name visible on your machines (check with `ssh root@<IP> ip link`).  
Common names: `eth1`, `enp7s0f1`, `enp0s31f6`.

> **Important**: Create the vSwitch in Hetzner Robot first (§2.1 of hetzner_setup.md) and add all 3 machines to it before running the playbook. Ansible can't do this step — it requires the Hetzner web console.

## Step 1: Test SSH Access

```bash
ansible -i inventory.ini all -m ping
```

All 3 machines should return `pong`.

## Step 2: Run the Full Setup

```bash
ansible-playbook -i inventory.ini site.yml
```

This runs in order:
1. **vlan** — configures `eth1.4000` with private IPs, persists via netplan
2. **kernel_tune** — socket buffers, UDP mem, conntrack, CPU governor, ASLR off
3. **firewall** — NOTRACK rules (conntrack bypass), ACCEPT from private subnet
4. **pin_irqs** — NIC IRQ → SMT sibling core pinning (server only)
5. **build** — installs Rust, clones repo, builds with `target-cpu=native + lto=fat`

Total runtime: ~15-20 minutes (dominated by Rust compilation).

## Step 3: Verify Everything Works

```bash
ansible-playbook -i inventory.ini verify.yml
```

Checks:
- Ping RTT < 2ms between all machines over private VLAN
- iperf3 UDP bandwidth ≥ 8 Gbit/s between client-1 and server
- Server binary exists at `/opt/canvas/target/release/server`
- Client binary exists at `/opt/canvas/target/release/client` on both clients

## Re-running Individual Steps

Tags let you re-run only what changed:

```bash
# After a `git push` — rebuild only
ansible-playbook -i inventory.ini site.yml --tags build

# After changing sysctl values — retune only
ansible-playbook -i inventory.ini site.yml --tags kernel

# After changing firewall rules
ansible-playbook -i inventory.ini site.yml --tags firewall

# Only the server
ansible-playbook -i inventory.ini site.yml --limit canvas-server

# Dry run (check what would change)
ansible-playbook -i inventory.ini site.yml --check --diff
```

## File Structure

```
ansible/
├── inventory.ini               # Machine IPs and shared VLAN variables
├── host_vars/
│   ├── canvas-server.yml       # private IP, num_workers
│   ├── canvas-client1.yml      # private IP, loadgen config
│   └── canvas-client2.yml
├── site.yml                    # Master playbook (full setup)
├── verify.yml                  # Connectivity + binary health checks
└── roles/
    ├── vlan/                   # VLAN sub-interface + netplan persistence
    ├── kernel_tune/            # sysctl, fd limits, CPU governor
    ├── firewall/               # iptables NOTRACK + ACCEPT rules
    ├── pin_irqs/               # NIC IRQ → SMT core pinning (server)
    └── build/                  # Clone repo + cargo build --release
```

## Variable Reference

| Variable | Default | Where set | Description |
|----------|---------|-----------|-------------|
| `vlan_id` | `4000` | `inventory.ini` | Must match Hetzner Robot vSwitch VLAN ID |
| `vlan_parent_iface` | `eth1` | `inventory.ini` | Physical vSwitch port (check `ip link`) |
| `vlan_mtu` | `1400` | `inventory.ini` | Conservative MTU for VLAN headers |
| `private_ip` | per host | `host_vars/` | Private VLAN IP for each machine |
| `num_workers` | `47` | `host_vars/canvas-server.yml` | Server worker threads |
| `nic_iface` | `eth0` | `site.yml` (pin_irqs play) | NIC whose IRQs get pinned |
| `rmem_max` | 128MB (server) / 64MB (client) | `site.yml` | Kernel socket buffer ceiling |
