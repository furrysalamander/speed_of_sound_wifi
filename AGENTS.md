# Loopback Testing Guide

## Hardware Setup

Connect a 3.5mm TRS male-to-male audio cable between the computer's **output** (speaker/headphone) and **input** (microphone/line-in) jacks. Two variants:
- **Motherboard line-in** (flat response, up to 2000 baud reliable)
- **USB PnP Audio Device** (0c76 vendor, index 19, dip at 1200-1500 Hz, otherwise flat 200-8000+ Hz)

### Find Device Indices

```bash
python -m src.main --list-devices
```

Look for the motherboard's analog I/O (typically "analog-stereo" entries). Example output:

```
18  alsa_output.pci-0000_71_00.6.analog-stereo  0  2  48000  output
23  alsa_input.pci-0000_71_00.6.analog-stereo   2  0  48000  input
```

## Running Tests

### Quick Smoke Test

```bash
PYTHONPATH=. python tests/test_loopback.py -i 23 -o 18 --payload-size 64
```

### Baud Rate Sweep

```bash
PYTHONPATH=. python tests/test_loopback.py -i 23 -o 18 --payload-size 1024 \
  --sweep-baud 500 1000 2000 3000 5000 --output-csv results.csv
```

### M-FSK Sweep

```bash
PYTHONPATH=. python tests/test_loopback.py -i 23 -o 18 --payload-size 1024 \
  --baud-rate 1000 --sweep-mfsk 2 4 8 16 --output-csv results.csv
```

### Via main.py (CLI)

```bash
python -m src.main --loopback --input-device 23 --output-device 18 \
  --payload-size 1024 --baud-rate 1000 --m-fsk 4
```

### OFDM Mode

```bash
python tests/test_loopback.py -i 19 -o 17 --ofdm --payload-size 4096
```

## Loopback Test Results (Motherboard Line-In)

### Baud Rate Sweep (4-FSK, FEC on, 1024B payload)

| Baud | M-FSK | FEC  | Sync? | Valid | Recv'd | Throughput |
|------|-------|------|-------|-------|--------|------------|
| 500  | 4     | on   | yes   | 1     | 1024   | 633 bps    |
| 1000 | 4     | on   | yes   | 1     | 1024   | 1059 bps   |
| 2000 | 4     | on   | yes   | 1     | 1024   | 1598 bps   |
| 2200 | 4     | on   | yes   | 0     | 0      | 0 bps      |

### M-FSK Sweep (1000 baud, FEC on, 1024B payload)

| M-FSK | Sync? | Valid | Recv'd | Throughput |
|-------|-------|-------|--------|------------|
| 2     | yes   | 1     | 1024   | 636 bps    |
| 4     | yes   | 1     | 1024   | 1059 bps   |
| 8     | yes   | 1     | 1024   | 1361 bps   |
| 16    | yes   | 0     | 0      | 0 bps      |

## Loopback Test Results (USB PnP Audio Device)

### Optimal 4-FSK Config

| Baud | M-FSK | Payload | Pass Rate | Throughput |
|------|-------|---------|-----------|------------|
| 500  | 4     | 128 B   | ~100%     | 214 bps    |
| 750  | 4     | 128 B   | ~67%      | 254 bps    |
| 750  | 4     | 256 B   | ~33%      | 380 bps    |

### Success Parameters (2-FSK, 500 baud, 1500-5500 Hz)

This config works reliably (64-byte payload, 74 bps):
- **Goertzel detection** (DTFT at exact tone frequencies) instead of FFT bins
- **Purity-based symbol alignment**: brute-force search maximizes `e0/e1_cross * e1/e0_cross`
- **FEC-before-CRC**: Reed-Solomon correction applied before CRC verification
- **Sync threshold**: 4/8 byte matches (soft sync)
- **ADC notch**: DC offset removal before Goertzel
- **Frequencies**: 3000-9000 Hz range (avoiding 1200-1500 Hz USB mic dip)

### USB Mic Frequency Response (Normalized)

```
200-1200 Hz:  flat (±10%)
1200-1500 Hz: dip to ~50%  ← AVOID
1500-8000 Hz: flat (±10%)
8000+ Hz:    rolls off (present but weaker)
```

### Key Improvements Made

1. **Goertzel energy detection** (`_goertzel_detect` in `demodulator.py`): Computes energy at exact tone frequencies via Hann-windowed DTFT. Much better selectivity than FFT-bin-based methods for short symbol lengths.

2. **Purity-based alignment refinement**: After coarse preamble detection, brute-force searches ±1 symbol for the offset maximizing `(e_tone0 / e_toneM1) * (e_toneM1 / e_tone0)` for adjacent symbols.

