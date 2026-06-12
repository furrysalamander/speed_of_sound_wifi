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
| Config | `src/config.rs` | Data rates, OFDM params, frame config |
| QPSK | `src/physical/qpsk.rs` | Gray-coded QPSK map/demap |
| Preamble | `src/physical/preamble.rs` | Preamble gen (seed=42), cross-correlation |
| OFDM Mod | `src/physical/ofdm_mod.rs` | IFFT, CP, raised-cosine, normalization |
| OFDM Demod | `src/physical/ofdm_demod.rs` | Timing, channel est, DD phase tracking |
| Scrambler | `src/link/scrambler.rs` | ChaCha12 XOR (seed=12345) |
| CRC | `src/link/crc.rs` | CRC-32 (Ethernet/ZIP polynomial) |
| RS FEC | `src/link/fec.rs` | RS(255,223) encode/decode via `reed-solomon` crate |
| Frame | `src/link/frame.rs` | FrameAssembler, FrameParser (sync + RS + CRC) |
| Traits | `src/lib.rs` | Modulator, Demodulator traits |

## Test Results (14 passing)

```bash
tests/fec_tests.rs ......... 5 passed
tests/framing_tests.rs ..... 5 passed  
tests/ofdm_roundtrip.rs .... 4 passed
```

## Current OFDM Parameters

| Parameter | Value |
|-----------|-------|
| FFT size | 256 |
| CP length | 32 |
| Active subcarriers | 10-69 (60 total) |
| Modulation | QPSK (2 bits/subcarrier) |
| Preamble | 8 OFDM symbols (seed=42) |
| Scrambler | ChaCha12 PRNG (seed=12345) |
| Symbol duration | 288 samples (6 ms at 48 kHz) |
| Data syms/frame | Variable (up to 35, computed from payload) |
| Frame time | ~258 ms (8 preamble + 35 data at 166.7 Hz) |
| Payload per frame | 442 B (2 RS blocks, CRC-32) |
| Preamble threshold | 0.05 (normalized cross-correlation) |

## Next Steps

1. **CLI crate**: cpal audio I/O for desktop TX/RX testing
2. **WASM crate**: Leptos web app with AudioWorklet/ScriptProcessorNode
3. **OTA validation**: Compare frame reception rate vs Python baseline (100% at 30s)
4. **Signal quality test mode**: Preamble peak, CFO, per-SC channel estimate display

## Data Flow

```
TX: Bytes → Scramble → Pack bits → QPSK map → OFDM syms (IFFT+CP) → Preamble → Audio

RX: Audio → Cross-correlation → Preamble detect → Channel est → CFO est
       → Per-sym FFT → DD phase correction → Equalization → QPSK demap
       → Unpack bits → Descramble → Bytes
```
