// =============================================================================
// const_settings.rs — Single source of truth for all server constants
// =============================================================================
//
// ┌─────────────────────────────────────────────────────────────────────────────┐
// │  TUNING GUIDE                                                              │
// │                                                                            │
// │  1. Set MAX_CONNECTIONS_PER_WORKER to desired connection ceiling.           │
// │  2. All io_uring, cooldown, and TX constants auto-derive from it.          │
// │  3. Set CANVAS_WIDTH/HEIGHT for your canvas dimensions.                    │
// │  4. Everything else adapts. Check MEMORY BUDGET at the bottom.             │
// └─────────────────────────────────────────────────────────────────────────────┘

// ---------------------------------------------------------------------------
// Network & Transport  (BASE)
// ---------------------------------------------------------------------------

/// Default server listening port (QUIC / UDP).
pub const SERVER_PORT: u16 = 4433;

/// Maximum UDP packet buffer size for io_uring provided buffers.
/// Must hold the largest possible incoming QUIC packet.
/// QUIC mandates Initial packets are ≥1200 bytes; 2048 covers any UDP payload
/// up to standard Ethernet MTU (1500) plus io_uring recvmsg framing overhead.
pub const PKT_BUF_SIZE: usize = 2048;

/// Maximum payload size when sending datagrams to clients.
/// Sized for standard Ethernet MTU (1500).
pub const DGRAM_MAX_SEND_SIZE: usize = 1500;

/// Kernel socket receive buffer size (bytes).
pub const SOCKET_RECV_BUF_SIZE: usize = 32 * 1024 * 1024; // 32 MB

/// Kernel socket send buffer size (bytes).
pub const SOCKET_SEND_BUF_SIZE: usize = 32 * 1024 * 1024; // 32 MB

// ---------------------------------------------------------------------------
// Per-Worker Connection Limits  (BASE — most things derive from this)
// ---------------------------------------------------------------------------

/// Maximum number of concurrent QUIC connections a single worker can hold.
/// This is the **primary tuning knob**: cooldown bitset size, timing wheel
/// width, TX capacity, io_uring buffer count, and SQ depth all derive from it.
///
/// Must be a multiple of 64 so the cooldown bitset packs evenly into u64s.
pub const MAX_CONNECTIONS_PER_WORKER: usize = 65_536;

// ---------------------------------------------------------------------------
// Application-Layer Data Sizes  (used to derive heuristics below)
// ---------------------------------------------------------------------------

/// Size of individual pixel wire format: x(u16) + y(u16) + color(u8) = 5 bytes.
/// Matches size_of::<PixelDatagram>() which is #[repr(C, packed)].
pub const PIXEL_DATAGRAM_SIZE: usize = 5;

/// Size of a single diff entry in the broadcast diff buffer: index(u32) + color(u8).
pub const DIFF_ENTRY_SIZE: usize = 5;

/// Estimated QUIC overhead per packet (short header + DATAGRAM frame + AEAD tag).
///   1 (form byte) + 8 (conn ID) + 4 (pkt num) + 3 (frame hdr) + 16 (AEAD) ≈ 32
///   Rounded up to 40 for safety margin.
#[allow(dead_code)]
const EST_QUIC_OVERHEAD: usize = 40;

/// Estimated total incoming packet size in an io_uring provided buffer.
/// = recvmsg_out(16) + sockaddr_in(16) + cmsg(MSG_CONTROL_LEN) + QUIC payload.
/// QUIC payload for a pixel datagram ≈ EST_QUIC_OVERHEAD + PIXEL_DATAGRAM_SIZE.
#[allow(dead_code)]
const EST_INCOMING_BUF_USAGE: usize =
    16 + 16 + MSG_CONTROL_LEN + EST_QUIC_OVERHEAD + PIXEL_DATAGRAM_SIZE; // ~141 bytes

// ---------------------------------------------------------------------------
// Broadcasting
// ---------------------------------------------------------------------------