3. **FEC-before-CRC** (`framing.py`): Applies RS decoding before CRC verification. Previously CRC was verified on raw data, rejecting frames before FEC could correct errors.

4. **Soft sync** with lowered threshold (4/8 bytes): Tolerates bit errors in sync pattern.

## OFDM Implementation

### Parameters (current defaults)

| Parameter | Value | Notes |
|-----------|-------|-------|
| FFT size | 256 | 187.5 Hz subcarrier spacing |
| CP length | 128 samples | ~2.7 ms guard interval (longer for better timing margin) |
| Active subcarriers | 13-42 (30 total) | 2438-7875 Hz (avoids 2250 Hz dip on USB mic) |
| Subcarrier modulation | BPSK (1 bit) | Preamble uses QPSK for channel estimation |
| Preamble | 4 OFDM symbols | Known QPSK (more symbols = better channel estimate) |
| Symbol duration | 384 samples (8 ms) | CP + FFT body |
| Symbol rate | 125 Hz | 48000/384 |

### Throughput Estimate

```
Raw:         30 subcarriers × 1 bit × 125 Hz = 3,750 bps
RS-FEC (32): 3,750 × (223/255)              = 3,279 bps
Framing:     3,279 × (223/271)              ≈ 2,699 bps
```

### Per-Subcarrier Error Profile (USB Mic, Loopback)

| SC Index | Freq | Error Rate | Notes |
|----------|------|------------|-------|
| 5 (idx 18) | 3375 Hz | ~2.3% | Occasional errors |
| 15 (idx 28) | 5250 Hz | ~0.9% | Minor |
| 18 (idx 31) | 5812 Hz | ~0.6% | Minor |
| 19 (idx 32) | 6000 Hz | ~1.2% | Minor |
| 27 (idx 40) | 7500 Hz | ~5.8% | Worst — near roll-off |
| 28 (idx 41) | 7688 Hz | ~8.5% | Worst — near roll-off |

Subcarriers 12 (2250 Hz, FFT index 22) consistently had ~82% error rate from bad QPSK preamble channel estimate. Subcarriers 27/28 (7500-7688 Hz) have higher errors due to timing drift phase rotation amplified by high frequency.

### Phase Tracking

**Decision-directed phase tracking** (`_dd_common`, `_dd_slope` in OfdmDemodulator):
- After equalization and BPSK demap, re-encodes decisions and estimates residual phase error per subcarrier
- Fits linear model (phase = a + b*k, where k = FFT index) to distinguish common phase drift from timing offset
- Exponential smoothing (alpha=0.05) filters noisy estimates
- Correction applied to next symbol as `exp(-j*(a + b*k))`

Without DD tracking, block 4 of 5 (last ~68 symbols of 344 total) accumulated ~1.6 samples timing drift, causing 49 byte errors. With DD tracking, block 4 errors dropped to 6-8 (within RS-32 correction limit of 16).

### Timing Recovery

Uses **preamble cross-correlation** (not CP autocorrelation):
- Demodulator stores the full time-domain preamble (generated identically to modulator)
- Cross-correlates with incoming samples to find preamble start
- Provides accurate timing even with the raised-cosine onset ramp on the first CP

Channel estimation uses zero-forcing from the 4 known preamble symbols.
One-tap equalization per subcarrier for data symbols.

## QPSK + Pilot Subcarriers

### Implementation (June 2026)

OFDM modem now supports **QPSK data modulation** (2 bits/subcarrier) with **pilot subcarriers** for robust phase tracking:

| Feature | Description |
|---------|-------------|
| Data modulation | QPSK (Gray-coded), configurable via `bits_per_subcarrier` |
| Pilot subcarriers | 4 pilots (indices 0, 10, 20, 29 within active range) carry known QPSK `(1+j)/√2` symbols |
| Pilot overhead | 4 pilot subcarriers out of 30 active = 52 data bits/symbol (vs 60 raw) |
| Phase tracking | DD tracking boosted: pilot positions get `max(weight, 2.0)` confidence in weighted LS fit |
| Pilot symbol override | Demod re-encodes decisions for all SC, but overrides pilot positions with known symbols |
| Backward compat | BPSK mode still works with or without pilots |

### Throughput (QPSK, 30 SC, CP=128)

```
Raw:           30 SC × 2 bits × 125 Hz = 7,500 bps
Pilot overhead: 26 data SC × 2 bits × 125 Hz = 6,500 bps
RS-FEC (32):   6,500 × (223/255)        = 5,684 bps
Framing:       5,684 × (223/271)        ≈ 4,679 bps
```

