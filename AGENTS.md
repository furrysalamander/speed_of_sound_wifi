# OTA Testing Guide

## Hardware Setup

PC speaker → USB desk mic (0c76 USB PnP Audio Device), ~30 cm separation.

### Find Device Names

```bash
python -m src.main --list-devices
```

Typical:
```
alsa_output.pci-0000_71_00.6.analog-stereo   (output, "analog-stereo")
alsa_input.usb-0c76_USB_PnP_Audio-01.mono    (input, "USB_PnP")
```

## OFDM Implementation

### Current Parameters

| Parameter | Value | Notes |
|-----------|-------|-------|
| FFT size | 256 | 187.5 Hz subcarrier spacing |
| CP length | 32 | 0.67 ms guard interval (sufficient for 30 cm desk path) |
| Active subcarriers | 10-69 (60 total) | 1875-12938 Hz (avoids USB mic dip at 1200-1500 Hz) |
| Subcarrier modulation | QPSK (2 bits) | Gray-coded, 120 bits/symbol |
| Preamble | 4 OFDM symbols | Known QPSK, per-frame |
| Symbol duration | 288 samples (6 ms) | CP + FFT |
| Symbol rate | 166.7 Hz | 48000/288 |
| Data syms/frame | 35 | Computed dynamically from payload_size |
| Frame time | 234 ms | 4 preamble + 35 data syms |
| Payload per frame | 442 B | 2 RS(255,223) blocks, CRC-32 |

### Throughput

| Layer | Bits/sym | Symbol rate | Raw bps | Overhead | Net bps |
|-------|----------|-------------|---------|----------|---------|
| Modulation (60× QPSK) | 120 | 166.7 Hz | 20,000 | — | 20,000 |
| OFDM (35 data / 39 total) | — | — | 20,000 | ×0.897 | 17,949 |
| Framing (522B → 442B payload) | — | — | 17,949 | ×0.847 | 15,200 |
| RS(32) FEC (223/255) | — | — | 15,200 | ×0.875 | 13,300 |

The Shrek target is ~8.5 kbps, giving **1.8× headroom** with current config.

### Phase Tracking

**Decision-directed phase tracking** (`_dd_common`, `_dd_slope` in OfdmDemodulator):
- After equalization and QPSK demap, re-encodes decisions and estimates residual phase error per subcarrier
- Fits linear model (phase = a + b*(k - k_ref)) to separate common phase drift from timing offset
- Exponential smoothing (alpha=0.05) filters noisy estimates
- Correction applied to next symbol as `exp(-j*(a + b*(k - k_ref)))`
- State reset per-frame (each call to `process_samples`)

### Timing Recovery

Uses **preamble cross-correlation** (not CP autocorrelation):
- Demodulator stores the full time-domain preamble (generated identically to modulator)
- Cross-correlates with incoming samples to find preamble start
- Channel estimation uses zero-forcing from the 4 known preamble symbols
- One-tap equalization per subcarrier for data symbols

### Scrambler

Data bytes are XOR-scrambled with `np.random.default_rng(seed=12345)` before modulation to avoid problematic bit patterns (e.g., the sync pattern `\xAA\x55...` creating all-same QPSK constellation symbols). Demod de-scrambles with the same PRNG after demapping.

## Per-Frame Preamble Architecture

Instead of one preamble for the entire burst, **each frame gets its own preamble** (4 OFDM syms):

| Benefit | Description |
|---------|-------------|
| Unlimited duration | PLL resets per frame, no drift accumulation |
| AGC immunity | Continuous audio prevents mic AGC gain changes |
| Independent detection | If one frame is lost, subsequent frames unaffected |

**Reliability boundary**: Each frame is independently detected. The channel is binary — received frames have exactly 0 byte errors, or are completely missed (preamble not detected or RS uncorrectable).

### OTA Test Results (USB Mic, ~30 cm)

| Duration | Frames | Received | Rate |
|----------|--------|----------|------|
| 30 s (68 frames) | 66/68 | 97% | Latest (SC 10-69, 60 SC) |
| 30 s (68 frames) | 55/68 | 81% | Baseline (SC 9-43, 35 SC) |
| 60 s (150 frames) | 141/150 | 94% | Best (SC 9-43) |

The improvement from 81% to 97% when moving to per-frame analysis (vs sequential grid alignment) indicates frames are detected independently but misalignment caused false negatives in earlier analysis.

### Frame Loss Pattern

- **Losses are binary**: every received frame is byte-perfect; missing frames are completely absent
- Only 2 frames lost in the best 30 s run (frames 22 and 42)
- Loss is likely from short noise bursts or preamble detection misses — RS(32) is never the bottleneck (no partial-error frames exist)
- To recover lost frames: improve preamble detection (not stronger FEC)

## Commands

### Software Round-Trip (No Hardware)

```bash
python -c "
from src.config import Config; from src.physical.ofdm import *
c=Config(); mod=OfdmModulator(c); dem=OfdmDemodulator(c)
audio=mod.modulate_with_preamble(bytes(range(200)))
bits=dem.process_samples(audio)
r=dem.symbols_to_bytes(bits, 200) if bits is not None else b''
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
# Full pipeline: TX -> [OTA] -> RX -> ffplay
python -m examples.demo_pipeline --input /tmp/shrek_test_30s.bin

# To file (no ffplay):
python -m examples.demo_pipeline --input /tmp/shrek_test_30s.bin --no-ffplay

# Manual: terminal 1 (RX -> ffplay):
python -m examples.demo_rx --output-name USB_PnP | ffplay -i pipe:0 -an -nodisp

# Manual: terminal 2 (TX):
python -m examples.demo_tx --input /tmp/shrek_test_30s.bin --input-name analog-stereo
```

