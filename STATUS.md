# Status Report — June 11, 2026

## Overall Objective

Stream a Shrek video (5.75 MB AV1 WebM, 114×64, 90 min, ~8.5 kbps) over
acoustic OFDM from PC speaker to USB desk microphone (0c76 USB PnP) in
real-time. The receiving machine pipes reassembled bytes into
`ffplay -i pipe:0` for playback.

The Shrek file is the most heavily compressed AV1 achievable — we cannot
re-encode. The modem must match or exceed ~8.5 kbps net throughput after
all framing, FEC, and modulation overhead.

---

## Current Status: TARGET EXCEEDED

**15.1 kbps net throughput** — 1.8× the 8.5 kbps target.

Measured over **open air** (PC speaker → USB desk mic, ~30 cm):
- 30 s: **66/68 frames (97%)** — SC 10-69, 60 SC, QPSK, CP=32
- 60 s: **141/150 frames (94%)** — SC 9-43, 35 SC (previous config)

All received frames have **0 byte errors** — the channel is binary
(perfect or absent). No partial-error frames exist, so RS(32) is never
the bottleneck.

---

## Key Changes Made This Session

| # | Change | File | Lines | Why |
|---|--------|------|-------|-----|
| 1 | **Data scrambler** (XOR with PRNG seed=12345) | `src/physical/ofdm.py` | 138-141, 495-498 | Prevents the frame's `\xAA\x55\xAA\x55\xAA\x55\xAA\x55` sync pattern from creating "all-same" QPSK constellation symbols that confused decision-directed phase tracking. |
| 2 | **Centered phase slope** `(k - k_ref)` instead of raw `k` | `src/physical/ofdm.py` | 242-243, 422, 447 | Decouples the common phase estimate from the slope in the weighted least-squares fit. |
| 3 | **Correct `dd_common` initialization** (2.5 × per_sym_drift, was 4×) | `src/physical/ofdm.py` | 389-393 | The channel estimate H is centered at preamble symbol 1.5 (not symbol 4). Old init over-corrected by 1.5 × per_sym_drift. |
| 4 | **CP=32** (was CP=128) | `src/config.py` | 37 | 150 Hz → 167 Hz symbol rate. 0.67 ms guard still sufficient at 30 cm. |
| 5 | **SC range 10-69** (was 9-43 / 60 SC vs 35 SC) | `src/config.py` | 39-40 | Uses full -3 dB bandwidth of USB mic (750-12938 Hz). Avoids dip at SC 7-9. |
| 6 | **No pilot subcarriers** (was 4) | `src/config.py` | 42 | Saves 4/60 = 6.7% overhead. DD tracking reliable enough without pilots. |
| 7 | **35 data syms/frame** (was 60) | `examples/demo_rx.py` | 28-37 | Dynamic computation from config. Reduces frame time 384 → 234 ms (39% shorter). |
| 8 | **No buffer pruning** | `examples/demo_rx.py` | 49 | Pruning caused cumulative 1-sample/frame drift → preamble misses at 277+ frames. |
| 9 | **TX polling wait** | `examples/demo_tx.py` | 78-80 | Replaced `time.sleep` with `while tx_pos[0] < len(audio)` to prevent truncation. |
| 10 | **Signal handlers** | `examples/demo_rx.py` | 57-62 | SIGINT/SIGTERM flush output cleanly. |
| 11 | **Output flush on success** | `examples/demo_rx.py` | 117 | Explicit flush after each frame for pipe reliability. |
| 12 | **Mic frequency response measurement** | `/tmp/freq_sweep3.py` | — | Full-band OFDM sweep (SC 1-127, 8 syms). Confirmed -3 dB at SC 4-69, dip at SC 7-9. |
| 13 | **Per-frame architecture** | `demo_tx.py`, `demo_rx.py` | — | Each frame has own preamble. PLL resets per frame, unlimited duration, no AGC adaptation. |
| 14 | **Fixed test_stream_pipe.py data size** | `examples/test_stream_pipe.py` | 16-17 | Now computed from config instead of hardcoded 1784. |
| 15 | **Fixed demo_pipeline.py default input** | `examples/demo_pipeline.py` | 13 | Changed from `/tmp/shrek_10f.bin` to `/tmp/shrek_test_30s.bin`. |

---

## Default Config

