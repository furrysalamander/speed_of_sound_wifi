# Speed of Sound WiFi — Rust Rewrite

## Architecture

```
sosw-core/     — Core library (no I/O deps)
sosw-cli/      — Desktop CLI (cpal audio)
sosw-web/      — WASM web demo (Leptos)
sosw-tap/      — Ethernet-over-sound (CSMA/CA MAC)
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

# TAP (Linux Ethernet-over-sound)
cargo build -p sosw-tap
cargo run -p sosw-tap -- serve --tap sosw0 --preset default
```

## Core Library (`sosw-core`)

| Module | File | Purpose |
|--------|------|---------|
| Config | `src/config.rs` | Data rates, OFDM params, frame config + presets |
| QPSK | `src/physical/qpsk.rs` | Gray-coded QPSK map/demap |
| Preamble | `src/physical/preamble.rs` | Preamble gen (seed=42), cross-correlation |
| OFDM Mod | `src/physical/ofdm_mod.rs` | IFFT, CP, raised-cosine, normalization |
| OFDM Demod | `src/physical/ofdm_demod.rs` | Timing, channel est, DD phase tracking |
| DTMF | `src/physical/dtmf.rs` | DTMF tone-pair encode + grid-locked decode (control channel) |
| FSK | `src/physical/fsk.rs` | Non-coherent CPFSK M-FSK: grid lock, per-tone calibration, compact RS FEC, wire framing. **The working acoustic PHY.** |
| FSK bank | `src/physical/fsk_bank.rs` | Parallel multi-tone narrowband 2-FSK across the band (Stage 4 rate scaling) |
| Scrambler | `src/link/scrambler.rs` | ChaCha12 XOR (seed=12345) |
| CRC | `src/link/crc.rs` | CRC-32 (Ethernet/ZIP polynomial) |
| RS FEC | `src/link/fec.rs` | RS(255,223) encode/decode via `reed-solomon` crate |
| Frame | `src/link/frame.rs` | FrameAssembler, FrameParser (sync + RS + CRC) |
| Traits | `src/lib.rs` | Modulator, Demodulator traits |

## TAP Crate (`sosw-tap`)

| Module | File | Purpose |
|--------|------|---------|
| CLI | `src/main.rs` | CLI: `serve`, `list-devices` |
| TAP | `src/tap.rs` | TAP device wrapper (tappers crate) |
| PHY | `src/phy.rs` | Audio I/O + OFDM modem bridge |
| MAC | `src/mac.rs` | CSMA/CA state machine |
| Link | `src/link.rs` | DTMF link-training handshake framing (SYNC/kind/node/param/CRC) |
| Audio | `src/audio.rs` | Reusable full-duplex cpal endpoint + capture file/WAV I/O |
| Transport | `src/xfer.rs` | Stop-and-wait ARQ over FSK, shared `Transport` trait, lossy sim |
| Fragment | `src/fragment.rs` | Ethernet fragmentation/reassembly |

### Acoustic tools (`sosw-tap/src/bin`)

| Tool | Purpose |
|------|---------|
| `phy_bench` | Stage 0 characterization: gain-step, gain-level, impulse, two-tone, tone-snr, latency |
| `fsk_bench` | Stage 1/2/4: software/self/tx/rx/sweep, single-stream + parallel bank, FEC, PER/headroom/goodput |
| `link_train` | Stage 3: discovery, control-channel headroom, rate negotiation (`--mode a|b|sim`) |
| `sosw_ftp` | Stage 5: stop-and-wait ARQ file transfer (`--mode send|recv|sim`) |

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

```
sosw-core lib (dtmf) ............. 7 passed
sosw-core lib (fsk) ............. 14 passed  FSK M-FSK, FEC, calibration, framing
sosw-core lib (fsk_bank) ......... 3 passed  parallel multi-tone bank
tests/crc_tests.rs ............... 9 passed
tests/scrambler_tests.rs ......... 9 passed
tests/fec_tests.rs ............... 5 passed
tests/fec_extended_tests.rs ..... 8 passed
tests/framing_tests.rs ........... 5 passed
tests/framing_extended_tests.rs . 20 passed
tests/ofdm_roundtrip.rs .......... 4 passed
tests/param_sweep.rs ............ 4 passed  (software sweep: FFT 128-512, CP 16-64, SC 1-127)
tests/debug_fft.rs .............. 1 passed
tests/debug_preamble.rs ......... 1 passed
sosw-tap lib (link/mac/fragment) 16 passed
sosw-tap lib (xfer) .............. 3 passed  ARQ over a simulated lossy channel
Total: 106 tests  (1 mock_tap_loopback test ignored)
```