/// Maximum chunk size when broadcasting canvas data to clients.
///
/// Heuristic: must fit in one UDP packet after all headers.
///   Ethernet MTU (1500) - IP(20) - UDP(8) - QUIC overhead(~40) ≈ 1432
///   We use 1200 conservatively to handle path MTU < 1500 and IPv6 headers.
pub const BROADCAST_CHUNK_SIZE: usize = 1200;

/// How often the master publishes a new canvas snapshot (milliseconds).
pub const BROADCAST_INTERVAL_MS: u64 = 100;

/// Send a full (RLE-compressed) canvas every N broadcasts instead of a diff.
/// 60 × 100ms = every 6 seconds.
pub const FULL_BROADCAST_INTERVAL: u32 = 60;

// ---------------------------------------------------------------------------
// Cooldown Bitset  (derived from MAX_CONNECTIONS_PER_WORKER)
// ---------------------------------------------------------------------------

/// Bits per chunk in the cooldown bitset (= 64 for u64).
const BITS_PER_COOLDOWN_CHUNK: usize = std::mem::size_of::<u64>() * 8;

/// Number of u64 chunks in the cooldown bitset.
/// Each u64 tracks BITS_PER_COOLDOWN_CHUNK connections.
pub const COOLDOWN_ARRAY_LEN: usize = MAX_CONNECTIONS_PER_WORKER / BITS_PER_COOLDOWN_CHUNK;

// ---------------------------------------------------------------------------
// Timing Wheel
// ---------------------------------------------------------------------------

/// Number of ticks in the timing wheel (1 tick = 1 second).
/// Determines how long a user stays on cooldown before being evicted.
/// 300 ticks = 5 minutes.
pub const TIMING_WHEEL_TICKS: usize = 300;

// ---------------------------------------------------------------------------
// io_uring  (derived from MAX_CONNECTIONS_PER_WORKER & socket buffers)
// ---------------------------------------------------------------------------

/// Number of pre-registered receive buffers provided to io_uring.
///
/// Heuristic: match the kernel socket receive buffer capacity.
///   max queued packets = SOCKET_RECV_BUF_SIZE / PKT_BUF_SIZE
///   Capped at u16::MAX (65535) which is the io_uring provided-buffer limit.
///
/// Buffers are consumed on RX and replenished after processing each CQE.
/// Under burst, all buffers may fill before we process any. If that happens,
/// io_uring returns ENOBUFS and RecvMsgMulti is resubmitted — packets stay
/// safe in the kernel socket buffer until we replenish.
pub const IO_URING_NUM_BUFFERS: u16 = {
    let derived = SOCKET_RECV_BUF_SIZE / PKT_BUF_SIZE; // 32MB / 2048 = 16384
    if derived > u16::MAX as usize {
        u16::MAX
    } else {
        derived as u16
    }
};

/// io_uring submission queue depth (must be a power of two).
///
/// Heuristic: must accommodate a full TX flush wave + RX buffer replenish.
///   - TX flush: up to MAX_CONNECTIONS_PER_WORKER SendMsg SQEs in one pass
///     (one conn.send() per connection for a diff broadcast).
///   - RX replenish: one ProvideBuffers SQE per processed CQE.
///   - When the SQ fills, the code calls submit() and retries, so undersizing
///     is safe — it just causes extra syscalls.
///
/// We size at half of MAX_CONNECTIONS_PER_WORKER (rounded to power of two)
/// since TX items are recycled by completed CQEs between pushes.
pub const IO_URING_SQ_DEPTH: u32 = {
    let target = MAX_CONNECTIONS_PER_WORKER / 2;
    // const next_power_of_two: 1 << (BITS - leading_zeros(target - 1))
    1u32 << (u32::BITS - ((target as u32) - 1).leading_zeros())
};

/// Buffer Group ID for io_uring provided buffers.
pub const IO_URING_BGID: u16 = 0;

