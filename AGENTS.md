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
| CP length | 64 samples | ~1.3 ms guard interval |
| Active subcarriers | 4-64 (61 total) | 750-12000 Hz band |
| Subcarrier modulation | QPSK (2 bits) | Gray-coded |
| Preamble | 2 OFDM symbols | Known QPSK for channel estimation |
| Symbol duration | 320 samples (6.67 ms) | CP + FFT body |
| Symbol rate | 150 Hz | 48000/320 |

### Throughput Estimate

```
Raw:         61 subcarriers × 2 bits × 150 Hz = 18,300 bps
RS-FEC (32): 18,300 × (223/255)              = 16,012 bps
Framing:     16,012 × (223/271)              ≈ 12,570 bps
```

Target 7.5 kbps is feasible with >50% margin.

### Timing Recovery

Uses **preamble cross-correlation** (not CP autocorrelation):
- Demodulator stores the full time-domain preamble (generated identically to modulator)
- Cross-correlates with incoming samples to find preamble start
- Provides accurate timing even with the raised-cosine onset ramp on the first CP

Channel estimation uses zero-forcing from the 2 known preamble symbols.
One-tap equalization per subcarrier for data symbols.

## Commands Summary

```bash
# Unit tests (no hardware needed)
python -m pytest tests/ -v

# Software-only round-trip
python -m src.main --headless

# Hardware loopback (USB mic, FSK)
python tests/test_loopback.py -i 19 -o 17 --payload-size 64 --baud-rate 500 \
  --freq-min 3000 --freq-max 7000 --m-fsk 2

# Hardware loopback (USB mic, 4-FSK)
python tests/test_loopback.py -i 19 -o 17 --payload-size 128 --baud-rate 500 \
  --freq-min 3000 --freq-max 9000 --m-fsk 4

# Hardware loopback (USB mic, OFDM)
python tests/test_loopback.py -i 19 -o 17 --ofdm --payload-size 4096

# Hardware loopback with parameter sweep
python tests/test_loopback.py -i 19 -o 17 --payload-size 128 \
  --sweep-baud 500 750 --sweep-mfsk 2 4 --output-csv results.csv

# CLI loopback mode (FSK)
python -m src.main --loopback --input-device 19 --output-device 17 \
  --payload-size 1024 --baud-rate 2000 --m-fsk 8 --output-csv results.csv

# CLI loopback mode (OFDM)
python -m src.main --loopback --input-device 19 --output-device 17 \
  --ofdm --payload-size 4096 --output-csv results.csv

# List audio devices
python -m src.main --list-devices
```
