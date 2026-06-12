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

| Preset | FFT | CP | SC Range | Freq Range | Sym Rate | Bitrate |
|--------|-----|----|----------|------------|----------|---------|
| Default | 256 | 32 | 10–69 | 1.9–12.9 kHz | 167 Hz | 20 kbps |
| High Baud | 128 | 16 | 4–31 | 1.5–11.6 kHz | 333 Hz | 18.7 kbps |
| Robust | 512 | 64 | 20–120 | 1.9–11.3 kHz | 83 Hz | 16.8 kbps |
| Ultrasonic | 256 | 32 | 80–120 | 15.0–22.5 kHz | 167 Hz | 13.7 kbps |
| Ultrawide | 256 | 16 | 5–110 | 0.9–20.6 kHz | 176 Hz | 37.4 kbps |

All presets verified by software roundtrip test (`param_sweep.rs`).

## Debug Tab Features

- **Config Panel**: Preset buttons, FFT/CP/SC sliders, threshold/amp/PLL controls, derived readout
- **RX Monitor**: Frames (total/valid/dropped), preamble peak, CFO, |H| mean, FPS, CRC/FEC fails, per-subcarrier channel magnitude bar chart, spectrogram with active band overlay + frequency axis
- **Test / Loopback**: Continuous test signal generation, looped playback for cross-device RX quality assessment
- **Config persistence**: Settings saved to localStorage automatically
- **FrameParser integration**: CRC and FEC validation on received frames

## Frequency Response (Measured OTA)

Tested with ALC1220 analog speaker output → USB PnP Audio Device mic.

| Band | Freq Range | Valid % | Peak |
|------|-----------|---------|------|
| 188–5,812 Hz | SC 1-31 | **0%** | 0.80 (ambient noise below 2 kHz) |
| 1.9–7.5 kHz | SC 10-40 | 50% | 0.93 |
| 5.6–11.3 kHz | SC 30-60 | 60% | 0.93 |
| 9.4–15.0 kHz | SC 50-80 | 40% | 0.93 |
| 13.1–18.8 kHz | SC 70-100 | 40% | 0.79 |
| 16.9–22.5 kHz | SC 90-120 | **20%** | 0.81 |

**Best presets OTA:** `ultrawide` (90% valid) and `robust` (80% valid). Ultrasonic preset works at 20% on this hardware — expect better with phone speakers or ultrasonic transducers.

All bands have preamble peaks >0.70, confirming the USB PnP mic responds up to at least 22.5 kHz. Frame loss is from bit errors in data symbols, not preamble detection failure. The key weak spot is **below 2 kHz** (ambient noise) and **above 15 kHz** (increasing symbol error rate).

Run your own sweep: `python -m examples.ota_freq_sweep --frames 10`

## Next Steps

1. ~~**CLI crate**: cpal audio I/O for desktop TX/RX testing~~ (done)
2. ~~**WASM crate**: Leptos web app with AudioWorklet~~ (done: RX, TX, Debug tabs)
3. **OTA validation**: Compare frame reception rate vs Python baseline (100% at 30s)
4. **sosw-tap**: Ethernet-over-sound with CSMA/CA MAC
5. **WASM cross-device testing**: Validate ultrasonic presets with high-frequency hardware

## Data Flow

```
TX: Bytes → Scramble → Pack bits → QPSK map → OFDM syms (IFFT+CP) → Preamble → Audio

RX: Audio → Cross-correlation → Preamble detect → Channel est → CFO est
       → Per-sym FFT → DD phase correction → Equalization → QPSK demap
       → Unpack bits → Descramble → Bytes → FrameParser (CRC/FEC)
```
