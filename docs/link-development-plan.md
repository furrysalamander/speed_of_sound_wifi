# Link Development Plan — From Current State to a Proven Acoustic Data Link

> Status: **Stages 0–5 implemented and exercised 2026-09-20.** Ethernet/TAP/IP
> remains deferred. See `README.md` → "Acoustic Link Proven" and
> `docs/channel-report.md`.
>
> | Stage | Deliverable | Outcome |
> |-------|-------------|---------|
> | 0 | `phy_bench` channel characterization | done; report written |
> | 1 | non-coherent M-FSK baseline | done; decodes acoustically |
> | 2 | M-FSK rate ladder | done; ~350 Hz coherent span limit found |
> | 3 | `link_train` discovery + rate negotiation | done; sim-validated |
> | 4 | parallel multi-tone FSK bank + RS FEC | done; 16ch/20ms ~210 bps eff. |
> | 5 | `sosw_ftp` stop-and-wait ARQ | done; **cross-machine transfer proven** |
>
> Concrete results and rates are in `README.md` → "Acoustic Link Proven".

## Objective

Build and prove a **bidirectional acoustic data link** between giratina and
deoxys, from first principles, climbing the rate ladder only after each rung is
proven. Rate is whatever we can achieve with a **reasonable headroom for poor
environments**; there is no hard target. Robustness takes priority over speed.

### Success criteria

- A link with a measurable **headroom** (margin above the detection threshold
  for a target error rate), reported on all four paths.
- An automated **handshake + link-training** phase that settles level, timing,
  channel, and mode before data.
- Round-trip (bidirectional) validation at every stage, not just one-way.
- Honest reporting: no unqualified "it works"; every claim backed by a saved
  measurement.

## Current state

See `README.md` → "Link Status (measured 2026-09-19)" and
`docs/frequency_sweep.md` for the full tables. Summary of what is established:

- **Works:** DTMF control-channel handshake across machines (both directions);
  OFDM software loopback; OFDM digital (PipeWire monitor) loopback on both
  machines; tone sweep carries 2–22 kHz with ≥21 dB SNR (≥30 dB at nearly every
  tone) on all four paths.
- **Fails acoustically:** OFDM (coherent and time-differential) and single-carrier
  QPSK — 0% across tested levels, presets, and CP lengths. The Python reference
  fails identically now.
- **Measured constraints:**
  - Capture transfer curve is linear only to input ≈ 0.1, then saturates ≈ 1.16.
  - Acoustic OFDM per-subcarrier SNR ≈ 3 dB, **unchanged when the signal is made
    9 dB louder** — a signal-proportional impairment (not additive noise).
  - Clock offset ≈ 6.4 ppm.
  - Cross-machine DTMF delay ≈ 3–5.7 s observed.
- **Not established:** the mechanism of the signal-proportional impairment.
  It is a hypothesis (capture AGC/limiter/time-variation), to be characterized
  in Stage 0 before it is designed around.

## Guiding principles

Derived from the observations above:

1. **Stay in the linear region.** Prefer low-PAPR / constant-envelope waveforms
   and calibrate transmit level during link training.
2. **Do not depend on absolute phase across a sustained signal.** Prefer
   non-coherent or differentially detected modulation, and/or short coherent
   bursts with per-burst channel estimates.
3. **Bound burst length** so slow gain changes cannot corrupt a burst; use link
   training to settle the capture gain first.
4. **Measure headroom explicitly** on all four paths; do not infer it.
5. **Train before data.** Handshake and link training are a first-class part of
   the link, not an afterthought.

## Stage 0 — Instrumentation and impairment characterization

Goal: explain and quantify the wall before committing to a waveform.

Build `phy_bench` (a Rust bin in `sosw-tap`): run N bursts for a given
(host, direction, config) and report BER/PER, FEC-correctable %, per-tone /
per-symbol SNR, headroom, and round-trip success; save raw captures for offline
re-analysis.

Characterize directly:

- **Gain dynamics:** tone-after-silence step response (gain vs time); gain vs
  burst duration; gain vs level. Establish the safe operating level and the
  maximum burst length before gain moves.
- **Delay spread / coherence bandwidth:** chirp or MLS impulse response (currently
  unknown). Determines CP/guard and baud limits.
- **Two-tone intermodulation vs level:** extend the existing low-volume points.
- **Clock offset** (have 6.4 ppm) and **cross-machine latency** (have ~3–5.7 s);
  attempt to reduce latency (PipeWire buffer size, sink suspend/idle handling).

Deliverable: `docs/channel-report.md` with the measured curves.

**Exit criteria:** known linear operating level; known gain time constant; known
delay spread and coherence bandwidth; latency measured (and reduced if possible).

## Stage 1 — Baseline reliable link (2-FSK, non-coherent)

> **Done.** `sosw-core/src/physical/fsk.rs` (CPFSK + Goertzel + grid lock +
> per-tone calibration + compact RS FEC); `fsk_bench` measures it.

The simplest thing that can be proven end to end.

- Constant-envelope **2-FSK**, narrowband (start ≈ 1–3 kHz), low baud (start
  ≈ 50–100), long symbols.
- **Goertzel** energy detection with **grid-locked** timing — generalize the
  existing DTMF decoder (`sosw-core/src/physical/dtmf.rs`).
- Frame = preamble + length + payload + CRC-32 (+ optional RS).
- Minimal send/ACK ping-pong to prove round trips.
- Reuse `sosw-tap/src/link.rs` and the `sosw_link` scaffolding.

**Exit criteria:** cross-machine round-trip **PER < 1%** (post-FEC ≈ 0) with
**≥15 dB measured headroom**; documented goodput. Validated on the 4-path matrix.

