# /// script
# requires-python = ">=3.10"
# dependencies = [
#     "pandas>=2.0",
#     "seaborn>=0.13",
#     "matplotlib>=3.8",
# ]
# ///
"""
plot_results.py — Publication-quality benchmark visualization.

Reads server + client metric CSVs from a results directory and produces
a 6-panel figure with seaborn dark theme.

Usage:
    python bench/plot_results.py <results_dir>
    python bench/plot_results.py bench_results/20260228_013000/

Output:
    <results_dir>/benchmark_report.png  (300 DPI)
    <results_dir>/benchmark_report.svg
"""

import argparse
import glob
import os
import sys

import matplotlib.pyplot as plt
import matplotlib.ticker as mticker
import pandas as pd
import seaborn as sns

# ── Theme & Palette ──────────────────────────────────────────────────

# Dark theme with custom colors
PALETTE = {
    "active": "#00D4AA",      # teal-green
    "failed": "#FF4B6E",      # coral-red
    "tx_pixels": "#FFB347",   # amber
    "rx_dgram": "#7B68EE",    # medium slate blue
    "udp_in": "#4FC3F7",      # light blue
    "udp_errors": "#FF4B6E",  # coral-red
    "cpu_user": "#00D4AA",    # teal
    "cpu_system": "#FFB347",  # amber
    "cpu_softirq": "#FF6B9D", # pink
    "cpu_idle": "#2D2D3F",    # dark (almost invisible)
    "rss": "#E040FB",         # purple
    "net_rx": "#4FC3F7",      # light blue
    "net_tx": "#FFB347",      # amber
    "rx_pkts": "#7B68EE",     # slate blue
    "tx_pkts": "#FF6B9D",     # pink
    "grid": "#3D3D5C",        # muted grid
    "text": "#E0E0E0",        # light text
    "bg": "#1A1A2E",          # deep navy
    "panel_bg": "#16213E",    # slightly lighter navy
}

BG_COLOR = PALETTE["bg"]
PANEL_BG = PALETTE["panel_bg"]


def setup_style():
    """Configure the matplotlib/seaborn dark theme."""
    sns.set_theme(style="darkgrid")
    plt.rcParams.update({
        "figure.facecolor": BG_COLOR,
        "axes.facecolor": PANEL_BG,
        "axes.edgecolor": PALETTE["grid"],
        "axes.labelcolor": PALETTE["text"],
        "text.color": PALETTE["text"],
        "xtick.color": PALETTE["text"],
        "ytick.color": PALETTE["text"],
        "grid.color": PALETTE["grid"],
        "grid.alpha": 0.3,
        "font.family": "sans-serif",
        "font.sans-serif": ["Inter", "Helvetica Neue", "Arial", "sans-serif"],
        "font.size": 10,
        "axes.titlesize": 13,
        "axes.titleweight": "bold",
        "axes.labelsize": 11,
        "legend.fontsize": 9,
        "legend.facecolor": BG_COLOR,
        "legend.edgecolor": PALETTE["grid"],
        "legend.framealpha": 0.8,
    })


def human_format(num, pos=None):
    """Format large numbers: 1200000 → '1.2M'."""
    if abs(num) >= 1e6:
        return f"{num / 1e6:.1f}M"
    if abs(num) >= 1e3:
        return f"{num / 1e3:.0f}K"
    return f"{num:.0f}"


def bytes_format(num, pos=None):
    """Format bytes: 1073741824 → '1.0 GB/s'."""
    if abs(num) >= 1e9:
        return f"{num / 1e9:.1f} GB/s"
    if abs(num) >= 1e6:
        return f"{num / 1e6:.0f} MB/s"
    if abs(num) >= 1e3:
        return f"{num / 1e3:.0f} KB/s"
    return f"{num:.0f} B/s"


# ── Data Loading ─────────────────────────────────────────────────────

