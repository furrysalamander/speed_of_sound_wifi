# Speed of Sound WiFi — Rust Rewrite

## Architecture

```
sosw-core/     — Core library (no I/O deps)
sosw-cli/      — Desktop CLI (cpal audio)
sosw-web/      — WASM web demo (Leptos)
```

## Build & Test

```bash
# Core library
cargo build -p sosw-core
cargo test -p sosw-core

# CLI (desktop)
cargo build -p sosw-cli
cargo run -p sosw-cli -- --help

# Web (WASM)
cd sosw-web && trunk serve
cd sosw-web && trunk build --release
```

## Core Library (`sosw-core`)

| Module | File | Purpose |
|--------|------|---------|
| Config | `src/config.rs` | Data rates, OFDM params, frame config + presets |
| QPSK | `src/physical/qpsk.rs` | Gray-coded QPSK map/demap |
| Preamble | `src/physical/preamble.rs` | Preamble gen (seed=42), cross-correlation |
| OFDM Mod | `src/physical/ofdm_mod.rs` | IFFT, CP, raised-cosine, normalization |
| OFDM Demod | `src/physical/ofdm_demod.rs` | Timing, channel est, DD phase tracking |
| Scrambler | `src/link/scrambler.rs` | ChaCha12 XOR (seed=12345) |
| CRC | `src/link/crc.rs` | CRC-32 (Ethernet/ZIP polynomial) |
| RS FEC | `src/link/fec.rs` | RS(255,223) encode/decode via `reed-solomon` crate |
| Frame | `src/link/frame.rs` | FrameAssembler, FrameParser (sync + RS + CRC) |
| Traits | `src/lib.rs` | Modulator, Demodulator traits |

## WASM Web App (`sosw-web`)

| File | Purpose |
|------|---------|
| `lib.rs` | App shell with 3 tabs (RX, TX, Debug) |
| `rx.rs` | Simple RX demo panel (start/stop, basic stats, waterfall) |
| `tx.rs` | Simple TX demo panel (file upload, modulate, playback) |
| `debug.rs` | **Debug tab**: config controls, enhanced monitoring, loopback testing |
| `audio.rs` | AudioWorklet RX, AudioBuffer TX, test signal builder |
| `waterfall.rs` | Real-time spectrogram with OFDM band overlay + freq axis |
| `presets.rs` | Config presets + localStorage persistence |

## Test Results

```bash
tests/fec_tests.rs ................ 5 passed
tests/framing_tests.rs ............ 5 passed
tests/ofdm_roundtrip.rs ........... 4 passed
tests/crc_tests.rs ............... 9 passed
tests/scrambler_tests.rs ......... 9 passed
tests/fec_extended_tests.rs ...... 8 passed
tests/framing_extended_tests.py .. 20 passed
tests/param_sweep.rs ............. 4 passed  (software sweep: FFT 128-512, CP 16-64, SC 1-127)
tests/debug_fft.rs ............... 1 passed
tests/debug_preamble.rs .......... 1 passed
Total: 66 tests
```

## Config Presets

`data_symbols_per_frame` is matched to full FrameAssembler output (sync + RS-FEC + CRC, exact fit).

| Preset | FFT | CP | SC Range | Freq Range | Sym/s | Syms/Frame | Frame Dur | Bitrate |
|--------|-----|----|----------|------------|-------|------------|-----------|---------|
| Default | 256 | 32 | 10–69 | 1.9–12.9 kHz | 167 | 35 | 258 ms | 20 kbps |
| High Baud | 128 | 16 | 4–31 | 1.5–11.6 kHz | 333 | 39 | 141 ms | 18.7 kbps |
| Robust | 512 | 64 | 20–120 | 1.9–11.3 kHz | 83 | 21 | 348 ms | 16.8 kbps |
| Ultrasonic | 256 | 32 | 80–120 | 15.0–22.5 kHz | 167 | 51 | 354 ms | 13.7 kbps |
| Ultrawide | 256 | 16 | 5–110 | 0.9–20.6 kHz | 176 | 20 | 159 ms | 37.4 kbps |

