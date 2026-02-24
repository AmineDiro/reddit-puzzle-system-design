// =============================================================================
// const_settings.rs — Single source of truth for all server constants
// =============================================================================
//
// Base constants are defined first. Derived constants are computed from them.
// Every module should import from here instead of defining its own magic numbers.

// ---------------------------------------------------------------------------
// Network & Transport
// ---------------------------------------------------------------------------

/// Default server listening port (QUIC / UDP).
pub const SERVER_PORT: u16 = 4433;

/// Maximum UDP packet buffer size (standard MTU ceiling for QUIC).
pub const PKT_BUF_SIZE: usize = 2048;

/// Maximum payload size when sending datagrams to clients (fits in one UDP packet).
pub const DGRAM_MAX_SEND_SIZE: usize = 1500;

/// Maximum chunk size when broadcasting canvas data to clients.
/// Must fit inside a single QUIC datagram (< MTU).
pub const BROADCAST_CHUNK_SIZE: usize = 1200;

/// Kernel socket receive buffer size (bytes).
pub const SOCKET_RECV_BUF_SIZE: usize = 32 * 1024 * 1024; // 32 MB

/// Kernel socket send buffer size (bytes).
pub const SOCKET_SEND_BUF_SIZE: usize = 32 * 1024 * 1024; // 32 MB

// ---------------------------------------------------------------------------
// Per-Worker Connection Limits  (BASE — everything else derives from this)
// ---------------------------------------------------------------------------

/// Maximum number of concurrent QUIC connections a single worker can hold.
/// This is the primary tuning knob: cooldown bitset size, timing wheel width,
/// TX capacity, and io_uring buffer count all flow from this value.
///
/// Must be a multiple of 64 so the cooldown bitset packs evenly into u64 chunks.
pub const MAX_CONNECTIONS_PER_WORKER: usize = 65_536;

// ---------------------------------------------------------------------------
// Cooldown Bitset  (derived from MAX_CONNECTIONS_PER_WORKER)
// ---------------------------------------------------------------------------

/// Number of u64 chunks in the cooldown bitset.
/// Each u64 tracks 64 connections → total bits = COOLDOWN_ARRAY_LEN * 64.
pub const COOLDOWN_ARRAY_LEN: usize = MAX_CONNECTIONS_PER_WORKER / 64; // 1024

// ---------------------------------------------------------------------------
// Timing Wheel
// ---------------------------------------------------------------------------

/// Number of ticks in the timing wheel (1 tick = 1 second).
/// Determines how long a user stays on cooldown before being evicted.
/// 300 ticks = 5 minutes.
pub const TIMING_WHEEL_TICKS: usize = 300;

// ---------------------------------------------------------------------------
// io_uring
// ---------------------------------------------------------------------------

/// Number of pre-registered receive buffers provided to io_uring.
/// Capped at u16::MAX (65535) which is the io_uring provided-buffer limit.
pub const IO_URING_NUM_BUFFERS: u16 = u16::MAX; // 65535

/// io_uring submission queue depth (must be a power of two).
pub const IO_URING_SQ_DEPTH: u32 = 32_768;

/// Buffer Group ID for io_uring provided buffers.
pub const IO_URING_BGID: u16 = 0;

/// Tag embedded in io_uring CQE user_data to identify incoming UDP completions.
pub const TAG_INCOMING_UDP: u64 = 1;

/// Tag embedded in io_uring CQE user_data to identify outgoing UDP completions.
pub const TAG_OUTGOING_UDP: u64 = 2;

/// Number of pre-allocated TX items (outgoing sendmsg slots).
pub const TX_CAPACITY: usize = MAX_CONNECTIONS_PER_WORKER; // one slot per connection

// ---------------------------------------------------------------------------
// msghdr / ancillary control buffer
// ---------------------------------------------------------------------------

/// Ancillary data (cmsg) buffer size in recvmsg — must be large enough for IP_PKTINFO.
pub const MSG_CONTROL_LEN: usize = 64;

// ---------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------

/// Canvas width in pixels.
pub const CANVAS_WIDTH: usize = 1000;

/// Canvas height in pixels.
pub const CANVAS_HEIGHT: usize = 1000;

/// Total number of pixels in the canvas (1 byte per pixel).
pub const CANVAS_SIZE: usize = CANVAS_WIDTH * CANVAS_HEIGHT;

/// Number of canvas snapshot buffers in the RCU-like pool.
/// Must be a power of two so the master can advance with a bitmask.
pub const CANVAS_BUFFER_POOL_SIZE: usize = 16;

/// Bitmask for cycling through the canvas buffer pool indices.
pub const CANVAS_BUFFER_POOL_MASK: usize = CANVAS_BUFFER_POOL_SIZE - 1; // 15

// ---------------------------------------------------------------------------
// Broadcasting
// ---------------------------------------------------------------------------

/// How often the master publishes a new canvas snapshot (milliseconds).
pub const BROADCAST_INTERVAL_MS: u64 = 100;

/// Send a full (RLE-compressed) canvas every N broadcasts instead of a diff.
pub const FULL_BROADCAST_INTERVAL: u32 = 60;

// ---------------------------------------------------------------------------
// SPSC Ring Buffer  (worker → master pixel queue)
// ---------------------------------------------------------------------------

/// Capacity of each per-worker SPSC ring buffer. Must be a power of two.
pub const SPSC_CAPACITY: usize = 1024;

/// Maximum number of pixel writes the master drains from each worker queue
/// per iteration of its hot loop.
pub const MASTER_BATCH_DRAIN: usize = 128;

// ---------------------------------------------------------------------------
// QUIC / quiche Configuration
// ---------------------------------------------------------------------------

/// Initial QUIC max data (connection-level flow control window).
pub const QUIC_INITIAL_MAX_DATA: u64 = 10_000_000;

/// Initial max stream data for locally-initiated bidirectional streams.
pub const QUIC_INITIAL_MAX_STREAM_DATA_BIDI_LOCAL: u64 = 1_000_000;

/// Initial max stream data for remotely-initiated bidirectional streams.
pub const QUIC_INITIAL_MAX_STREAM_DATA_BIDI_REMOTE: u64 = 1_000_000;

/// Initial max stream data for unidirectional streams.
pub const QUIC_INITIAL_MAX_STREAM_DATA_UNI: u64 = 1_000_000;

/// Maximum number of concurrent bidirectional streams.
pub const QUIC_INITIAL_MAX_STREAMS_BIDI: u64 = 100;

/// Maximum number of concurrent unidirectional streams.
pub const QUIC_INITIAL_MAX_STREAMS_UNI: u64 = 100;

/// Datagram send and receive queue depth inside quiche.
pub const QUIC_DGRAM_QUEUE_LEN: usize = 1000;

// ---------------------------------------------------------------------------
// Connection Maintenance
// ---------------------------------------------------------------------------

/// Minimum interval (ms) between connection timeout sweeps to avoid
/// excessive CPU overhead on large connection counts.
pub const CONN_TIMEOUT_THROTTLE_MS: u128 = 20;

// ---------------------------------------------------------------------------
// Diff Buffer
// ---------------------------------------------------------------------------

/// Initial capacity for the per-worker diff buffer used in delta broadcasts.
pub const DIFF_BUFFER_INITIAL_CAPACITY: usize = 1024;