def load_client_data(results_dir):
    """Load and merge client CSV files."""
    patterns = [
        os.path.join(results_dir, "*_data.csv"),
        os.path.join(results_dir, "canvas-client*", "*_data.csv"),
    ]
    files = []
    for p in patterns:
        files.extend(glob.glob(p))

    if not files:
        print(f"⚠ No client CSV files found in {results_dir}")
        return None

    dfs = []
    for f in files:
        try:
            df = pd.read_csv(f)
            if "timestamp" not in df.columns:
                continue
            df = df.sort_values("timestamp")
            # Calculate per-second rates from cumulative counters
            if "tx_pixels" in df.columns:
                df["tx_pixels_s"] = df["tx_pixels"].diff().fillna(0).clip(lower=0)
            dfs.append(df)
        except Exception as e:
            print(f"  ⚠ Error reading {f}: {e}")

    if not dfs:
        return None

    combined = pd.concat(dfs, ignore_index=True)
    # Aggregate per timestamp across all workers
    numeric_cols = combined.select_dtypes(include="number").columns
    cols_to_sum = numeric_cols.drop("timestamp", errors="ignore")
    agg = combined.groupby("timestamp")[cols_to_sum].sum().reset_index()

    # Create relative time in seconds
    t0 = agg["timestamp"].min()
    agg["elapsed_s"] = agg["timestamp"] - t0

    print(f"  ✓ Loaded {len(files)} client CSV files ({len(agg)} data points)")
    return agg


def load_server_data(results_dir):
    """Load server metrics CSV."""
    patterns = [
        os.path.join(results_dir, "server_metrics.csv"),
        os.path.join(results_dir, "server", "server_metrics.csv"),
    ]
    for p in patterns:
        if os.path.exists(p):
            try:
                df = pd.read_csv(p)
                if "timestamp" not in df.columns:
                    print(f"  ⚠ server_metrics.csv missing 'timestamp' column")
                    return None
                t0 = df["timestamp"].min()
                df["elapsed_s"] = df["timestamp"] - t0
                print(f"  ✓ Loaded server metrics ({len(df)} data points)")
                return df
            except Exception as e:
                print(f"  ⚠ Error reading {p}: {e}")

    print(f"  ⚠ No server_metrics.csv found in {results_dir}")
    return None


# ── Plotting ─────────────────────────────────────────────────────────

def plot_panel_connections(ax, client_df):
    """Panel 1: Client connections over time."""
    ax.fill_between(
        client_df["elapsed_s"], 0, client_df["active"],
        alpha=0.3, color=PALETTE["active"],
    )
    ax.plot(
        client_df["elapsed_s"], client_df["active"],
        color=PALETTE["active"], linewidth=1.5, label="Active",
    )
    if "failed" in client_df.columns and client_df["failed"].max() > 0:
        ax.plot(
            client_df["elapsed_s"], client_df["failed"],
            color=PALETTE["failed"], linewidth=1.5, linestyle="--", label="Failed",
        )
    ax.set_title("Client Connections")
    ax.set_ylabel("Connections")
    ax.yaxis.set_major_formatter(mticker.FuncFormatter(human_format))
    ax.legend(loc="upper left")
    # Annotate peak
    peak_idx = client_df["active"].idxmax()
    peak_val = client_df.loc[peak_idx, "active"]
    peak_t = client_df.loc[peak_idx, "elapsed_s"]
    ax.annotate(
        f"Peak: {human_format(peak_val)}",
        xy=(peak_t, peak_val),
        xytext=(peak_t + 20, peak_val * 0.85),
        fontsize=9, color=PALETTE["active"],
        arrowprops=dict(arrowstyle="->", color=PALETTE["active"], lw=0.8),
    )


