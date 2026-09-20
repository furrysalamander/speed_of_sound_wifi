# Speed of Sound WiFi

Acoustic OFDM data modem — transmit data through sound using a speaker and microphone. Pure Rust implementation with a Python legacy codebase for reference.

## Architecture

4-crate workspace sharing the `sosw-core` OFDM modem library:

```
┌──────────────────────────────────────────────────────────────┐
│  Application: sosw-tap (Ethernet-over-sound, CSMA/CA MAC)    │
│              sosw-web (browser modem, 3-tab Leptos app)      │
│              sosw-cli (desktop TX/RX/test via cpal)          │
├──────────────────────────────────────────────────────────────┤
│  sosw-core:  OFDM Modem (no I/O)                             │
│  ┌──────────┬──────────────┬──────────────────────────────┐  │
│  │ physical │ link         │ config                        │  │
│  │  qpsk    │  scrambler   │  5 presets, serde              │  │
│  │  ofdm_mod│  crc         │  FFT/CP/SC/freq derivation    │  │
│  │  ofdm_dem│  fec(RS)     │  symbol_rate, theoretical_bps  │  │
│  │  preamble│  frame       │                                │  │
│  │  dtmf    │              │                                │  │
│  └──────────┴──────────────┴──────────────────────────────┘  │
├──────────────────────────────────────────────────────────────┤
│  Audio: cpal (desktop)  /  Web Audio API (WASM)              │
│  48 kHz mono, f32 samples                                     │
└──────────────────────────────────────────────────────────────┘
```

### Data Flow

```
TX: Bytes → Scramble(ChaCha12) → Pack bits → QPSK map(2 bits/sc)
    → OFDM symbols (IFFT + CP + raised-cosine) → Preamble → Audio samples

RX: Audio → Cross-correlation → Preamble detect
    → Channel estimate + CFO estimate → Per-sym FFT
    → Decision-directed phase correction → Equalization → QPSK demap
    → Unpack bits → Descramble → Bytes
    → FrameParser (sync → RS(255,223) FEC → CRC-32 verify)
```

