# Sonic WiFi — Ethernet over OFDM Audio

> **Status: Implemented.** All modules (tap, phy, mac, fragment) are built and
> integrated. CSMA/CA state machine runs with per-fragment ACK and exponential
> backoff. Remaining work: real-world multi-node testing, RTS/CTS, echo
> cancellation for full-duplex.

## Goal

Bridge a Linux TAP interface to the OFDM audio modem (`sosw-core`), creating a
shared-medium Ethernet network where all devices communicate over sound.
Heavily inspired by the original Ethernet (thicknet coax) — a single shared
channel with CSMA/CA, backoff, and ACK-based reliability.

## Architecture

```
Application (ping, ssh, curl, ...)
    ↓  standard sockets
Linux TCP/IP stack
    ↓  raw Ethernet frames
TAP interface  (tappers crate)
    ↓  Ethernet frames (up to 1500 bytes)
┌─────────────────────────────────────┐
│         sosw-tap (new crate)        │
│                                     │
│  tap.rs        — TAP device wrapper │
│  fragment.rs   — Frag/reassemble    │
│  mac.rs        — CSMA/CA engine     │
│  phy.rs        — Audio I/O + OFDM   │
└─────────────────────────────────────┘
    ↓  OFDM frames (~442 B payload)
sosw-core  (OfdmModulator / OfdmDemodulator)
    ↓  audio samples (48 kHz, f32)
cpal  (PulseAudio / ALSA / WASAPI)
    ↓
Speakers / Microphone
```

## Crate: `sosw-tap`

New workspace member, depends on:

| Dependency       | Why                              |
|------------------|----------------------------------|
| `sosw-core`      | OFDM modem, frame types          |
| `tappers`        | TAP interface (link-layer)       |
| `cpal`           | Audio I/O                        |
| `clap`           | CLI argument parsing             |
| `tokio`          | Async runtime (timers, channels) |
| `anyhow`         | Error handling                   |
| `rand`           | Random backoff                   |

### MAC Layer Protocol

Each OFDM frame carries a 4-bit `frame_type` in its header:

| Type | Name    | Payload content           |
|------|---------|---------------------------|
| `0`  | DATA    | MAC header + frag payload |
| `1`  | ACK     | ACK packet                |

#### DATA frame payload layout

```
Offset  Size  Field
──────────────────────────────
0       1     dst_id           — recipient node ID
1       1     src_id           — sender node ID
2       1     flags | index    — bit7=first_frag, bit6=last_frag, low6=frag_index
3       1     frame_id (low)   — 8-bit, wraps quickly
4+      up to 438  fragment data (Ethernet frame bytes)
──────────────────────────────
Total: 4 header + 438 data = 442 bytes max
```

#### ACK frame payload layout

```
Offset  Size  Field
──────────────────────────────
0       1     dst_id
1       1     src_id
2       1     frame_id (being ACKed)
3       1     bitmap  (bit i = fragment i received OK)
──────────────────────────────
Total: 4 bytes
```

### Fragmentation (`fragment.rs`)

- TAP MTU = 1500 bytes (standard Ethernet)
- Each Ethernet frame is split into ceil(1500 / 438) = 4 fragments max
- The reassembly buffer groups fragments by `(src_id, frame_id)`
- A frame is complete when first_frag + last_frag flags seen and all
  fragments accounted for (derived from `frag_index` sequence)

### CSMA/CA MAC (`mac.rs`)

```
States: IDLE → SENSE → BACKOFF → TX_FRAGMENT → WAIT_ACK → (next frag / done)

Per-fragment procedure:
  1.  DIFS wait: channel must be clear for DIFS duration (300 ms)
  2.  Backoff: random(CW) × SLOT (60 ms), decrement only when idle
  3.  Transmit: modulate fragment → play audio output
  4.  WAIT_ACK: listen for ACK frame (timeout = 600 ms)
  5.  ACK received → next fragment
  6.  Timeout → CW = min(CW×2, CWmax), retry (max 5)
  7.  Exhausted → drop the Ethernet frame

Receiving:
  1.  Continuously demodulate audio input
  2.  DATA frame arriving → parse MAC header
  3.  dst_id == our_id → send ACK, buffer fragment
  4.  Fragment complete → reassemble → deliver to TAP
  5.  ACK frame arriving → match pending TX → wake transmitter
```

### Node Addressing

- Each node has a 1-byte `node_id` (0-255), derived from:
  1. CLI flag `--node-id`
  2. Or last byte of the TAP interface MAC address
  3. Or randomly generated locally-administered MAC (`02:xx:xx:xx:xx:xx`)
- The node ID is used in the MAC header only; Ethernet frames inside
  carry full 6-byte MAC addresses as usual.

### Timing Parameters

| Parameter    | Default | Description                     |
|--------------|---------|---------------------------------|
| OFDM frame   | ~258 ms | 8 preamble + 35 data symbols    |
| ACK frame    | ~54 ms  | 8 preamble + 1 data symbol      |
| DIFS         | 300 ms  | Clear-channel observation       |
| SLOT         | 60 ms   | Backoff slot duration           |
| CWmin        | 2 slots | Initial contention window       |
| CWmax        | 32 slots| Maximum contention window       |
| ACK timeout  | 600 ms  | Per-fragment ACK wait           |
| Max retries  | 5       | Per-fragment retransmit limit   |

### CLI

```bash
# Start a sonic Ethernet node
sosw-tap serve \
    --tap sosw0                    # TAP interface name
    --ip 10.0.0.1/24              # IP address (optional)
    --node-id 42                   # 1-byte node ID (optional)
    [--tx-device ...]              # cpal output device
    [--rx-device ...]              # cpal input device
    [--mac 02:00:00:00:00:01]     # explicit MAC (optional)

# List audio devices
sosw-tap list-devices
```

## Implementation Order

1. ~~sosw-core: add `assemble_frame_with_type()` to FrameAssembler~~ (done)
2. ~~Crate scaffold: Cargo.toml, workspace membership, modules~~ (done)
3. ~~`tap.rs` — TAP open/configure, MAC address~~ (done)
4. ~~`phy.rs` — cpal streams, OFDM bridge, carrier sense~~ (done)
5. ~~`fragment.rs` — Split/join Ethernet frames~~ (done)
6. ~~`mac.rs` — CSMA/CA state machine~~ (done)
7. ~~`main.rs` — CLI entry point, wiring~~ (done)
8. ~~Build + fix~~ (done)
9. Multi-node real-world testing (next)
10. RTS/CTS for hidden terminal problem
11. Echo cancellation for full-duplex

## Known Limitations (v1)

- **Linux-only** for the tap MAC-address query (`SIOCGIFHWADDR`)
- **Half-duplex only**: Mute RX during TX (no echo cancellation)
- **Root required**: TAP creation needs `CAP_NET_ADMIN`
- **Single audio device**: TX and RX share the same cpal device
- **No RTS/CTS**: Hidden terminal problem exists; fine for 2-node demo
- **No hardware addressing in MAC**: Broadcast-only at the link layer