```python
# src/config.py OfdmConfig
fft_size: int = 256
cp_length: int = 32
subcarrier_min: int = 10     # 1875 Hz
subcarrier_max: int = 69     # 12938 Hz
bits_per_subcarrier: int = 2  # QPSK
pilot_subcarriers: tuple = ()
preamble_symbols: int = 4

# src/config.py FrameConfig
payload_size: int = 442       # 2 RS(255,223) blocks

# src/config.py FecConfig
nsym: int = 32                # 16 byte errors corrected per block
```

### Throughput Breakdown

| Layer | Bits/sym | Symbol rate | Raw bps | Overhead | Net bps |
|-------|----------|-------------|---------|----------|---------|
| Modulation (60× QPSK) | 120 | 166.7 Hz | 20,000 | — | 20,000 |
| OFDM (35 data / 39 total) | — | — | 20,000 | ×0.897 | 17,949 |
| Framing (522 B → 442 B payload) | — | — | 17,949 | ×0.847 | 15,200 |
| RS(32) FEC (223/255) | — | — | 15,200 | ×0.875 | 13,300 |

Effective: **15.1 kbps** (1.8× headroom vs 8.5 kbps target)

---

## Test Commands

### Unit Tests (No Hardware)

```bash
python -m pytest tests/ -v
```

### Software Round-Trip

```bash
python -c "
from src.config import Config; from src.physical.ofdm import *
c=Config(); mod=OfdmModulator(c); dem=OfdmDemodulator(c)
audio=mod.modulate_with_preamble(bytes(range(200)))
bits=dem.process_samples(audio)
r=dem.symbols_to_bytes(bits,200) if bits is not None else b''
print('OK' if r[:200]==bytes(range(200)) else 'FAIL')
"
```

### Multi-Frame Software Round-Trip

```bash
python -c "
from src.config import Config; from src.physical.ofdm import *
from src.link.framing import FrameAssembler, FrameParser
c=Config(); mod=OfdmModulator(c); dem=OfdmDemodulator(c)
asm=FrameAssembler(c); parser=FrameParser(c)
n=68; ps=c.frame.payload_size
payload=bytes(i%256 for i in range(n*ps))
framed=b''.join(asm.assemble_frame(payload[i*ps:(i+1)*ps]) for i in range(n))
audio=mod.modulate_with_preamble(bytes(framed))
bits=dem.process_samples(audio)
byte_est=len(bits)//8+64
decoded=dem.symbols_to_bytes(bits, byte_est)
frames=parser.feed_bytes(decoded)
recv=bytearray()
for p,s,t,v in frames:
    if v: recv.extend(p)
print(f'{len(recv)}/{len(payload)} match={recv==payload}')
"
```

### OTA Demo Pipeline

```bash
# Convenience (generates test data automatically):
python -m examples.test_ota_demo

# Or manual:
python -m examples.demo_rx --output-name USB_PnP > /tmp/ota_out.bin &
sleep 1
python -m examples.demo_tx --input /tmp/shrek_test_30s.bin --input-name analog-stereo
sleep 30
kill %1

# Analyze:
python -c "
d = open('/tmp/shrek_test_30s.bin','rb').read()
r = open('/tmp/ota_out.bin','rb').read()
nf_total = len(d) // 442
nf = len(r) // 522
sent_set = set(d[i*442:(i+1)*442] for i in range(nf_total))
recv_set = set(r[i*442:(i+1)*442] for i in range(nf))
matching = sum(1 for p in sent_set if p in recv_set)
print(f'{matching}/{nf_total} frames ({100*matching//nf_total}%)')
"
```

---

## Things to Watch Out For

### 1. Scrambler Seed Dependency

Both modulator and demodulator use `seed=12345` for the XOR scrambler.
The mask is generated from `np.random.default_rng(seed=12345)` and is the
same for any data length because we generate `len(data)` bytes.

**Always use the same Config object (or identical parameters) on both
sides.**

### 2. Per-Frame Preamble Scrambler Independence

With per-frame preamble TX (each frame modulated separately), the
scrambler is re-seeded per frame. The RX de-scrambles each frame's
bits independently (one call to `process_samples` per frame), so the
seeds stay in sync. This works correctly because `symbols_to_bytes`
creates a fresh PRNG per call.

### 3. CP=32 Timing Margin