def plot_panel_throughput(ax, client_df):
    """Panel 2: TX/RX throughput."""
    if "tx_pps" in client_df.columns:
        ax.plot(
            client_df["elapsed_s"], client_df["tx_pps"],
            color=PALETTE["tx_pixels"], linewidth=1.5, label="TX Pixels/s",
        )
    elif "tx_pixels_s" in client_df.columns:
        ax.plot(
            client_df["elapsed_s"], client_df["tx_pixels_s"],
            color=PALETTE["tx_pixels"], linewidth=1.5, label="TX Pixels/s",
        )

    if "rx_dgram_s" in client_df.columns:
        ax.plot(
            client_df["elapsed_s"], client_df["rx_dgram_s"],
            color=PALETTE["rx_dgram"], linewidth=1.5, label="RX Datagrams/s",
        )

    ax.set_title("Client Throughput")
    ax.set_ylabel("Messages / second")
    ax.yaxis.set_major_formatter(mticker.FuncFormatter(human_format))
    ax.legend(loc="upper left")

    # Annotate average steady-state throughput
    if "tx_pps" in client_df.columns:
        col = "tx_pps"
    elif "tx_pixels_s" in client_df.columns:
        col = "tx_pixels_s"
    else:
        return
    # Use last 60% of data as "steady state"
    ss = client_df.iloc[int(len(client_df) * 0.4):]
    avg = ss[col].mean()
    ax.axhline(avg, color=PALETTE["tx_pixels"], linestyle=":", alpha=0.5, linewidth=1)
    ax.text(
        client_df["elapsed_s"].max() * 0.02, avg * 1.08,
        f"Avg: {human_format(avg)}/s",
        fontsize=9, color=PALETTE["tx_pixels"], alpha=0.8,
    )


def plot_panel_udp_health(ax, server_df):
    """Panel 3: Server UDP health (datagrams in/s and errors)."""
    ax.plot(
        server_df["elapsed_s"], server_df["udp_in_dgrams"],
        color=PALETTE["udp_in"], linewidth=1.5, label="UDP In/s",
    )
    ax.plot(
        server_df["elapsed_s"], server_df["udp_out_dgrams"],
        color=PALETTE["tx_pixels"], linewidth=1.2, alpha=0.7, label="UDP Out/s",
    )

    # Error overlay on twin axis
    has_errors = (
        server_df["udp_rcvbuf_errors"].sum() > 0
        or server_df["udp_sndbuf_errors"].sum() > 0
    )
    if has_errors:
        ax2 = ax.twinx()
        ax2.bar(
            server_df["elapsed_s"], server_df["udp_rcvbuf_errors"],
            color=PALETTE["udp_errors"], alpha=0.6, width=0.8, label="RcvbufErr/s",
        )
        if server_df["udp_sndbuf_errors"].sum() > 0:
            ax2.bar(
                server_df["elapsed_s"], server_df["udp_sndbuf_errors"],
                color="#FF9800", alpha=0.5, width=0.8,
                bottom=server_df["udp_rcvbuf_errors"], label="SndBufErr/s",
            )
        ax2.set_ylabel("Errors / sec", color=PALETTE["udp_errors"])
        ax2.tick_params(axis="y", labelcolor=PALETTE["udp_errors"])
        ax2.legend(loc="upper right", fontsize=8)

    ax.set_title("Server UDP Health")
    ax.set_ylabel("Datagrams / sec")
    ax.yaxis.set_major_formatter(mticker.FuncFormatter(human_format))
    ax.legend(loc="upper left")


def plot_panel_cpu(ax, server_df):
    """Panel 4: Server CPU utilization (stacked area)."""
    # CPU jiffies → percentage
    total = (
        server_df["cpu_user"] + server_df["cpu_nice"] + server_df["cpu_system"]
        + server_df["cpu_idle"] + server_df["cpu_iowait"]
        + server_df["cpu_irq"] + server_df["cpu_softirq"]
    ).replace(0, 1)  # avoid division by zero

    pct_user = server_df["cpu_user"] / total * 100
    pct_sys = server_df["cpu_system"] / total * 100
    pct_sirq = server_df["cpu_softirq"] / total * 100
    pct_idle = server_df["cpu_idle"] / total * 100

    ax.stackplot(
        server_df["elapsed_s"],
        pct_user, pct_sys, pct_sirq, pct_idle,
        labels=["User", "System", "Softirq", "Idle"],
        colors=[PALETTE["cpu_user"], PALETTE["cpu_system"],
                PALETTE["cpu_softirq"], PALETTE["cpu_idle"]],
        alpha=0.8,
    )
    ax.set_title("Server CPU Utilization")
    ax.set_ylabel("CPU %")
    ax.set_ylim(0, 100)
    ax.legend(loc="upper left", ncol=4)