The `dtmf` and `link` tests cover grid-locked DTMF decoding (short symbols,
capture offset, noise rejection) and link-frame recovery with dropped,
inserted, and substituted symbols at every position. The `fsk` tests cover
clean/differential roundtrips for M=2/4/8/16, offset+noise, multipath with
per-tone calibration, compact RS FEC, and chunked streaming. The `xfer` tests
cover stop-and-wait ARQ over clean and 25%-drop channels.

**Acoustic link status (2026-09-20):** a non-coherent **M-FSK** PHY now works
over the air. `sosw_ftp` transferred a 31-byte file **cross-machine
giratina→deoxys byte-exact** (FSK + RS FEC + CRC + stop-and-wait ARQ). Self-
loopback decodes at single-stream M=2/3ms (~130 bps effective) and a 16-channel
parallel bank (~210 bps effective). Acoustic latency is ~3.8 s and the channel's
coherent span is ~350 Hz; the channel is time-varying, so margins fluctuate and
FEC/ARQ are required. See `README.md` → "Acoustic Link Proven" and
`docs/channel-report.md`.

Coherent wideband **OFDM** (and time-differential OFDM / SC-QPSK) still do
**not** decode acoustically in this setup; older "100% RS-correctable" OFDM
tables are **historical and not reproducible now**. The DTMF control channel
works cross-machine. The staged plan and its per-stage status are in
`docs/link-development-plan.md` (Stages 0–5 now implemented).

## Config Presets

`data_symbols_per_frame` is matched to full FrameAssembler output (sync + RS-FEC + CRC, exact fit).

| Preset | FFT | CP | SC Range | Freq Range | Sym/s | Syms/Frame | Frame Dur | Bitrate |
|--------|-----|----|----------|------------|-------|------------|-----------|---------|
| Default | 256 | 32 | 10–69 | 1.9–12.9 kHz | 167 | 35 | 258 ms | 20 kbps |
| High Baud | 128 | 16 | 4–31 | 1.5–11.6 kHz | 333 | 39 | 141 ms | 18.7 kbps |
| Robust | 512 | 64 | 20–120 | 1.9–11.3 kHz | 83 | 21 | 348 ms | 16.8 kbps |
| Ultrasonic | 256 | 32 | 80–120 | 15.0–22.5 kHz | 167 | 51 | 354 ms | 13.7 kbps |
| Ultrawide | 256 | 16 | 5–110 | 0.9–20.6 kHz | 176 | 20 | 159 ms | 37.4 kbps |

All presets pass the software roundtrip test (`param_sweep.rs`). As of 2026-09-19
the digital (monitor) loopback passes at 100% but the acoustic OTA loopback does
not decode; the prior OTA verification of these presets is **historical and not
reproducible** in the current setup (see "OTA Validation" below).

`Config` also has a `differential: bool` field (serde-defaulted) that switches to
**time-differential** OFDM: data rides in the phase difference between consecutive
OFDM symbols on the same subcarrier, so a static channel needs no absolute
estimate. `with_layout(fft, cp, sc_min, sc_max, rs_nsym, payload)` rebuilds a
layout and auto-fits `data_symbols_per_frame`. `ota-validate` exposes both via
`--differential` and the layout flags.

## Debug Tab Features

- **Config Panel**: Preset buttons, FFT/CP/SC sliders, threshold/amp/PLL controls, derived readout
- **RX Monitor**: Frames (total/valid/dropped), preamble peak, CFO, |H| mean, FPS, CRC/FEC fails, per-subcarrier channel magnitude bar chart, spectrogram with active band overlay + frequency axis
- **Test / Loopback**: Continuous test signal generation, looped playback for cross-device RX quality assessment
- **Config persistence**: Settings saved to localStorage automatically
- **FrameParser integration**: CRC and FEC validation on received frames