/// Tag embedded in io_uring CQE user_data to identify incoming UDP completions.
pub const TAG_INCOMING_UDP: u64 = 1;

/// Tag embedded in io_uring CQE user_data to identify outgoing UDP completions.
pub const TAG_OUTGOING_UDP: u64 = 2;

/// Number of pre-allocated TX items (outgoing sendmsg slots).
///
/// Heuristic: one slot per connection.
///   During a diff broadcast (the common case), each connection produces ~1
///   conn.send() call → 1 TxItem. So MAX_CONNECTIONS_PER_WORKER covers a
///   full diff flush without running out of items.
///
///   During a full RLE broadcast (rare, every FULL_BROADCAST_INTERVAL), each
///   connection may produce many more sends. TX items are recycled as CQEs
///   complete, so the flush loop naturally throttles itself when items run out.
pub const TX_CAPACITY: usize = MAX_CONNECTIONS_PER_WORKER;

// ---------------------------------------------------------------------------
// msghdr / ancillary control buffer
// ---------------------------------------------------------------------------

/// Ancillary data (cmsg) buffer size in recvmsg — must be large enough for IP_PKTINFO.
/// sizeof(cmsghdr) + sizeof(in_pktinfo) ≈ 12 + 12 = 24 bytes, padded to 32.
/// We use 64 for generous alignment headroom.
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
pub const CANVAS_BUFFER_POOL_MASK: usize = CANVAS_BUFFER_POOL_SIZE - 1;

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
///
/// Heuristic: must hold all chunks of one broadcast round per connection.
///   Diff broadcast: typically 1-5 chunks (well under 1000).
///   Full broadcast: worst case CANVAS_SIZE*2/BROADCAST_CHUNK_SIZE ≈ 1667 chunks.
///   But dgram_send is best-effort (errors ignored), so overflow just drops
///   late chunks — the next full broadcast will resync. 1000 covers all
///   diffs and most practical full broadcasts.
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

// =============================================================================
// MEMORY BUDGET PER WORKER  (compile-time computed, for documentation)
// =============================================================================
//
// These are not used at runtime — they exist so you can see the memory impact
// of your tuning choices at compile time (e.g., via a const assertion or
// printing them in a build script).

/// Buffer slab: io_uring provided receive buffers.
///   IO_URING_NUM_BUFFERS × PKT_BUF_SIZE bytes.
pub const MEM_BUFFER_SLAB: usize = (IO_URING_NUM_BUFFERS as usize) * PKT_BUF_SIZE;

/// TX items: pre-allocated outgoing sendmsg slots.
///   TX_CAPACITY × DGRAM_MAX_SEND_SIZE bytes (dominates; addr/iov/msghdr are small).
pub const MEM_TX_ITEMS: usize = TX_CAPACITY * (DGRAM_MAX_SEND_SIZE + 88); // +88 for sockaddr+iov+msghdr

/// Cooldown bitset: one per worker.
pub const MEM_COOLDOWN: usize = COOLDOWN_ARRAY_LEN * std::mem::size_of::<u64>();

/// Timing wheel: TIMING_WHEEL_TICKS copies of the cooldown bitset.
pub const MEM_TIMING_WHEEL: usize = TIMING_WHEEL_TICKS * MEM_COOLDOWN;

/// Canvas copy: last_sent_canvas snapshot.
pub const MEM_CANVAS_COPY: usize = CANVAS_SIZE;

/// Total estimated heap memory per worker (bytes).
pub const MEM_PER_WORKER: usize =
    MEM_BUFFER_SLAB + MEM_TX_ITEMS + MEM_COOLDOWN + MEM_TIMING_WHEEL + MEM_CANVAS_COPY;

/// Total buffer pool memory (static, shared across all workers).
///   CANVAS_BUFFER_POOL_SIZE × CANVAS_SIZE × 3 (raw + compressed + lens).
pub const MEM_CANVAS_POOL: usize = CANVAS_BUFFER_POOL_SIZE * CANVAS_SIZE * 3;
