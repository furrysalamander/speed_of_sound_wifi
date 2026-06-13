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
    → Decision-directed phase tracking → Equalization
    → QPSK demap → Unpack bits → Descramble → Bytes
    → FrameParser (sync → RS(255,223) FEC → CRC-32 verify)
```

### Crate Map

| Crate | Purpose | Key Dependencies |
|-------|---------|-----------------|
| `sosw-core` | Core modem library (no I/O) | `rustfft`, `ndarray`, `reed-solomon`, `serde` |
| `sosw-cli` | Desktop CLI (TX/RX/test/ota-validate) | `sosw-core`, `clap`, `cpal` |
| `sosw-web` | WASM browser app (RX/TX/Debug tabs) | `sosw-core`, `leptos`, `web-sys` |
| `sosw-tap` | Ethernet-over-sound (CSMA/CA MAC) | `sosw-core`, `tappers`, `cpal`, `tokio` |

## Config Presets

All presets verified OTA: **100% RS-correctable** on 30-frame loopback runs (ALC1220 speaker → USB PnP mic).

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

All 5 presets verified at 100% RS-correctable (30 frames each) on ALC1220 speaker → USB PnP mic loopback.

## Ethernet-over-Sound (`sosw-tap`)

Bridges a Linux TAP interface to the OFDM audio modem. Shared-medium Ethernet with CSMA/CA — all devices communicate over sound.

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

- **CSMA/CA**: DIFS (300 ms), random backoff (4–64 slots × 60 ms), ACK timeout (700 ms)
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
# Rust unit tests (66+ tests)
cargo test -p sosw-core

# Software parameter sweep (FFT 128–512, CP 16–64, SC 1–127)
cargo test -p sosw-core --test param_sweep -- --nocapture

# OTA hardware validation
cargo run --release -p sosw-cli --bin ota-validate -- --preset default --frames 20

# Python tests (legacy)
python -m pytest tests/ -v
```

### Test Coverage

| Module | Tests | Description |
|--------|-------|-------------|
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
| `fragment` | 6 | sosw-tap Ethernet fragmentation tests |
| **Total** | **72** | |

## Project Structure

```
speed_of_sound_wifi/
├── Cargo.toml              # Workspace root (4 members)
├── README.md
├── AGENTS.md               # Development guide
├── PLAN.md                 # sosw-tap design document
├── docs/
│   └── frequency_sweep.md  # Hardware frequency sweep analysis
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
│       │   └── ofdm_demod.rs   # Timing, channel est, DD phase tracking, CFO
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
│       └── ota_validate.rs # OTA validation binary
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
│       └── fragment.rs     # Ethernet fragmentation/reassembly
│
├── src/                    # Legacy Python implementation (M-FSK)
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

The `src/` directory contains the original Python implementation (M-FSK modulation, PyQtGraph GUI, file transfer protocol). This codebase is preserved for reference but is no longer the active development target. The Rust OFDM implementation in `sosw-core` is the canonical modem.

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