CP=32 samples at 48 kHz = 0.67 ms guard interval. Preamble cross-
correlation gives timing accuracy of ±1 sample typically. For desk-to-
desk (fixed devices), this is fine. For mobile scenarios, increase CP.

### 4. Subcarriers Near Rolloff

SC > 69 (>12938 Hz) drops below -3 dB on this specific USB mic. The
scrambler spreads errors across all subcarriers, so RS correction
handles marginal SCs. If using a different mic, verify frequency
response and adjust `subcarrier_max`.

### 5. PortAudio Device Name Matching

The `--input-name` and `--output-name` CLI flags use substring matching.
PipeWire can rename devices on reconnect. Use `--list-devices` to
verify names before each test session.

### 6. Full-duplex vs Split-duplex

`AudioStream` tries full-duplex first, falls back to split-duplex. With
USB mic (different host API than motherboard output), split-duplex is
required. The fallback adds ~50 ms latency.

### 7. Frame Loss Is Binary

Every received frame has 0 byte errors. Lost frames are completely
absent (preamble not detected or RS uncorrectable). Stronger RS will
not help — the fix is better preamble detection.

---

## File Map

```
src/
├── config.py           ← Default OFDM params (CP=32, SC=10-69, no pilots, QPSK)
├── audio/
│   ├── io.py           ← AudioStream (full-duplex + split-duplex)
│   ├── loopback.py     ← LoopbackTester (burst TX → RX → verify, legacy)
│   └── devices.py      ← Device name resolution
├── physical/
│   ├── ofdm.py         ← OfdmModulator + OfdmDemodulator (core modem)
│   ├── modulator.py    ← FskModulator (legacy)
│   └── demodulator.py  ← FskDemodulator (legacy)
├── link/
│   ├── framing.py      ← FrameAssembler + FrameParser
│   ├── fec.py          ← ReedSolomonFec (RS(255,K))
│   └── crc.py          ← CRC-32
├── main.py             ← CLI entry point
examples/
├── demo_rx.py          ← Continuous per-frame OFDM receiver
├── demo_tx.py          ← Per-frame preamble OFDM transmitter
├── demo_pipeline.py    ← Convenience launcher (RX + TX + optional ffplay)
├── test_ota_demo.py    ← Reproducible OTA test (97% at 30 s expected)
├── test_stream_pipe.py ← Subprocess pipeline test
├── stream_tx_continuous.py  ← Zero-gap TX (deprecated by per-frame)
├── stream_rx.py             ← Streaming RX (deprecated by per-frame)
├── test_streaming.py        ← In-process burst test (deprecated)
├── stream_tx.py             ← Original streaming TX (deprecated)
tests/
└── test_*.py           ← Unit tests (all passing)
AGENTS.md               ← Testing guide (current)
STATUS.md               ← This file
absolute_smallest_shrek_v2_stripped.webm  ← 5.75 MB, 90 min AV1 (.gitignore'd)
```

---

## Quick Reference

```bash
# Unit tests (no hardware)
python -m pytest tests/ -v

# Software round-trip (single frame)
python -c "
from src.config import Config; from src.physical.ofdm import *
c=Config(); mod=OfdmModulator(c); dem=OfdmDemodulator(c)
audio=mod.modulate_with_preamble(bytes(range(200)))
bits=dem.process_samples(audio)
r=dem.symbols_to_bytes(bits, 200) if bits is not None else b''
print('OK' if r[:200]==bytes(range(200)) else 'FAIL')
"

# Software round-trip (68 frames)
python -c "
from src.config import Config; from src.physical.ofdm import *
from src.link.framing import FrameAssembler, FrameParser
c=Config(); mod=OfdmModulator(c); dem=OfdmDemodulator(c)
asm=FrameAssembler(c); parser=FrameParser(c)
n=68; ps=c.frame.payload_size
payload=bytes(i%256 for i in range(n*ps))
framed=b''.join(asm.assemble_frame(payload[i*ps:(i+1)*ps]) for i in range(n))
audio=mod.modulate_with_preamble(bytes(framed))
bits=dem.process_samples(audio)
byte_est=len(bits)//8+64
decoded=dem.symbols_to_bytes(bits, byte_est)
frames=parser.feed_bytes(decoded)
recv=bytearray()
for p,s,t,v in frames:
    if v: recv.extend(p)
print(f'{len(recv)}/{len(payload)} match={recv==payload}')
"
```
