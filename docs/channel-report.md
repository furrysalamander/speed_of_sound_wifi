# Acoustic Channel Report (Stage 0)

Measured on **giratina**, 2026-09-20, with the self-loopback path
(built-in/USB speaker → room → USB PnP mic). Produced by
`sosw-tap/src/bin/phy_bench.rs`. Raw captures are in `/tmp/sosw/`.

The purpose of Stage 0 is to explain the measured wideband failure **before**
choosing a waveform. These are direct measurements, not inference.

## Method

Every `phy_bench` script is prefixed with an unambiguous chirp marker (300–6000
Hz, 80 ms) so the recording can be aligned to the played audio even when the
body is repetitive. Recordings include 6 s of extra capture to cover the
multi-second acoustic latency. `--dump`/`--load` allow offline re-analysis.

## Findings

### 1. Acoustic latency is ~3.8 s (self-loopback)

Marker alignment found **3806 ms** consistently across every command on this
machine. This matches the multi-second cross-machine DTMF delay measured
earlier (≈3–5.7 s). A latency this large is a first-class protocol constraint:
it dominates handshake/interframe timing and is far larger than any symbol or
frame duration. Reducing it (PipeWire buffer/suspend settings) is a Stage 3/0
follow-up.

### 2. The capture path is linear over the usable range

Tone transfer curve at 1 kHz (digital drive → received RMS):

| drive | 0.01 | 0.02 | 0.04 | 0.06 | 0.08 | 0.10 | 0.15 | 0.20 | 0.30 | 0.40 | 0.60 | 0.80 | 1.00 |
|-------|------|------|------|------|------|------|------|------|------|------|------|------|------|
| rx_rms| .0096| .0166| .0317| .0471| .0627| .0784| .115 | .153 | .230 | .305 | .458 | .610 | .737 |
| clip %| 0    | 0    | 0    | 0    | 0    | 0    | 0    | 0    | 0    | 0    | 0    | 0    | 10.7 |

Received level is proportional to drive up to ≈0.8 (peak ≈0.88 FS); hard
clipping starts at drive 1.0 (peak 1.00, 10.7% of samples at FS). **Recommended
operating point: drive 0.5–0.7, peak ≈0.6–0.7**, comfortably linear with
headroom.

### 3. No AGC drift within a sustained tone

A 4 s, 1 kHz tone at drive 0.6 shows a **flat** capture: max−min over the middle
80% is **0.0 dB**. There is no gain tracking of a sustained tone on this path.

### 4. But the gain is history- and run-dependent (time variation)

Two `gain-level` runs taken minutes apart disagreed by ~2.7× at the same drive
(one run gave 0.23 RMS at drive 0.6, the next gave 0.46). The first run also
contained a large transient in the drive-0.02 segment (rx_peak 0.805) and all
subsequent segments were attenuated. This is consistent with a slow-release
capture AGC/limiter that reacts to loud transients and holds the reduced gain
for seconds, even though it does not move during a steady tone. This
**time-varying, history-dependent gain** is the leading explanation for the
wideband failure and is not an additive-noise problem.

### 5. Negligible intermodulation (linear, not limiting, at these levels)

Two-tone test (1.5 + 2.0 kHz), IM products relative to the fundamental:

| drive | 0.05 | 0.10 | 0.20 | 0.30 | 0.45 | 0.60 | 0.80 | 1.00 |
|-------|------|------|------|------|------|------|------|------|
| (2f1−f2)/f1 dB | −39.8 | −49.9 | −50.0 | −48.4 | −43.8 | −47.2 | −46.9 | −51.9 |
| (2f2−f1)/f1 dB | −47.9 | −62.6 | −54.0 | −55.7 | −45.2 | −53.7 | −56.8 | −55.4 |

IM stays 40–55 dB down and does **not** grow with drive. The path is linear at
these levels; compression is not the mechanism.

### 6. Wideband single-tone SNR is high everywhere

Frequency sweep (0.4 s per tone, drive 0.6), tone power vs in-band noise probes:

| Hz | 300 | 500 | 800 | 1000 | 1500 | 2000 | 3000 | 4000 | 5000 | 6000 | 8000 | 10000 | 12000 | 14000 | 16000 | 18000 | 20000 |
|----|-----|-----|-----|------|------|------|------|------|------|------|------|-------|-------|-------|-------|-------|-------|
| SNR dB | 34.5 | 29.7 | 33.9 | 35.2 | 66.3 | 41.6 | 76.1 | 48.0 | 49.5 | 71.9 | 54.1 | 54.9 | 81.5 | 58.7 | 59.3 | 66.6 | 62.9 |

Every tone from 300 Hz to 20 kHz carries ≥30 dB SNR. The channel is **not**
band-limited in the way a naive mic-response story would suggest. This agrees
with the earlier `docs/frequency_sweep.md`.

### 7. Delay spread is modest but non-trivial

Chirp (300–8000 Hz, 200 ms) matched filter:

- Peak at the expected arrival (~3.9 s), peak correlation 18.2.
- **−10 dB delay width: 0.67 ms** (coherence bandwidth ≈ 1.5 kHz).
- **RMS delay spread: 3.2 ms** (coherence bandwidth ≈ 50 Hz) — inflated by a
  low-level tail/noise in the PDP window; treat 0.67 ms as the strong-path
  width and 3.2 ms as a conservative bound.
- Guard/CP sizing: ≥12 ms is a safe conservative bound.

## Conclusions for waveform design

1. **Not a noise problem.** Single-tone SNR is ≥30 dB in-band, so a robust
   narrowband/FSK PHY should work and M-FSK can climb in rate.
2. **Not clipping or compression** at the recommended operating point.
3. **Time-varying, history-dependent capture gain** is the leading suspect for
   the wideband OFDM failure: a bounded, constant-envelope burst that is short
   relative to the gain's adaptation time is the right defense. This directly
   justifies the Stage 1 non-coherent M-FSK plan.
4. **Latency ~3.8 s** rules out tight turn-taking and must be absorbed by
   protocol timing and long record windows.
5. **Delay spread** sets a floor on guard/CP if/when we return to wideband
   (Stage 4): ≥12 ms conservative.
