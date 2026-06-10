# Loopback Testing Guide

## Hardware Setup

Connect a 3.5mm TRS male-to-male audio cable between the computer's **output** (speaker/headphone) and **input** (microphone/line-in) jacks.

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

### Loopback Combined Sweep

```bash
python -m src.main --loopback --input-device 23 --output-device 18 \
  --payload-size 1024 --sweep-baud 500 1000 2000 3000 --output-csv sweep.csv
```

## Loopback Test Results (Microphone/Line-In Cable)

### Baud Rate Sweep (4-FSK, FEC on, 1024B payload)

| Baud | M-FSK | FEC  | Sync? | Valid | Recv'd | Throughput |
|------|-------|------|-------|-------|--------|------------|
| 500  | 4     | on   | yes   | 1     | 1024   | 633 bps    |
| 1000 | 4     | on   | yes   | 1     | 1024   | 1059 bps   |
| 2000 | 4     | on   | yes   | 1     | 1024   | 1598 bps   |
| 2200 | 4     | on   | yes   | 0     | 0      | 0 bps      |
| 3000 | 4     | on   | yes   | 0     | 0      | 0 bps      |
| 5000 | 4     | on   | yes   | 0     | 0      | 0 bps      |
| 7500 | 4     | on   | yes   | 0     | 0      | 0 bps      |
| 10000| 4     | on   | yes   | 0     | 0      | 0 bps      |

**Max reliable baud rate: 2000 baud** (24 samples/symbol at 48kHz).  
Failure at ≥2200 baud due to insufficient FFT resolution — the hann-windowed FFT at <22 samples/symbol can't reliably distinguish the 4 tones (4450 Hz spacing) against FFT bins of ≥2182 Hz width.

### M-FSK Sweep (1000 baud, FEC on, 1024B payload)

| M-FSK | Sync? | Valid | Recv'd | Throughput | Bits/sym |
|-------|-------|-------|--------|------------|----------|
| 2     | yes   | 1     | 1024   | 636 bps    | 1        |
| 4     | yes   | 1     | 1024   | 1059 bps   | 2        |
| 8     | yes   | 1     | 1024   | 1361 bps   | 3        |
| 16    | yes   | 0     | 0      | 0 bps      | 4        |

- 8-FSK is the best at 1000 baud (1361 bps throughput, 3 bits/symbol).
- 16-FSK fails because 16 tones across 200-18000 Hz gives ~1133 Hz spacing per tone, which is unresolvable at 48-sample symbols (1000 Hz FFT bins).

### FEC Dependency

- **2000 baud, FEC on**: PASS (1024 bytes received)
- **2000 baud, FEC off**: FAIL (CRC verification fails due to bit errors)

FEC (RS-32) is essential for reliable transmission even at low baud rates.

## Known Limitation: FFT Resolution at High Baud Rates

The demodulator uses windowed FFT for tone detection. The FFT resolution is:

```
bin_width = sample_rate / symbol_samples = sample_rate / (sample_rate / baud_rate) = baud_rate
```

At 3000 baud → 3000 Hz/bin → only 9 real FFT bins. With the default 200-18000 Hz range and 4-FSK (4450 Hz/tone), tones fall between FFT bins and the hann window's main lobe smears adjacent bins enough to cause symbol errors.

### Potential Improvements for Higher Baud Rates

1. **OFDM** (as you mentioned): Splits the channel into many orthogonal subcarriers, each with a long symbol duration. This lets the system use many narrowband subcarriers simultaneously, achieving high aggregate throughput while maintaining robust per-subcarrier detection.

2. **Goertzel algorithm**: Instead of a full FFT, the demodulator could evaluate signal energy at only the expected tone frequencies. This works well for M-FSK with few tones and short symbol lengths.

3. **Phase-locked loop (PLL) demodulation**: Tracks the instantaneous frequency of the carrier by measuring phase changes between samples. Works with as few as ~2-4 samples per bit and is used by legacy FSK modems (Bell 202, V.23) at 1200-2400 baud over audio channels.

4. **Matched filter / correlator**: Each tone has a known time-domain waveform; the receiver correlates the incoming signal against each candidate tone and picks the best match.

## All Tests Pass

```bash
python -m pytest tests/ -v  # 32 tests, all pass
```

## Commands Summary

```bash
# Unit tests (no hardware needed)
python -m pytest tests/ -v

# Software-only round-trip
python -m src.main --headless

# Hardware loopback with defaults
python tests/test_loopback.py -i <INPUT_IDX> -o <OUTPUT_IDX>

# Hardware loopback with parameter sweep
python tests/test_loopback.py -i <INPUT_IDX> -o <OUTPUT_IDX> \
  --sweep-baud 500 1000 2000 --sweep-mfsk 2 4 8 --output-csv results.csv

# CLI loopback mode
python -m src.main --loopback --input-device <IDX> --output-device <IDX> \
  --baud-rate 2000 --m-fsk 8 --payload-size 1024 --output-csv results.csv

# List audio devices
python -m src.main --list-devices
```