def plot_panel_memory(ax, server_df):
    """Panel 5: Server memory usage."""
    rss_mb = server_df["server_rss_kb"] / 1024
    ax.fill_between(
        server_df["elapsed_s"], 0, rss_mb,
        alpha=0.3, color=PALETTE["rss"],
    )
    ax.plot(
        server_df["elapsed_s"], rss_mb,
        color=PALETTE["rss"], linewidth=1.5, label="RSS",
    )
    ax.set_title("Server Memory (RSS)")
    ax.set_ylabel("MB")
    ax.legend(loc="upper left")

    # Annotate peak
    peak_mb = rss_mb.max()
    ax.text(
        server_df["elapsed_s"].max() * 0.7, peak_mb * 0.9,
        f"Peak: {peak_mb:.0f} MB ({peak_mb / 1024:.1f} GB)",
        fontsize=9, color=PALETTE["rss"],
    )


def plot_panel_network(ax, server_df):
    """Panel 6: Network I/O (bytes/s and packets/s)."""
    ax.plot(
        server_df["elapsed_s"], server_df["net_rx_bytes"],
        color=PALETTE["net_rx"], linewidth=1.5, label="RX",
    )
    ax.plot(
        server_df["elapsed_s"], server_df["net_tx_bytes"],
        color=PALETTE["net_tx"], linewidth=1.5, label="TX",
    )
    ax.set_title("Network I/O (bytes/s)")
    ax.set_xlabel("Time (seconds)")
    ax.set_ylabel("Bytes / sec")
    ax.yaxis.set_major_formatter(mticker.FuncFormatter(bytes_format))
    ax.legend(loc="upper left")

    # Packets/s on twin axis
    ax2 = ax.twinx()
    ax2.plot(
        server_df["elapsed_s"], server_df["net_rx_packets"],
        color=PALETTE["rx_pkts"], linewidth=1, alpha=0.5, linestyle=":",
        label="RX Pkts/s",
    )
    ax2.plot(
        server_df["elapsed_s"], server_df["net_tx_packets"],
        color=PALETTE["tx_pkts"], linewidth=1, alpha=0.5, linestyle=":",
        label="TX Pkts/s",
    )
    ax2.set_ylabel("Packets / sec", alpha=0.7)
    ax2.yaxis.set_major_formatter(mticker.FuncFormatter(human_format))
    ax2.legend(loc="upper right", fontsize=8)


def add_summary_box(fig, client_df=None, server_df=None):
    """Add a summary stats text box to the figure."""
    lines = []

    if client_df is not None:
        peak_conn = client_df["active"].max()
        lines.append(f"Peak Connections: {human_format(peak_conn)}")

        # Average steady-state throughput (last 60%)
        ss = client_df.iloc[int(len(client_df) * 0.4):]
        for col in ["tx_pps", "tx_pixels_s"]:
            if col in ss.columns:
                avg_tx = ss[col].mean()
                lines.append(f"Avg TX: {human_format(avg_tx)}/s")
                break
        if "rx_dgram_s" in ss.columns:
            avg_rx = ss["rx_dgram_s"].mean()
            lines.append(f"Avg RX: {human_format(avg_rx)}/s")

        if "failed" in client_df.columns:
            total_failed = client_df["failed"].max()
            lines.append(f"Failed: {int(total_failed)}")

        duration = client_df["elapsed_s"].max()
        lines.append(f"Duration: {int(duration)}s")

    if server_df is not None:
        total_rcvbuf_err = server_df["udp_rcvbuf_errors"].sum()
        if total_rcvbuf_err > 0:
            lines.append(f"⚠ RcvbufErrors: {human_format(total_rcvbuf_err)}")
        else:
            lines.append("✓ Zero RcvbufErrors")

        peak_rss = server_df["server_rss_kb"].max() / 1024
        lines.append(f"Peak RSS: {peak_rss:.0f} MB")

    text = "\n".join(lines)
    fig.text(
        0.98, 0.98, text,
        transform=fig.transFigure,
        fontsize=10,
        verticalalignment="top",
        horizontalalignment="right",
        fontfamily="monospace",
        color=PALETTE["text"],
        bbox=dict(
            boxstyle="round,pad=0.5",
            facecolor=BG_COLOR,
            edgecolor=PALETTE["grid"],
            alpha=0.9,
        ),
    )