## OTA Validation (Rust)

### Current status (2026-09-19)

- **Digital** loopback (each machine's output → its own PipeWire monitor), 20
  frames: **100% RS-correctable** on both giratina and deoxys.
- **Software** loopback (`ota-validate --snr-db <n>`): **100%**.
- **Acoustic** loopback and cross-machine, both directions: **0% RS-correctable**
  across tested levels (sink 0.30–1.00, TX gain 1–18), all presets, and CP
  32–4800. Time-differential OFDM and single-carrier QPSK also fail acoustically
  while passing digitally.
- The Python reference (`examples/demo_pipeline`) fails over the air now (0
  bytes) and passes via the monitor (2811 bps).
- Measured: acoustic per-subcarrier SNR 3.1/3.2/3.2 dB at TX gains 2/4/6 (does
  not improve with level); capture transfer curve linear to input ≈0.1 then
  saturating at ≈1.16; tone sweep carries 2–22 kHz at ≥21 dB SNR on all four
  paths (≥30 dB at nearly every tone); clock offset 6.4 ppm. See `README.md` →
  "Link Status" for tables.

### Historical results (June 2026 — not reproducible)

The table below was recorded in June 2026 and is retained for reference. It is
**not reproducible now** in the current physical setup, with either modem.

| Preset | SC Range | RS-Correctable | Notes |
|--------|----------|---------------|-------|
| Default | 10–69 | **100%** | 30/30, <13 err/frame |
| High Baud | 4–31 | **100%** | 30/30, <8 err/frame |
| Robust | 20–120 | **100%** | 30/30, <10 err/frame |
| Ultrasonic | 80–120 | **100%** | 30/30, <6 err/frame |
| Ultrawide | 5–110 | **100%** | 30/30, <18 err/frame |

### Key Fixes

- **Mask-padding**: Zero-bit padding caused all active subcarriers at (+1,+1), producing massive IFFT peaks that crushed normalization gain; replaced with scrambler-mask padding for normal PAPR
- **Guard interval**: `sym_dur` silence between frames prevents cross-correlation false peaks from previous frame's random tail data
- **`consumed_samples` stride**: Converges preamble offset to ~`guard` samples after 3 training frames, matching actual audio spacing with zero drift
- **Training frames**: 3 dummy frames before data allow `consumed_samples` stride to converge (coarse energy search triggers on lead-in ambient noise, causing false preamble offset on frame 0)
- **FrameParser CRC re-check**: `try_extract_frame` used raw (potentially errored) sync bytes in the re-encode CRC check after RS decode. Sync bytes from the first OFDM data symbol can have bit errors even with |H|=0.75. Fixed by using known `SYNC_PATTERN` constant instead of `&self.buffer[..sync_end]`.
- **Phy API `rx_skip`**: Persistent audio streams cause `energy_coarse_search` to false-trigger on ambient noise at the batch front, missing the preamble. Drain only `preamble_samples/4` (not `consumed`) on invalid frames so the real echo stays in the batch until detected. Batch cap raised to `frame_samples * 200` to prevent small-frame presets from flushing the echo.
- **`data_symbols_per_frame`**: Exact-fit to FrameAssembler output for all presets (was over/under-provisioned for 4 of 5 presets)
- **RS budget**: Correctly uses `rs_nsym/2` instead of hardcoded 16 per block
- **Chunk margin**: `frame_samples + 6×guard` ensures first frame's preamble offset (~1440 samples / 30ms audio latency) doesn't truncate data symbols

Run your own sweep: `cargo run --release -p sosw-cli --bin ota-validate -- --preset <name> --frames 20 --tx-device <tx> --rx-device <rx>`

## Data Flow

```
TX: Bytes → Scramble → Pack bits → QPSK map → OFDM syms (IFFT+CP) → Preamble → Audio

RX: Audio → Cross-correlation → Preamble detect → Channel est → CFO est
       → Per-sym FFT → DD phase correction → Equalization → QPSK demap
       → Unpack bits → Descramble → Bytes → FrameParser (CRC/FEC)
```
