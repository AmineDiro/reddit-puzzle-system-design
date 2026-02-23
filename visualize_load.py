import pandas as pd
import glob
import matplotlib.pyplot as plt

def merge_and_plot():
    files = glob.glob("test_results/*_data.csv")
    if not files:
        files = glob.glob("/metrics/*_data.csv")
        
    if not files:
        print("No CSV files found in test_results/ or /metrics/")
        return
        
    dfs = []
    for f in files:
        try:
            df = pd.read_csv(f)
            df = df.sort_values('timestamp')
            # Calculate per-worker rate from cumulative counter
            df['tx_pixels_s'] = df['tx_pixels'].diff().fillna(0)
            # Handle potential counter resets
            df.loc[df['tx_pixels_s'] < 0, 'tx_pixels_s'] = 0
            dfs.append(df)
        except Exception as e:
            print(f"Error reading {f}: {e}")
            
    if not dfs:
        return
        
    # Combine and group by timestamp to aggregate all workers
    combined = pd.concat(dfs)
    agg = combined.groupby('timestamp').sum().reset_index()
    
    # Plotting
    fig, axes = plt.subplots(3, 1, figsize=(10, 15), sharex=True)
    
    # 1. Active Clients
    axes[0].plot(agg['timestamp'], agg['active'], label='Active Clients', color='blue')
    axes[0].plot(agg['timestamp'], agg['failed'], label='Failed Connects', color='red')
    axes[0].set_title('Client Connections')
    axes[0].set_ylabel('Count')
    axes[0].legend()
    axes[0].grid(True)
    
    # 2. Datagrams per second & Pixels per second
    axes[1].plot(agg['timestamp'], agg['rx_dgram_s'], label='RX Datagrams/s', color='green')
    axes[1].plot(agg['timestamp'], agg['tx_pixels_s'], label='TX Pixels/s', color='orange')
    axes[1].set_title('Throughput (Messages)')
    axes[1].set_ylabel('Messages / second')
    axes[1].legend()
    axes[1].grid(True)
    
    # 3. Bandwidth
    axes[2].plot(agg['timestamp'], agg['rx_mbps'], label='RX Bandwidth (Mbps)', color='purple')
    axes[2].set_title('Bandwidth (Mbps)')
    axes[2].set_xlabel('Timestamp (Unix epoch)')
    axes[2].set_ylabel('Mbps')
    axes[2].legend()
    axes[2].grid(True)
    
    plt.tight_layout()
    # Save the output to a file
    output_path = 'load_report.png'
    plt.savefig(output_path)
    print(f"Saved report to {output_path}")

if __name__ == "__main__":
    merge_and_plot()