### Reproducible OTA Test

```bash
python -m examples.test_ota_demo
```

### Prepare Test Clips

```bash
# Create a 30-second test clip (68 frames of 442 B):
python -c "
data = open('/tmp/shrek_30s.webm', 'rb').read()  # or any test data
ps = 442
n = (len(data) + ps - 1) // ps
data += b'\x00' * (n * ps - len(data))
open('/tmp/shrek_test_30s.bin', 'wb').write(data)
"
```

### Unit Tests

```bash
python -m pytest tests/ -v
```

### List Audio Devices

```bash
python -m src.main --list-devices
```

## OTA Frequency Response (USB Mic 0c76)

Measured via full-band OFDM preamble (SC 1-127, 8 syms). Speaker → mic OTA.

| Range | |H| | Notes |
|-------|------|-------|
| SC 2-78 (375-14625 Hz) | -6 dB | Usable range |
| SC 4-69 (750-12938 Hz) | -3 dB | Recommended range |
| SC 7-9 (1312-1688 Hz) | ~-6 dB dip | The known 1200-1500 Hz USB mic notch |
| SC 20 (3750 Hz) | peak (1.42) | Best channel response |
| > SC 69 (12938 Hz) | > -3 dB rolloff | Below usable threshold |

Current config uses SC 10-69 to stay within -3 dB band while avoiding the dip.

## Architecture

### Frame Format

Each frame: 522 total bytes
```
[8 B sync] [4 B header] [RS(255,223) block 1: 223 data + 32 parity]
[RS(255,223) block 2: 219 data + 32 parity] [4 B CRC-32]
```

- `payload_size=442`: fills 2 RS blocks (223 + 219 data bytes)
- `nsym=32`: corrects up to 16 byte errors per block (32 total per frame)
- Sync pattern: `\xAA\x55\xAA\x55\xAA\x55\xAA\x55`

### TX Flow (demo_tx.py)

1. Read input file
2. Pad to payload_size boundary
3. For each frame: assemble (sync + RS + CRC), modulate with own preamble
4. Concatenate all frame audio
5. Append 0.5 s silence
6. Play via callback (audio stream)
7. Poll `tx_pos[0]` until all samples consumed

### RX Flow (demo_rx.py)

1. Continuous audio capture via callback
2. Buffer grows (no pruning — pruning caused cumulative drift)
3. Slide `search_pos` forward, correlate with `_preamble_audio` (reverse convolution)
4. Find cross-correlation peak, compute normalized peak energy
5. If `norm_peak >= 0.10`: extract chunk (margin + frame_samples + padding), demodulate
6. `process_samples()` does: PLL init → per-symbol equalization → DD tracking → QPSK demap → descramble
7. Parse bytes with FrameParser → if CRC valid → write payload to stdout
8. Advance `search_pos` by `frame_samples` regardless of success

### Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| No buffer pruning | Pruning caused 1-sample/frame drift → hundreds of samples across 277 frames → preamble miss |
| `search_pos = abs_pos + frame_samples` | Fixed step matches frame boundaries exactly; avoids redundant scanning |
| Per-frame preamble | PLL resets, independent frame detection, unlimited total duration |
| No pilot subcarriers | DD tracking sufficient; pilots waste 4/60 SC (7%) |
| Scrambler (seed=12345) | Prevents sync-pattern aliasing in DD phase tracking |
| Preamble threshold 0.10 | Balances detection rate vs false positives; CRC catches false alarms |

## Next Steps

1. ✅ **SC range expanded** 9-43 → 10-69 based on OTA frequency response measurement
2. ✅ **Frame duration reduced** 60 → 35 data syms (384 ms → 234 ms, 39% shorter)
3. ✅ **OTAs reliability characterized** 97% at 30 s, binary frame loss pattern
4. 🔲 **Reduce preamble miss rate** — increase preamble symbols (4→6 or 8), or lower threshold with CRC catch
5. 🔲 **On-the-fly TX generation** — avoid O(1 GB) audio buffer for 90-min Shrek

## Relevant Files

- `src/config.py` — Defaults: SC 10-69, CP=32, QPSK, no pilots, preamble=4, payload_size=442, nsym=32
- `src/physical/ofdm.py` — Channel threshold 0.01, CFO clamp ±0.05, PLL β=0.08, leak=0.999, slope clip ±0.02. Per-frame PLL reset.
- `examples/demo_rx.py` — Continuous RX, dynamic `frame_data_syms()`, signal handlers, no buffer pruning
- `examples/demo_tx.py` — Per-frame preamble TX, polling wait for playback completion
- `examples/demo_pipeline.py` — Convenience launcher: RX (+ optional ffplay) + TX
- `examples/test_ota_demo.py` — Reproducible OTA test (97% at 30 s expected)
- `src/link/framing.py` — FrameAssembler, FrameParser (sync, RS, CRC)
- `src/link/fec.py` — ReedSolomonFec with configurable nsym
- `src/audio/io.py` — AudioStream, split-duplex fallback
- `/tmp/freq_response.json` — Mic |H| per SC 1-127, captured OTA
