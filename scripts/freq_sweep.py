#!/usr/bin/env python3
"""Cross-machine acoustic frequency sweep.

Plays a sequence of tones on one machine and records on another, then reports
the local in-band SNR at each frequency (tone power vs nearby noise bins).
Self-aligns each tone by finding the window where its Goertzel energy peaks.

Usage (from the recording machine):
    python3 freq_sweep.py analyze rec.wav [--tone-ms 500]
Generate a sweep wav for playback:
    python3 freq_sweep.py gen out.wav
"""
import sys
import numpy as np
import wave

FS = 48000
FREQS = [200, 300, 400, 500, 600, 800, 1000, 1200, 1500, 1800, 2200,
         2700, 3300, 4000, 4700, 5600, 6800, 8200, 10000, 12000]
TONE_MS = 500
GAP_MS = 250
AMP = 0.4


def gen(path):
    tone = int(FS * TONE_MS / 1000)
    gap = int(FS * GAP_MS / 1000)
    parts = []
    for f in FREQS:
        t = np.arange(tone) / FS
        # raised-cosine edges to avoid clicks
        r = int(FS * 0.005)
        env = np.ones(tone)
        env[:r] = 0.5 * (1 - np.cos(np.pi * np.arange(r) / r))
        env[-r:] = env[:r][::-1]
        parts.append((AMP * env * np.sin(2 * np.pi * f * t)).astype('<f4'))
        parts.append(np.zeros(gap, dtype='<f4'))
    x = np.concatenate(parts)
    w = wave.open(path, 'wb')
    w.setnchannels(1); w.setsampwidth(2); w.setframerate(FS)
    w.writeframes((x * 32767).astype('<i2').tobytes()); w.close()
    print(f"wrote {path}: {len(FREQS)} tones, {len(x)/FS:.1f}s")


def goertzel(x, f):
    n = len(x)
    k = f / FS * n
    w = 2 * np.pi * k / n
    c = 2 * np.cos(w)
    s1 = s2 = 0.0
    for v in x:
        s0 = v + c * s1 - s2
        s2, s1 = s1, s0
    return s1 * s1 + s2 * s2 - c * s1 * s2


def read_wav(path):
    w = wave.open(path, 'rb')
    ch = w.getnchannels(); sw = w.getsampwidth(); sr = w.getframerate()
    raw = w.readframes(w.getnframes())
    if sw == 2:
        x = np.frombuffer(raw, dtype='<i2').astype(np.float64) / 32768
    elif sw == 4:
        x = np.frombuffer(raw, dtype='<i4').astype(np.float64) / 2**31
    else:
        raise SystemExit(f"unsupported sample width {sw}")
    if ch > 1:
        x = x.reshape(-1, ch)[:, 0]
    if sr != FS:
        raise SystemExit(f"expected {FS} Hz, got {sr}")
    return x


def analyze(path, tone_ms=TONE_MS):
    x = read_wav(path)
    tone = int(FS * tone_ms / 1000)
    hop = int(FS * 0.02)
    print(f"recording: {len(x)/FS:.2f}s, peak {np.max(np.abs(x)):.4f}, rms {np.sqrt(np.mean(x**2)):.5f}")
    print(f"{'freq':>6} {'tone_dB':>9} {'noise_dB':>9} {'SNR_dB':>8} {'peak':>8}")
    rows = []
    for f in FREQS:
        best = None
        i = 0
        while i + tone <= len(x):
            e = goertzel(x[i:i + tone], f)
            if best is None or e > best[0]:
                best = (e, i)
            i += hop
        e, i = best
        win = x[i:i + tone] * np.hanning(tone)
        X = np.abs(np.fft.rfft(win)) ** 2
        fr = np.fft.rfftfreq(tone, 1 / FS)
        sig = X[(fr >= f - 20) & (fr <= f + 20)].sum()
        near = X[(fr >= f - 300) & (fr <= f + 300) & ~((fr >= f - 40) & (fr <= f + 40))]
        # noise power per signal-bandwidth, scaled from the nearby band
        sig_bw = ((fr >= f - 20) & (fr <= f + 20)).sum()
        noise = near.sum() / max(near.size, 1) * sig_bw
        snr = 10 * np.log10(sig / (noise + 1e-30))
        # absolute tone level in dBFS
        tone_db = 10 * np.log10(sig + 1e-30)
        noise_db = 10 * np.log10(noise + 1e-30)
        rows.append((f, snr))
        print(f"{f:>6} {tone_db:>9.1f} {noise_db:>9.1f} {snr:>8.1f} {np.max(np.abs(x[i:i+tone])):>8.4f}")
    usable = [f for f, s in rows if s >= 10]
    print(f"\n>=10 dB SNR: {usable}")
    return rows


if __name__ == '__main__':
    if len(sys.argv) >= 3 and sys.argv[1] == 'gen':
        gen(sys.argv[2])
    elif len(sys.argv) >= 3 and sys.argv[1] == 'analyze':
        analyze(sys.argv[2], tone_ms=int(sys.argv[3]) if len(sys.argv) > 3 else TONE_MS)
    else:
        raise SystemExit(__doc__)