All presets verified by software roundtrip test (`param_sweep.rs`) and OTA validation (`ota-validate`).

## Debug Tab Features

- **Config Panel**: Preset buttons, FFT/CP/SC sliders, threshold/amp/PLL controls, derived readout
- **RX Monitor**: Frames (total/valid/dropped), preamble peak, CFO, |H| mean, FPS, CRC/FEC fails, per-subcarrier channel magnitude bar chart, spectrogram with active band overlay + frequency axis
- **Test / Loopback**: Continuous test signal generation, looped playback for cross-device RX quality assessment
- **Config persistence**: Settings saved to localStorage automatically
- **FrameParser integration**: CRC and FEC validation on received frames

## OTA Validation (Rust)

Tested with ALC1220 analog speaker output → USB PnP Audio Device mic (loopback, frames separated by `sym_dur` silence). 30 frames per preset, `consumed_samples` stride for alignment convergence.

| Preset | SC Range | RS-Correctable | Notes |
|--------|----------|---------------|-------|
| Default | 10–69 | **100%** | 30/30, <13 err/frame |
| High Baud | 4–31 | **100%** | 30/30, <8 err/frame |
| Robust | 20–120 | **100%** | 30/30, <10 err/frame |
| Ultrasonic | 80–120 | **100%** | 30/30, <6 err/frame |
| Ultrawide | 5–110 | **100%** | 30/30, <18 err/frame |

All 5 presets verified with OTA loopback — **100% RS-correctable** on 30-frame runs.

### Key Fixes

- **Mask-padding**: Zero-bit padding caused all active subcarriers at (+1,+1), producing massive IFFT peaks that crushed normalization gain; replaced with scrambler-mask padding for normal PAPR
- **Guard interval**: `sym_dur` silence between frames prevents cross-correlation false peaks from previous frame's random tail data
- **`consumed_samples` stride**: Converges preamble offset to ~`guard` samples after 3 training frames, matching actual audio spacing with zero drift
- **Training frames**: 3 dummy frames before data allow `consumed_samples` stride to converge (coarse energy search triggers on lead-in ambient noise, causing false preamble offset on frame 0)
- **`data_symbols_per_frame`**: Exact-fit to FrameAssembler output for all presets (was over/under-provisioned for 4 of 5 presets)
- **RS budget**: Correctly uses `rs_nsym/2` instead of hardcoded 16 per block
- **Chunk margin**: `frame_samples + 6×guard` ensures first frame's preamble offset (~576 sample audio latency) doesn't truncate data symbols

Run your own sweep: `cargo run --release -p sosw-cli --bin ota-validate -- --preset <name> --frames 20 --tx-device <tx> --rx-device <rx>`

### OTA Throughput Analysis

| Preset | PHY Rate | Frame Size | Frame Dur | Ethernet Goodput (est.) |
|--------|----------|------------|-----------|------------------------|
| Ultrawide | 37.4 kbps | 442 B | 159 ms | ~2 kbps |
| High Baud | 18.7 kbps | 128 B | 141 ms | ~2 kbps |

MAC timings (300 ms DIFS, 60 ms slots, 700 ms ACK timeout) are the real bottleneck, not PHY capacity.

## Next Steps

1. ~~**CLI crate**: cpal audio I/O for desktop TX/RX testing~~ (done)
2. ~~**WASM crate**: Leptos web app with AudioWorklet~~ (done: RX, TX, Debug tabs)
3. ~~**OTA validation**: Rust OTA matches Python baseline; all 5 presets verified~~ (done)
4. **sosw-tap**: Ethernet-over-sound with CSMA/CA MAC
5. **WASM cross-device testing**: Validate ultrasonic presets with high-frequency hardware

## Data Flow

```
TX: Bytes → Scramble → Pack bits → QPSK map → OFDM syms (IFFT+CP) → Preamble → Audio

RX: Audio → Cross-correlation → Preamble detect → Channel est → CFO est
       → Per-sym FFT → DD phase correction → Equalization → QPSK demap
       → Unpack bits → Descramble → Bytes → FrameParser (CRC/FEC)
```