# ── Main ─────────────────────────────────────────────────────────────

def plot(results_dir):
    """Generate the full benchmark report."""
    print(f"\n{'═' * 60}")
    print(f"  Canvas Benchmark Report Generator")
    print(f"  Results: {results_dir}")
    print(f"{'═' * 60}\n")

    client_df = load_client_data(results_dir)
    server_df = load_server_data(results_dir)

    if client_df is None and server_df is None:
        print("\n✗ No data found. Nothing to plot.")
        sys.exit(1)

    setup_style()

    png_paths = []

    if server_df is not None:
        fig_s, axes_s = plt.subplots(2, 2, figsize=(18, 11))
        axes_flat_s = axes_s.flatten()
        plot_panel_udp_health(axes_flat_s[0], server_df)
        plot_panel_cpu(axes_flat_s[1], server_df)
        plot_panel_memory(axes_flat_s[2], server_df)
        plot_panel_network(axes_flat_s[3], server_df)

        add_summary_box(fig_s, server_df=server_df)

        fig_s.suptitle(
            "Canvas Server — Benchmark Report",
            fontsize=18, fontweight="bold", color=PALETTE["text"],
            y=0.995,
        )

        fig_s.tight_layout(rect=[0, 0, 0.85, 0.97])

        s_png_path = os.path.join(results_dir, "benchmark_server_report.png")
        s_svg_path = os.path.join(results_dir, "benchmark_server_report.svg")
        fig_s.savefig(s_png_path, dpi=300, bbox_inches="tight", facecolor=BG_COLOR)
        fig_s.savefig(s_svg_path, bbox_inches="tight", facecolor=BG_COLOR)

        print(f"\n  ✓ Server PNG: {s_png_path}")
        print(f"  ✓ Server SVG: {s_svg_path}")
        png_paths.append(s_png_path)

    if client_df is not None:
        fig_c, axes_c = plt.subplots(1, 2, figsize=(18, 5))
        plot_panel_connections(axes_c[0], client_df)
        plot_panel_throughput(axes_c[1], client_df)

        add_summary_box(fig_c, client_df=client_df)

        fig_c.suptitle(
            "Canvas Client — Benchmark Report",
            fontsize=18, fontweight="bold", color=PALETTE["text"],
            y=0.995,
        )

        fig_c.tight_layout(rect=[0, 0, 0.85, 0.97])

        c_png_path = os.path.join(results_dir, "benchmark_client_report.png")
        c_svg_path = os.path.join(results_dir, "benchmark_client_report.svg")
        fig_c.savefig(c_png_path, dpi=300, bbox_inches="tight", facecolor=BG_COLOR)
        fig_c.savefig(c_svg_path, bbox_inches="tight", facecolor=BG_COLOR)

        print(f"\n  ✓ Client PNG: {c_png_path}")
        print(f"  ✓ Client SVG: {c_svg_path}")
        png_paths.append(c_png_path)

    print(f"\n{'═' * 60}\n")

    return png_paths[0] if png_paths else None


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description="Generate benchmark report from collected metrics."
    )
    parser.add_argument(
        "results_dir",
        help="Path to the results directory (e.g. bench_results/20260228_013000/)",
    )
    args = parser.parse_args()

    if not os.path.isdir(args.results_dir):
        print(f"Error: '{args.results_dir}' is not a directory")
        sys.exit(1)

    plot(args.results_dir)