## Stage 2 — Rate ladder (M-FSK, baud up)

> **Done** (single-stream). `fsk_bench --mode sweep`; the coherent span limit
> (~350 Hz) caps single-stream M-FSK at ~333 bps and motivated Stage 4.


Climb one rung at a time, measuring at every rung.

- **M = 2 → 4 → 8 → 16**, baud **100 → 500 → 1000 → 2000** (adjust to the
  Stage-0 delay spread).
- DSP/encoding tricks as needed: orthogonal tone spacing, windowing, per-tone
  normalization, frequency-selective fading handling (skip/duplicate bad tones),
  differential tone encoding, interleaving.
- Produce a **rate vs headroom curve** and identify the knee.

**Exit criteria:** best M-FSK rate at target headroom documented; explicit
decision whether it is good enough or Stage 4 is required.

## Stage 3 — Link training / handshake (full)

> **Done** (logic). `sosw-tap/src/bin/link_train.rs`, `--mode a|b|sim`:
> discovery, control-channel headroom, fastest-first mode probing, selection.


Run over the robust low-rate channel (DTMF or the Stage-1 FSK), tolerant of
multi-second latency and retries:

1. Discovery and node IDs.
2. **Channel sounding**: tone sweep / chirp; receiver estimates SNR per band and
   delay spread.
3. **Level calibration**: receiver measures received level and requests a TX gain
   that lands in the measured linear region; verifies.
4. **Timing/clock**: estimate clock offset, exchange, lock the symbol grid.
5. **Mode negotiation**: select modulation/baud/FEC from a table based on measured
   SNR/headroom.
6. **Round-trip proof**: exchange test bursts and confirm the achieved PER before
   starting data.

**Exit criteria:** a `link_train` binary that converges automatically between the
two nodes, reports achieved headroom, and is repeatable across runs.

## Stage 4 — Wideband equalized PHY (only if needed for rate)

> **Done, as parallel multi-tone FSK** (`sosw-core/src/physical/fsk_bank.rs`),
> chosen over SC-FDE because the channel is time-varying and non-coherent
> detection is robust to it. 16 channels @ 20 ms reached ~210 bps effective.


Gated on Stage 2 falling short **and** Stage 0/3 showing the channel supports it.
Options, in order of preference:

- **SC-FDE**: single-carrier + cyclic prefix + frequency-domain equalization —
  low PAPR, suited to frequency-selective channels.
- **OFDM with mitigation**: PAPR reduction + level calibration + short bursts
  (defeat gain drift) + per-burst channel estimates.
- **Multi-tone parallel FSK**.

DSP menu: adaptive equalization, decision-directed tracking, differential
encoding, interleaving, stronger FEC.

**Exit criteria:** target rate at target headroom on all four paths.

## Stage 5 — Link layer (still no Ethernet)

> **Done.** `sosw-tap/src/xfer.rs` (stop-and-wait ARQ) + `sosw_ftp`;
> cross-machine giratina→deoxys transfer of 31 bytes verified byte-exact.


- Framing, RS FEC, CRC, stop-and-wait ARQ, fragmentation.
- Bidirectional **file transfer** with round-trip ACKs and integrity checking
  (hash).

**Exit criteria:** transfer a file (e.g. ~100 KB) cross-machine with 0 corrupt
bytes; document goodput and retransmission rate.

## Deferred — Ethernet

MAC (CSMA/CA), TAP plumbing, IP, ping, and HTTP are **deferred** until Stages
0–5 are proven. Existing `sosw-tap` MAC/fragment code is retained for that later
work; it is not the current milestone.

## Validation methodology (applies to every stage)

- **4-path matrix:** giratina self-loopback, deoxys self-loopback,
  giratina→deoxys, deoxys→giratina.
- **Headroom:** reduce TX gain (or add noise) until PER rises; report the margin.
- **Environment stress:** background noise, microphone repositioning, obstruction.
- **Reproducibility:** scripts plus saved raw captures so every result can be
  re-analyzed offline (a recurring, valuable technique this project has used).

## Existing assets to build on

- DTMF codec with grid-locked decoder (`sosw-core/src/physical/dtmf.rs`) →
  generalize to FSK.
- Link framing and handshake (`sosw-tap/src/link.rs`, `sosw_link`).
- `FrameAssembler` / `FrameParser`, RS FEC, CRC (`sosw-core/src/link/`).
- Measurement tools: `ota-validate`, `sc_loopback`, `chan_test`,
  `sr_offset_test`, `scripts/freq_sweep.py`.
- `Modulator`/`Demodulator` traits (`sosw-core/src/lib.rs`) for placing new
  modulations alongside OFDM.

## Decisions (2026-09-19)

- **Rate:** no hard target; accept whatever the hardware gives. Possible future
  hardware work to push beyond 10 kbps.
- **Hardware:** keep the current giratina/deoxys mics and speakers for the whole
  exercise.
- **Latency:** attempt to reduce it, but accept it if that is all we get.
- **Band:** start with the well-characterized audible range; expand to the wider
  2–22 kHz range once the link works well.
- **Code placement:** in the existing crates behind the `Modulator`/`Demodulator`
  traits, wherever it fits best.

## Risks

- The signal-proportional impairment is unexplained; Stage 0 is make-or-break.
  If it is an unavoidable capture AGC, design around it (short bursts, constant
  envelope) or that becomes a hardware discussion.
- Multi-second latency is hostile to round-trip protocols; if it cannot be
  reduced, protocol timing must absorb it.
- M-FSK may cap below the desired rate, forcing Stage 4 and its own risk.
- Rate achievability on these mics is unknown; the plan measures rather than
  assumes.