An optional **time-differential** mode (`Config::differential`) carries data in
the phase difference between consecutive OFDM symbols on the same subcarrier, so
a static channel needs no absolute estimate. It passes software and digital
tests but not the current acoustic path (see [Link Status](#link-status-measured-2026-09-19)).

The **DTMF control channel** (`sosw-tap::link`) is independent of the OFDM modem
and is the only channel verified working across machines.

### Crate Map

| Crate | Purpose | Key Dependencies |
|-------|---------|-----------------|
| `sosw-core` | Core modem library + DTMF control codec (no I/O) | `rustfft`, `ndarray`, `reed-solomon`, `serde` |
| `sosw-cli` | Desktop CLI (TX/RX/test/ota-validate) | `sosw-core`, `clap`, `cpal` |
| `sosw-web` | WASM browser app (RX/TX/Debug tabs) | `sosw-core`, `leptos`, `web-sys` |
| `sosw-tap` | Ethernet-over-sound (CSMA/CA MAC) + DTMF link handshake | `sosw-core`, `tappers`, `cpal`, `tokio` |

## Control Channel (`sosw-tap::link`, `sosw-core::physical::dtmf`)

A robust DTMF tone-pair control channel is used for link training. Each message
is a 7-nibble frame (`SYNC, kind, node_hi, node_lo, param_hi, param_lo, crc`)
transmitted with the configured repetition, and the decoder locks to the known
symbol grid (fine phase search + hysteresis + noise-floor-relative threshold).

```bash
# A initiates, B responds
sosw_link --role a --node-id 1
sosw_link --role b --node-id 2
```

Defaults: 60 ms tone / 60 ms gap, repeat 2. `--symbol-ms --gap-ms --repeat
--echo-tail-ms --turn-gap-ms` are configurable. This channel is verified working
cross-machine; the OFDM data channel is not (see
[Link Status](#link-status-measured-2026-09-19)).

## Link Status (measured 2026-09-19)

This section records what has actually been observed on the development machines
(giratina: ALC1220 analog output + USB PnP Audio Device mic; deoxys: onboard
speaker + Digital Microphone), so it supersedes older claims elsewhere in this
document. Items marked *historical* are not reproducible in the current setup.

### Verified working

- **DTMF control-channel handshake across machines (`sosw_link`)**, both directions.
  Repeated runs show `A: TX HELLO → RX HELLO_ACK → TX TRAIN → RX TRAIN_ACK →
  LINK ESTABLISHED`, and the mirrored sequence on B, with every message decoded
  on the first attempt.
- **OFDM software loopback**: all `sosw-core` tests pass (`cargo test -p sosw-core`).
- **OFDM digital loopback** through each machine's PipeWire monitor source
  (`ota-validate --rx-device <...>.monitor`): **100% RS-correctable** on both
  machines.
- **Frequency sweep** (`scripts/freq_sweep.py`) carries **2–22 kHz with per-tone
  SNR of at least 21 dB on all four paths** (≥30 dB at nearly every tone; the
  21 dB low is deoxys→giratina at 2 kHz). See
  [docs/frequency_sweep.md](docs/frequency_sweep.md).

### Verified not working (acoustic)

- **OFDM over the air** fails on all tested paths: giratina self-loopback,
  deoxys self-loopback, and both cross-machine directions. 0% RS-correctable
  across the tested output levels (sink 0.30–1.00, TX gain 1–18), all five
  presets, and CP lengths 32–4800 samples.
- **Time-differential OFDM** (channel-agnostic; data in consecutive-symbol phase
  differences) and **single-carrier QPSK** (low PAPR, RRC-shaped) each decode on
  the digital monitor but fail acoustically (0%).
- **The legacy Python reference** (`examples/demo_pipeline`, the code used for the
  historical video demo) also decodes **0 bytes** over the air now, while decoding
  2811 bps through the monitor.

### Measured characteristics

Per-tone SNR (dB) from a 2–22 kHz sweep (500 ms tones):

| freq | giratina self | deoxys self | gir→deoxys | deoxys→gir |
|------|---------------|-------------|------------|------------|
| 2 kHz | 61 | 41 | 52 | 21 |
| 4 kHz | 61 | 47 | 47 | 49 |
| 6 kHz | 44 | 34 | 43 | 47 |
| 8 kHz | 39 | 44 | 37 | 40 |
| 10 kHz | 47 | 51 | 45 | 46 |
| 12 kHz | 43 | 56 | 45 | 30 |
| 14 kHz | 44 | 56 | 52 | 44 |
| 16 kHz | 37 | 58 | 41 | 36 |
| 18 kHz | 43 | 52 | 42 | 41 |
| 20 kHz | 37 | 39 | 38 | 38 |
| 22 kHz | 41 | 33 | 37 | 31 |

Capture level, giratina ALC1220 → USB mic, single 2 kHz tone:

| input | output | gain |
|-------|--------|------|
| 0.02 | 0.22 | 20.7 dB |
| 0.05 | 0.54 | 20.6 dB |
| 0.10 | 1.00 | 20.0 dB |
| 0.20 | 1.13 | 15.0 dB |
| 0.40 | 1.17 | 9.3 dB |
| 0.80 | 1.16 | 3.2 dB |

The path is linear only up to input ≈ 0.1 and saturates at output ≈ 1.16.

- Acoustic OFDM **per-subcarrier SNR is 3.1 / 3.2 / 3.2 dB** at TX gains
  2 / 4 / 6 (RX RMS 0.012 / 0.023 / 0.035) — it does not improve with level.
- Clock offset (giratina USB mic vs ALC1220 output): **6.4 ppm**.
- Cross-machine DTMF acoustic delay: **~3–5.7 s** observed (hosts NTP-synced to 18 ms).

The measurements establish a **signal-proportional impairment** (not additive
noise) plus a compressive capture curve. The mechanism has **not** been
confirmed; the capture-path cause is a hypothesis, not a verified fact.

### Not reproducible (historical)

`README.md`, `AGENTS.md` and `docs/frequency_sweep.md` previously reported
"100% RS-correctable" acoustic OFDM for all five presets. Those results are from
June 2026 and are **not reproducible now**, with either the Rust modem or the
Python reference, in the current physical setup (the microphone was moved between
then and now).

The staged path from this state to a proven acoustic link (Ethernet deferred) is
in [docs/link-development-plan.md](docs/link-development-plan.md).

## Acoustic Link Proven (2026-09-20) — Rust FSK

Following the plan in `docs/link-development-plan.md`, the existing coherent
OFDM PHY was set aside and a **non-coherent M-FSK** PHY was built from first
principles, characterized, and proven end to end. Details in
[docs/channel-report.md](docs/channel-report.md).

### What is proven

- **Cross-machine acoustic file transfer, giratina → deoxys**: a 31-byte payload
  (`HELLO-ACOUSTIC-LINK-1234567890!`) was transferred with **exact content
  match**, using FSK + Reed-Solomon FEC + CRC-32 + stop-and-wait ARQ
  (`sosw_ftp`). The link's own ARQ handled a fresh `HELLO_ACOUSTIC` DATA frame and
  a BYE; the receiver log shows both decoded with 1.3 / 3.5 dB worst-case
  headroom.
- **Both directions cross-machine**: a one-way M=2/20 ms FSK probe decodes
  giratina→deoxys (3/5 frames) and deoxys→giratina (3/5 frames, 8.2 dB
  worst-case headroom). The 40% frame loss is why FEC + ARQ are mandatory.
- **Self-loopback decode** on giratina across many configs, with per-frame CRC
  validation.
- **The protocol logic is deterministically tested** in software
  (`sosw_ftp --mode sim`; `xfer` unit tests): clean transfers and 20–25% frame
  drops at 18–20 dB SNR, with real ARQ retransmissions and byte-exact
  reassembly.

### Key measured facts that shaped the design (Stage 0)

- Acoustic latency is **~3.8 s** (marker alignment), so timeouts are multi-second
  and protocol turn-taking is expensive.
- The capture path is linear to drive ≈0.8 and clips at 1.0; **no** drift within
  a sustained tone, but **run- and history-dependent gain** — the likely reason
  wideband coherent OFDM fails. Constant-envelope, bounded bursts are the
  defense.
- Single-tone SNR is ≥30 dB from 300 Hz–20 kHz, so this is not a noise problem.
- The channel is only coherent over a **~350 Hz span**; a single wideband M-FSK
  stream therefore caps at ~333 bps.

### Why the rate is limited: the room is frequency-selective

Sounding the band (24 carriers, 1.0–10.2 kHz, 2-FSK pairs) shows **deep, narrow
(~200 Hz) notches**: carriers decode or are dead depending on where they fall
(deox→gir is solid at 1.0/1.5/2.0/3.5 kHz and dead at 2.5/3.0/4.5/5.0/6.0/8.0
kHz, repeatable). A ~200 Hz null spacing implies a ~5 ms reflection.

- The **500 ms tone sweep is not predictive**: it samples one tone every 2 kHz
  and reports 47–49 dB at 6 kHz, stepping over the nulls that kill a 200 Hz FSK
  pair. "The sweep looked great" is a trap.
- **Coherent OFDM fails** at every CP (64–2048) and every level inside the
  measured linear region (62% byte errors, preamble peak 0.30 vs 0.998 on the
  digital monitor): whole subcarriers sit in nulls and there is no frequency
  interleaving.
- **Non-coherent FSK works** because it only needs one clean 200–350 Hz window.
  Replaying recorded clips showed **zero symbol errors** on every fully-captured
  frame; the apparent PER was recording-window truncation.

### Carrier-selective bank (higher-rate path)

`sosw-core/src/physical/fsk_bank.rs` now supports an explicit carrier list, and
`sosw-tap/src/bin/fsk_bench.rs --carriers ...` sounds/uses it. Per-channel
preamble SNR and match fraction are reported (`--diag`). Six carriers are solid
in **both** directions (1.0/3.0/4.6/8.2/8.6/9.0 kHz); the 6-carrier bank
decodes with **100% conditional success** at payload 16 and ~75% at payload 32
(long frames lose per-channel calibration, so short chunks are used).

Status: the bank PHY and sounding are proven; `xfer::BankTransport` and
`sosw_ftp --carriers` are wired but the bank ARQ receiver is not yet decoding
reliably (remaining bug). Realistic target ~3× the single-stream rate.

### Achieved rates (self-loopback, FEC on)

| PHY | raw | result | effective goodput |
|-----|-----|--------|-------------------|
| single M=2 / 3 ms | 333 bps | 0% frame error | ~130 bps |
| bank 16ch / 20 ms | 800 bps | 0% frame error | ~210 bps |

### Cross-machine ARQ file transfer (M=2 / 5 ms, RS FEC, CRC, stop-and-wait)

| direction | payload | result | throughput |
|-----------|---------|--------|------------|
| giratina → deoxys | 256 B | byte-exact, 0 retransmits | **9.0 B/s** |
| deoxys → giratina | 128 B | byte-exact | **4.3 B/s** (weaker direction) |

The jump from the initial ~0.3 B/s came from fixing real bugs found by
replaying recorded clips, not from tuning blind:

- **Latency was 3.9 s** because cpal used `BufferSize::Default` and PulseAudio
  picked a multi-second buffer. `BufferSize::Fixed(256)` drops it to ~120 ms.
- **The ARQ ACK timeout was 12 s**; at 120 ms latency the sender only needs
  ~3 s, so every lost ACK cost 12 s instead of 3 s.
- **Half-duplex echo**: the sender decoded its own loud DATA echo and the
  receiver re-decoded its own ACK. Fixed with the proven `sosw_link` pattern
  (mute + echo tail on TX, receiver turn gap, collect-then-decode, persistent
  decoded-frame queue).
- **A partial frame's region discarded the rest of the frame** on recv timeout;
  the capture buffer now persists and is only cleared once a valid frame is in
  hand.
- **ACKs were as long as DATA frames** (padded FEC); they now use a short
  preamble with a small RS FEC, cutting ACK air time roughly in half.

Every run logs per-frame signal metrics (min/mean tone SNR, confidence, FEC
status) and the ARQ carries the receiver's measured SNR back in the ACK, so
rate/timing decisions are made from measurements. `--dump-rx` records raw
audio for offline analysis, and `fsk_bench --diag` reports per-frame grid lock
and symbol-level detail.

### New tooling (`sosw-tap`)

- `phy_bench` — Stage 0 channel characterization (gain step, transfer curve,
  impulse response, two-tone IM, tone SNR, latency) with dump/load captures.
- `fsk_bench` — Stage 1/2/4 benchmark: software/self/tx/rx/sweep, single-stream
  and parallel multi-tone bank, FEC, PER/headroom/goodput.
- `link_train` — Stage 3 discovery + headroom measurement + rate negotiation
  (`--mode a|b|sim`).
- `sosw_ftp` — Stage 5 stop-and-wait ARQ file transfer (`--mode send|recv|sim`).

The PHY lives in `sosw-core/src/physical/fsk.rs` and `fsk_bank.rs`; framing/FEC
in the same modules. The old OFDM path remains but is not the acoustic link.

## Config Presets

Software and digital (monitor) loopbacks pass at 100%. Acoustic over-the-air
decoding is not currently reproducible (see [Link Status](#link-status-measured-2026-09-19)).

| Preset | FFT | CP | SC Range | Freq Range | Sym/s | Bitrate |
|--------|-----|----|----------|------------|-------|---------|
| Default | 256 | 32 | 10–69 | 1.9–12.9 kHz | 167 | 20.0 kbps |
| High Baud | 128 | 16 | 4–31 | 1.5–11.6 kHz | 333 | 18.7 kbps |
| Robust | 512 | 64 | 20–120 | 1.9–11.3 kHz | 83 | 16.8 kbps |
| Ultrasonic | 256 | 32 | 80–120 | 15.0–22.5 kHz | 167 | 13.7 kbps |
| Ultrawide | 256 | 16 | 5–110 | 0.9–20.6 kHz | 176 | 37.4 kbps |

**QPSK modulation** (2 bits per active subcarrier). **RS(255,223)** FEC corrects up to `nsym/2` byte errors per block. **CRC-32** (Ethernet/ZIP polynomial) provides integrity verification. **ChaCha12 XOR scrambler** (seed=12345) whitens data to prevent DC bias and improve phase tracking.

**Default payload size**: 442 bytes (2 RS blocks). Validated by software roundtrip across FFT sizes 128–512, CP lengths 16–64, subcarrier ranges 1–127.

## Quick Start

```bash
# Build everything
cargo build

# Core library tests
cargo test -p sosw-core

# CLI
cargo run -p sosw-cli -- list-devices
cargo run -p sosw-cli -- test -p default

# WASM web app
cd sosw-web && trunk serve          # dev server
cd sosw-web && trunk build --release # production (output in dist/)
```

### Prerequisites

- **CLI / TAP**: Rust toolchain, system audio (ALSA/PulseAudio/PipeWire). `libasound2-dev`, `libpulse-dev` on Debian/Ubuntu.
- **WASM**: `trunk` (`cargo install trunk`), `wasm32-unknown-unknown` target (`rustup target add wasm32-unknown-unknown`).
- **sosw-tap**: Linux only (TAP requires `CAP_NET_ADMIN`). Run as root or with capabilities: `sudo setcap cap_net_admin+ep target/release/sosw-tap`.

## CLI Usage

```bash
# List audio devices
sosw-cli list-devices

# Transmit a file
sosw-cli tx file.bin -p ultrawide -d "speaker-name"

# Receive frames
sosw-cli rx -n 100 -p robust -d "mic-name" -o output.bin

# Monitor mode (decode what's in the air)
sosw-cli test -t 30 -p default -d "mic-name"
```

### OTA Validation

Test your hardware with a preset:

```bash
cargo run --release -p sosw-cli --bin ota-validate -- \
    --preset ultrawide --frames 20 \
    --tx-device "ALC1220" --rx-device "USB_PnP"
```

The tool also accepts `--fft --cp --sc-min --sc-max --rs --payload`
(layout overrides) and `--differential` (time-differential OFDM), and reports
measured goodput. As of 2026-09-19 the digital monitor path passes 100%; the
acoustic path does not decode in the current setup (see
[Link Status](#link-status-measured-2026-09-19)).

## Ethernet-over-Sound (`sosw-tap`)

Bridges a Linux TAP interface to the OFDM audio modem. Shared-medium Ethernet with CSMA/CA — all devices communicate over sound.

> **Note (2026-09-19):** the MAC is implemented and exercised by software/mock
> tests, but end-to-end operation depends on the acoustic OFDM link, which does
> not currently decode over the air. See [Link Status](#link-status-measured-2026-09-19).

```
Linux TCP/IP stack → TAP interface → sosw-tap → OFDM modem → Speakers/Mic
```

### Usage

```bash
# Start a sonic Ethernet node
sosw-tap serve \
    --tap sosw0 \
    --preset default \
    --node-id 42 \
    --tx-device "speaker" \
    --rx-device "mic"

# Assign IP and use
sudo ip addr add 10.0.0.1/24 dev sosw0
sudo ip link set sosw0 up
ping 10.0.0.2
```

### MAC Layer

- **CSMA/CA**: DIFS (600 ms), random backoff (4–64 slots × 60 ms), ACK timeout (2000 ms)
- **Fragmentation**: 1500-byte Ethernet frames split into 438-byte fragments (4 frags max), reassembled by (src_id, frame_id)
- **Node addressing**: 1-byte node IDs (0–255), broadcast at MAC layer

## WASM Web App

3-tab browser app built with Leptos:

| Tab | Features |
|-----|----------|
| **RX** | Real-time audio capture, preamble detection, waterfall spectrogram, frames/peak/CFO stats |
| **TX** | File upload, OFDM modulation, AudioBuffer playback, progress bar |
| **Debug** | Config tuning (5 presets + sliders), enhanced monitoring (CRC/FEC failures, per-subcarrier channel magnitude), loopback testing, config persistence via localStorage |

Open `http://localhost:8080` after `trunk serve`. Grant microphone permission when prompted.

## Testing

```bash
# Rust unit tests
cargo test -p sosw-core
cargo test -p sosw-tap

# Software parameter sweep (FFT 128–512, CP 16–64, SC 1–127)
cargo test -p sosw-core --test param_sweep -- --nocapture

# OTA hardware validation (see Link Status for current acoustic results)
cargo run --release -p sosw-cli --bin ota-validate -- --preset default --frames 20

# Python tests (legacy)
python -m pytest tests/ -v
```

### Test Coverage

| Module | Tests | Description |
|--------|-------|-------------|
| `dtmf` (sosw-core lib) | 7 | DTMF encode/decode, grid lock, short symbols, noise rejection |
| `crc_tests` | 9 | CRC-32 encode/verify/corruption |
| `scrambler_tests` | 9 | ChaCha12 XOR, determinism, edge cases |
| `fec_tests` | 5 | RS(255,223) encode/decode |
| `fec_extended_tests` | 8 | Multi-block, error injection, edge cases |
| `framing_tests` | 5 | Frame assemble/parse roundtrip |
| `framing_extended_tests` | 20 | Multi-frame, corruption, edge cases |
| `ofdm_roundtrip` | 4 | Modulator→Demodulator roundtrip |
| `param_sweep` | 4 | Software sweep across parameter ranges |
| `debug_fft` | 1 | FFT sanity check |
| `debug_preamble` | 1 | Preamble cross-correlation |
| sosw-tap lib (`link`, `mac`, `fragment`) | 16 | DTMF link framing, CSMA/CA, fragmentation |
| **Total** | **89** | (1 `mock_tap_loopback` test is ignored) |

## Project Structure

```
speed_of_sound_wifi/
├── Cargo.toml              # Workspace root (4 members)
├── README.md
├── AGENTS.md               # Development guide
├── PLAN.md                 # sosw-tap design document
├── docs/
│   ├── frequency_sweep.md       # Hardware frequency sweep analysis
│   └── link-development-plan.md # Staged plan from current state to a proven link
│
├── sosw-core/              # Core OFDM modem library (no I/O)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs          # Modulator/Demodulator traits, bit helpers
│       ├── config.rs       # Config + 5 presets + frequency derivations
│       ├── physical/
│       │   ├── qpsk.rs         # Gray-coded QPSK map/demap
│       │   ├── preamble.rs     # Preamble gen (seed=42), cross-correlation
│       │   ├── ofdm_mod.rs     # IFFT, CP, raised-cosine, normalization
│       │   ├── ofdm_demod.rs   # Timing, channel est, DD phase tracking, CFO
│       │   └── dtmf.rs         # DTMF control codec (grid-locked decoder)
│       └── link/
│           ├── scrambler.rs    # ChaCha12 XOR (seed=12345)
│           ├── crc.rs          # CRC-32 (Ethernet/ZIP polynomial)
│           ├── fec.rs          # RS(255,223) via reed-solomon crate
│           └── frame.rs        # FrameAssembler, FrameParser (sync+RS+CRC)
│
├── sosw-cli/               # Desktop CLI (cpal audio)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs         # CLI: list-devices, tx, rx, test
│       ├── ota_validate.rs # OTA validation binary (preset/layout/differential)
│       └── bin/            # dtmf-tx, dtmf-rx (standalone DTMF tools)
│
├── sosw-web/               # WASM web app (Leptos)
│   ├── Cargo.toml
│   ├── index.html
│   └── src/
│       ├── lib.rs          # App shell with 3 tabs
│       ├── rx.rs           # RX panel (capture, waterfall, stats)
│       ├── tx.rs           # TX panel (file upload, playback)
│       ├── debug.rs        # Debug tab (config, monitor, loopback)
│       ├── audio.rs        # AudioWorklet RX, AudioBuffer TX
│       ├── waterfall.rs    # Real-time spectrogram + freq axis
│       └── presets.rs      # Presets + localStorage persistence
│
├── sosw-tap/               # Ethernet-over-sound
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs          # Module declarations
│       ├── main.rs         # CLI: serve, list-devices
│       ├── tap.rs          # TAP device wrapper (tappers crate)
│       ├── phy.rs          # Audio I/O + OFDM modem bridge
│       ├── mac.rs          # CSMA/CA state machine
│       ├── link.rs         # DTMF link-training handshake framing
│       ├── fragment.rs     # Ethernet fragmentation/reassembly
│       └── bin/            # sosw_link, link_tx/rx/gen/probe, data_rx,
│                           # frame_tx, sc_loopback, ota/phy/latency tests
│
├── src/                    # Legacy Python implementation (M-FSK + OFDM)
│   ├── config.py
│   ├── main.py
│   ├── audio/              # Audio I/O (sounddevice/PortAudio)
│   ├── physical/           # M-FSK modulator/demodulator, OFDM
│   ├── link/               # CRC, FEC, framing
│   ├── application/        # File transfer protocol
│   └── ui/                 # PyQtGraph GUI (legacy)
│
├── examples/               # Python example scripts
├── tests/                  # Python tests
├── pyproject.toml
└── requirements.txt
```

## Legacy Python Implementation

The `src/` directory contains the original Python implementation (M-FSK **and**
OFDM modulation, PyQtGraph GUI, file transfer protocol). It is preserved for
reference; the Rust implementation in `sosw-core` is the active target.

The Python OFDM modem is what `examples/demo_pipeline.py` (the historical
open-air video demo) drives. As of 2026-09-19 that pipeline decodes **0 bytes
over the air** via the USB mic (`--tx-device analog-stereo --rx-device
USB_PnP`) while decoding 2811 bps through the monitor — i.e. it fails exactly
like the Rust modem in the current setup (see
[Link Status](#link-status-measured-2026-09-19)).

```bash
# Python setup
python -m venv .venv && source .venv/bin/activate
pip install -r requirements.txt

# GUI mode
python -m src.main

# Headless loopback test
python -m src.main --headless

# Python unit tests
python -m pytest tests/ -v
```

Python config parameters (historical):

| Parameter | Default | Description |
|-----------|---------|-------------|
| `sample_rate` | 48000 | Audio sample rate (Hz) |
| `baud_rate` | 1000 | M-FSK symbols per second |
| `m_fsk` | 4 | FSK tones (2/4/8/16) |
| `fec_nsym` | 32 | RS parity symbols |

## License

MIT