Still below the 8,506 bps target. Next steps: `cp_length=64` (150 Hz symbol rate), then subcarrier expansion.

### Software Round-Trip (Verified)

```bash
# QPSK with pilots
/venv/bin/python -c "
from src.config import Config; from src.physical.ofdm import *
c=Config(); c.ofdm.bits_per_subcarrier=2; c.ofdm.pilot_subcarriers=(0,10,20,29)
audio=OfdmModulator(c).modulate_with_preamble(bytes(range(100)))
bits=OfdmDemodulator(c).process_samples(audio)
print('Match:', OfdmDemodulator(c).symbols_to_bytes(bits, 100)[:100]==bytes(range(100)))
"

# BPSK still works too (with or without pilots)
```

## Hardware Test Results (USB PnP, Loopback)

| Payload | Valid Frames | Block Errors | Throughput |
|---------|-------------|--------------|------------|
| 256 B   | 5/5         | All ≤16      | ~560 bps*  |
| 1024 B  | 5/5         | All ≤8       | ~2940 bps* |

* Throughput measured as payload / over-the-air time (excludes startup/silence).

## Commands Summary

```bash
# Unit tests (no hardware needed)
python -m pytest tests/ -v

# Software-only round-trip
python -m src.main --headless

# Hardware loopback (USB mic, FSK)
python tests/test_loopback.py --input-name USB_PnP --output-name analog-stereo \
  --payload-size 64 --baud-rate 500 --freq-min 3000 --freq-max 7000 --m-fsk 2

# Hardware loopback (USB mic, 4-FSK)
python tests/test_loopback.py --input-name USB_PnP --output-name analog-stereo \
  --payload-size 128 --baud-rate 500 --freq-min 3000 --freq-max 9000 --m-fsk 4

# Hardware loopback (USB mic, OFDM)
python tests/test_loopback.py --input-name USB_PnP --output-name analog-stereo \
  --ofdm --payload-size 1024

# CLI loopback mode (OFDM)
python -m src.main --loopback --input-name USB_PnP --output-name analog-stereo \
  --ofdm --payload-size 1024 --output-csv results.csv

# CLI loopback mode (FSK)
python -m src.main --loopback --input-name USB_PnP --output-name analog-stereo \
  --payload-size 1024 --baud-rate 2000 --m-fsk 8 --output-csv results.csv

# List audio devices
python -m src.main --list-devices

# Single-burst streaming test (OTA, USB mic)
python examples/test_streaming.py --input /tmp/test.bin \
  --input-name USB_PnP --output-name analog-stereo \
  --frames-per-burst 10

# Multi-burst pipe test (subprocess, OTA)
python examples/test_stream_pipe.py

# OTA demo pipeline (RX -> ffplay)
python -m examples.demo_pipeline --input /tmp/shrek_10f.bin

# Single-burst TX only
python -m examples.demo_tx --input /tmp/test.bin --input-name analog-stereo

# Single-burst RX only (pipe to ffplay)
python -m examples.demo_rx --output-name USB_PnP | ffplay -i pipe:0 -an -nodisp
```

## Streaming Architecture (June 2026)

### Current OFDM Config
- **Mode**: QPSK (2 bits/SC), no pilots, 35 SC (9–43), CP=32, FFT=256
- **Preamble**: 4 OFDM symbols, QPSK, seeded with PRNG(42)
- **Data rate**: 70 bits/symbol → ~544 B/s (4.35 kbps) effective after RS(255,223) + framing
- **Output amplitude**: 0.08 peak (default) — avoids USB mic AGC clipping in the first burst

### Single-Burst Mode (Working)
Reads entire input, assembles all frames, modulates as **one** OFDM burst with a single preamble, plays continuously. Tested up to 10 frames (2230 B, 4.1s audio, 600 data syms) with 100% reliability via OTA path.

**Reliability Boundary**:
- **10 frames (600 data syms, 2230 B)**: reliable (3/3 runs verified)
- **11 frames (660 data syms, 2453 B)**: degrades (only ~10 frames valid)
- **14+ frames (840+ data syms)**: unreliable — PLL phase drift exceeds RS(32) correction

**Limitations**:
- PLL tracking degrades over very long bursts (>600 data symbols). Phase drift accumulates beyond the RS(32) correction capability.
- 20 frames (4460 B, 7.7s) showed uncorrectable RS errors — only 14/20 frames valid.

### Multi-Burst Zero-Gap Mode (Blocked)
Streaming scripts (`stream_tx_continuous.py`, `stream_rx.py`) support zero-gap concatenation with safety-margin consumption. Works in software but **fails OTA** due to USB mic AGC:

**The AGC Problem**:
- USB PnP mic (0c76) has hardware AGC with fast attack (~tens of ms)
- During the first burst (0.744s), AGC reduces gain by 8–10x
- Subsequent bursts have H ≈ 0.003–0.006 (vs H ≈ 0.02–0.06 for burst 1)
- Below the `process_samples` channel threshold (0.01), the signal passes
- But SNR is too low for reliable QPSK demodulation + RS(32) correction
- Zero gaps between bursts don't help (AGC adapts during the burst, not during gaps)
- Higher TX amplitude (0.15–0.50) does not help (AGC normalizes output)
- Lower amplitude (0.02–0.04) does not avoid AGC triggering

**Failed mitigation attempts**:
| Attempt | Result |
|---------|--------|
| Zero gaps between bursts | AGC still adapts during the burst (not gaps) |
| Higher TX amplitude (0.15–0.50) | AGC normalizes; second burst still 8–10× weaker |
| Lower TX amplitude (0.02–0.04) | AGC still triggers; signal too weak for demod |
| CFO clamp ±0.05 | Helps PLL stability but not SNR |
| Safety margin in consumption | Fixes off-by-one boundary issue but not AGC |
| Continuous TX (no stop/start) | AGC is device-level, unaffected by software |

**Root cause**: USB mic AGC attack time (~50–200 ms) is much shorter than burst duration (744 ms). The AGC fully adapts within the first burst, leaving subsequent bursts at reduced gain.

### OTA Demo Pipeline (Working)
Single-burst pipeline that reads a payload file, plays as OFDM audio over speaker, captures on USB mic, and pipes decoded bytes to ffplay.

```bash
# Convenience launcher (wraps TX + RX + ffplay)
python -m examples.demo_pipeline --input /tmp/shrek_10f.bin

# Or run components manually:
# Terminal 1 (RX -> ffplay):
python -m examples.demo_rx --output-name USB_PnP | ffplay -i pipe:0 -an -nodisp
# Terminal 2 (TX):
python -m examples.demo_tx --input /tmp/shrek_10f.bin --input-name analog-stereo

# To file (no ffplay):
python -m examples.demo_pipeline --input /tmp/shrek_10f.bin --no-ffplay --output /tmp/out.bin
```

### Prepare Demo Clip
```bash
# Extract a 2-second WebM segment from the Shrek file:
ffmpeg -ss 0 -t 2 -i absolute_smallest_shrek_v2_stripped.webm -c copy /tmp/shrek_2s.webm

# Pad to 10-frame boundary (2230 B = 10 × 223 B):
python -c "
data = open('/tmp/shrek_2s.webm', 'rb').read()[:2230]
data += b'\\x00' * (2230 - len(data))
open('/tmp/shrek_10f.bin', 'wb').write(data)
"
```

### Next Steps
1. ✅ **ffplay pipeline** — `demo_rx | ffplay -i pipe:0` for live video demo
2. ✅ **Short demo clip** — 2s WebM clip (2333 B, 10 frames = 2230 B within reliable limit)
3. 🔲 **Continuous pilot tone** — send unmodulated carrier during gaps to lock AGC
4. 🔲 **Pre-emphasis** — start first burst at low amplitude, ramp up for subsequent bursts
5. 🔲 **Thoughput optimization** — CP=16 (150 Hz sym rate), more subcarriers, reduced pilot overhead

### Relevant Files
- `src/physical/ofdm.py` — OFDM modem. Channel threshold 0.01, CFO clamp ±0.05 (was ±0.5), PLL β=0.08, leak=0.999, slope clip ±0.02
- `src/config.py` — `ModulationConfig.output_amplitude=0.08`, `OfdmConfig`: CP=32, SC=9–43, no pilots, QPSK, FFT=256, preamble=4
- `examples/demo_tx.py` — Single-burst TX: reads file, frames, modulates, plays, exits
- `examples/demo_rx.py` — Single-burst RX: listens for preamble, demodulates all symbols, parses frames, writes payload to stdout
- `examples/demo_pipeline.py` — Convenience launcher: starts RX + optional ffplay, runs TX, waits, cleans up
- `examples/stream_tx_continuous.py` — Zero-gap TX (appends no silence between bursts)
- `examples/stream_rx.py` — Streaming RX with safety-margin consumption (margin=4)
- `examples/test_stream_pipe.py` — Subprocess-based pipeline test (446 B single-burst passes, 1784 B multi-burst fails)
- `examples/test_streaming.py` — In-process single-burst test (works up to 10 frames)
- `examples/stream_tx.py` — Original streaming TX (per-burst AudioStream — deprecated by continuous version)```
