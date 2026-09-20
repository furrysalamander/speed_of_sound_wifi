//! Stage 0 instrumentation: characterize the acoustic channel before choosing
//! a waveform. All commands build a deterministic audio script, play it once
//! while recording, then analyze the recording. `--dump` saves the raw capture
//! (little-endian f32) and `--load` re-analyzes it offline, so cross-machine
//! captures can be moved and re-examined.
//!
//! Commands:
//!   gain-step   tone after silence -> capture gain vs time (AGC/limiter test)
//!   gain-level  tone at several drives -> capture transfer curve / clipping
//!   impulse     chirp -> impulse response, delay spread, coherence bandwidth
//!   two-tone    two tones -> intermodulation products vs level
//!   tone-snr    frequency sweep -> per-tone SNR (in-band noise floor)
//!   latency     click -> acoustic loopback latency

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use sosw_core::physical::dtmf::goertzel;
use sosw_tap::audio::{self, DuplexAudio};
use std::time::Duration;

const SR: u32 = 48_000;

#[derive(Parser)]
#[command(name = "phy-bench", about = "Acoustic channel characterization (Stage 0)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Args)]
struct AudioArgs {
    #[arg(long)]
    tx_device: Option<String>,
    #[arg(long)]
    rx_device: Option<String>,
    #[arg(long, default_value_t = 0.2)]
    amplitude: f32,
    /// Save the recording to this raw f32 path.
    #[arg(long)]
    dump: Option<String>,
    /// Load a previously dumped capture instead of acquiring.
    #[arg(long)]
    load: Option<String>,
    /// Keep recording this long after playback ends.
    #[arg(long, default_value_t = 500)]
    tail_ms: u64,
    /// Extra recording time to cover the (multi-second) acoustic latency.
    #[arg(long, default_value_t = 6000)]
    record_extra_ms: u64,
    /// Drain the capture for this long before playing, to flush any input
    /// pipeline backlog (which otherwise inflates the apparent latency).
    #[arg(long, default_value_t = 0)]
    pre_drain_ms: u64,
}

#[derive(Subcommand)]
enum Cmd {
    /// Tone after silence: is there capture gain drift over a sustained tone?
    GainStep {
        #[command(flatten)]
        audio: AudioArgs,
        #[arg(long, default_value_t = 1000.0)]
        freq: f32,
        #[arg(long, default_value_t = 3000)]
        tone_ms: u64,
        #[arg(long, default_value_t = 1000)]
        lead_ms: u64,
    },
    /// Tone at several digital drives: capture transfer curve and clipping.
    GainLevel {
        #[command(flatten)]
        audio: AudioArgs,
        #[arg(long, default_value_t = 1000.0)]
        freq: f32,
        #[arg(long, default_value_t = 1200)]
        tone_ms: u64,
    },
    /// Chirp: impulse response, delay spread, coherence bandwidth, latency.
    Impulse {
        #[command(flatten)]
        audio: AudioArgs,
        #[arg(long, default_value_t = 500.0)]
        f0: f32,
        #[arg(long, default_value_t = 8000.0)]
        f1: f32,
        #[arg(long, default_value_t = 200)]
        chirp_ms: u64,
    },
    /// Two tones: intermodulation products vs drive (compression/limiter).
    TwoTone {
        #[command(flatten)]
        audio: AudioArgs,
        #[arg(long, default_value_t = 1500.0)]
        f1: f32,
        #[arg(long, default_value_t = 2000.0)]
        f2: f32,
        #[arg(long, default_value_t = 1200)]
        tone_ms: u64,
    },
    /// Frequency sweep: per-tone SNR.
    ToneSnr {
        #[command(flatten)]
        audio: AudioArgs,
        #[arg(long, default_value_t = 400)]
        tone_ms: u64,
    },
    /// Click: acoustic loopback latency (self-loopback).
    Latency {
        #[command(flatten)]
        audio: AudioArgs,
        #[arg(long, default_value_t = 2000.0)]
        freq: f32,
        #[arg(long, default_value_t = 30)]
        click_ms: u64,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::GainStep { audio: a, freq, tone_ms, lead_ms } => gain_step(&a, freq, tone_ms, lead_ms),
        Cmd::GainLevel { audio: a, freq, tone_ms } => gain_level(&a, freq, tone_ms),
        Cmd::Impulse { audio: a, f0, f1, chirp_ms } => impulse(&a, f0, f1, chirp_ms),
        Cmd::TwoTone { audio: a, f1, f2, tone_ms } => two_tone(&a, f1, f2, tone_ms),
        Cmd::ToneSnr { audio: a, tone_ms } => tone_snr(&a, tone_ms),
        Cmd::Latency { audio: a, freq, click_ms } => latency(&a, freq, click_ms),
    }
}

// --- signal generation ------------------------------------------------------

fn tone(freq: f32, n: usize, amp: f32) -> Vec<f32> {
    let ramp = (SR as usize / 200).max(1);
    (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            let mut s = (2.0 * std::f32::consts::PI * freq * t).sin();
            if i < ramp {
                s *= 0.5 * (1.0 - (std::f32::consts::PI * i as f32 / ramp as f32).cos());
            } else if i + ramp >= n {
                let j = n - 1 - i;
                s *= 0.5 * (1.0 - (std::f32::consts::PI * j as f32 / ramp as f32).cos());
            }
            s * amp
        })
        .collect()
}

fn two_tone_sig(f1: f32, f2: f32, n: usize, amp: f32) -> Vec<f32> {
    let ramp = (SR as usize / 200).max(1);
    (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            let mut s = 0.5 * (2.0 * std::f32::consts::PI * f1 * t).sin()
                + 0.5 * (2.0 * std::f32::consts::PI * f2 * t).sin();
            if i < ramp {
                s *= 0.5 * (1.0 - (std::f32::consts::PI * i as f32 / ramp as f32).cos());
            } else if i + ramp >= n {
                let j = n - 1 - i;
                s *= 0.5 * (1.0 - (std::f32::consts::PI * j as f32 / ramp as f32).cos());
            }
            s * amp
        })
        .collect()
}

fn chirp(f0: f32, f1: f32, n: usize, amp: f32) -> Vec<f32> {
    let t_total = n as f32 / SR as f32;
    let k = (f1 - f0) / t_total;
    let ramp = (SR as usize / 200).max(1);
    (0..n)
        .map(|i| {
            let t = i as f32 / SR as f32;
            let phase = 2.0 * std::f32::consts::PI * (f0 * t + 0.5 * k * t * t);
            let mut s = phase.sin();
            if i < ramp {
                s *= 0.5 * (1.0 - (std::f32::consts::PI * i as f32 / ramp as f32).cos());
            } else if i + ramp >= n {
                let j = n - 1 - i;
                s *= 0.5 * (1.0 - (std::f32::consts::PI * j as f32 / ramp as f32).cos());
            }
            s * amp
        })
        .collect()
}

// --- analysis helpers -------------------------------------------------------

fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|s| s * s).sum::<f32>() / x.len() as f32).sqrt()
}

fn peak(x: &[f32]) -> f32 {
    x.iter().map(|s| s.abs()).fold(0.0, f32::max)
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-12).log10()
}

#[allow(dead_code)]
fn envelope(x: &[f32], win: usize) -> Vec<f32> {
    x.chunks(win)
        .map(|w| (w.iter().map(|s| s * s).sum::<f32>() / w.len() as f32).sqrt())
        .collect()
}

/// Active regions (start,end) in samples, using min/peak-anchored thresholds.
#[allow(dead_code)]
fn regions(x: &[f32], win: usize) -> Vec<(usize, usize)> {
    let env = envelope(x, win);
    if env.len() < 3 {
        return Vec::new();
    }
    let mut sorted = env.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pk = sorted[sorted.len() - 1].max(1e-12);
    let mut fl = sorted[0];
    if fl > pk * 0.5 {
        fl = 0.0;
    }
    let thr = (fl * 4.0).max(pk * 0.05).max(1e-9);
    let thr_start = (fl * 8.0).max(pk * 0.15).max(thr);
    let gap_tol = win * 3;
    let mut out = Vec::new();
    let mut i = 0;
    while i < env.len() {
        while i < env.len() && env[i] <= thr_start {
            i += 1;
        }
        if i >= env.len() {
            break;
        }
        let start = i;
        let mut j = i;
        let mut last = i;
        while j < env.len() {
            if env[j] > thr {
                last = j;
            } else if (j - last) * win > gap_tol {
                break;
            }
            j += 1;
        }
        out.push((start * win, ((last + 1) * win).min(x.len())));
        i = last + 1;
    }
    out
}

/// An unambiguous chirp marker prepended to every script, so the recording can
/// be aligned to the played audio even when the body is repetitive (e.g. the
/// same tone at many amplitudes).
fn marker() -> Vec<f32> {
    chirp(300.0, 6000.0, SR as usize * 80 / 1000, 0.3)
}

/// Matched-filter the marker in `rec`; return the shift between the recording
/// and the script (peak position minus marker's script position).
fn marker_delta(rec: &[f32], m: &[f32], marker_pos: usize, search: usize) -> isize {
    let search = search.min(rec.len());
    if search < m.len() {
        return 0;
    }
    let m_norm: f32 = m.iter().map(|s| s * s).sum::<f32>().max(1e-12).sqrt();
    let mut best = (0usize, f32::MIN);
    for i in (0..search - m.len()).step_by(4) {
        let mut acc = 0.0f32;
        for j in 0..m.len() {
            acc += rec[i + j] * m[j];
        }
        let v = acc / m_norm;
        if v > best.1 {
            best = (i, v);
        }
    }
    best.0 as isize - marker_pos as isize
}

struct Acquired {
    rec: Vec<f32>,
    /// Segment offsets shifted into recording coordinates.
    segs: Vec<(usize, usize)>,
    /// Acoustic latency implied by the marker, in ms.
    latency_ms: f32,
}

/// Acquire (or load) a recording of `body`, with a chirp marker prepended.
/// `segs` are offsets into `body`; the returned segments are in `rec` coords.
fn acquire(a: &AudioArgs, body: &[f32], segs: &[(usize, usize)]) -> Result<Acquired> {
    let m = marker();
    let lead = SR as usize / 100; // 10 ms guard before the marker
    let gap = SR as usize / 20; // 50 ms gap after the marker
    let body_off = lead + m.len() + gap;
    let mut script = vec![0.0f32; lead];
    script.extend_from_slice(&m);
    script.extend(std::iter::repeat(0.0).take(gap));
    script.extend_from_slice(body);

    let rec = if let Some(path) = &a.load {
        eprintln!("loading capture {}", path);
        audio::load_capture(path)?
    } else {
        let dev = DuplexAudio::new(a.tx_device.as_deref(), a.rx_device.as_deref())?;
        dev.clear_rx();
        if a.pre_drain_ms > 0 {
            // Continuously discard until the pipeline backlog has flushed.
            let t = std::time::Instant::now();
            while t.elapsed() < Duration::from_millis(a.pre_drain_ms) {
                let _ = dev.take_rx();
                std::thread::sleep(Duration::from_millis(50));
            }
            dev.clear_rx();
        }
        std::thread::sleep(Duration::from_millis(100));
        dev.play_now(&script);
        let dur = Duration::from_secs_f64(script.len() as f64 / SR as f64);
        // Record long enough for the audio to arrive (latency can be seconds).
        std::thread::sleep(dur + Duration::from_millis(a.record_extra_ms + a.tail_ms));
        let rec = dev.take_rx();
        if let Some(path) = &a.dump {
            audio::save_f32(path, &rec)?;
            eprintln!("saved {} samples to {}", rec.len(), path);
        }
        rec
    };
    let search = SR as usize * (a.record_extra_ms as usize / 1000 + 2);
    let delta = marker_delta(&rec, &m, lead, search);
    let aligned = segs
        .iter()
        .map(|&(s, e)| {
            let so = (s as isize + body_off as isize + delta).max(0) as usize;
            let eo = ((e as isize + body_off as isize + delta).max(0) as usize).min(rec.len());
            (so.min(rec.len()), eo)
        })
        .collect();
    eprintln!(
        "recorded {} samples, script {} samples, marker delta {} samples ({:.0} ms)",
        rec.len(),
        script.len(),
        delta,
        delta as f32 / SR as f32 * 1000.0
    );
    Ok(Acquired {
        rec,
        segs: aligned,
        latency_ms: delta as f32 / SR as f32 * 1000.0,
    })
}

// --- commands ---------------------------------------------------------------

fn gain_step(a: &AudioArgs, freq: f32, tone_ms: u64, lead_ms: u64) -> Result<()> {
    let lead = lead_ms as usize * SR as usize / 1000;
    let n = tone_ms as usize * SR as usize / 1000;
    let mut body = vec![0.0f32; lead];
    body.extend(tone(freq, n, a.amplitude));
    body.extend(std::iter::repeat(0.0).take(SR as usize)); // 1 s tail
    let seg = (lead, lead + n);

    eprintln!(
        "gain-step: {} Hz, {} ms, drive {:.3}, lead {} ms",
        freq, tone_ms, a.amplitude, lead_ms
    );
    let acq = acquire(a, &body, &[seg])?;
    if acq.rec.is_empty() {
        anyhow::bail!("empty recording");
    }
    let (start, end) = acq.segs[0];
    let win = SR as usize / 50; // 20 ms
    eprintln!("play latency: {:.0} ms", acq.latency_ms);
    eprintln!("\n time_ms   level_dB");
    let rows: Vec<(f32, f32)> = (start..end)
        .step_by(SR as usize / 10)
        .map(|s| {
            let e = (s + win).min(acq.rec.len());
            let l = rms(&acq.rec[s..e]);
            ((s - start) as f32 / SR as f32 * 1000.0, l)
        })
        .collect();
    let maxl = rows.iter().map(|r| r.1).fold(0.0f32, f32::max).max(1e-12);
    for (t, l) in &rows {
        let bar = "#".repeat(((db(*l) - db(maxl) + 40.0).max(0.0) / 2.0) as usize);
        println!("{:>8.0}   {:>7.1}   {}", t, db(*l), bar);
    }
    let first = rows.first().map(|r| r.1).unwrap_or(0.0);
    let last = rows.last().map(|r| r.1).unwrap_or(0.0);
    let mid: Vec<f32> = rows.iter().skip(rows.len() / 5).map(|r| r.1).collect();
    let drift = if mid.is_empty() {
        0.0
    } else {
        db(mid.iter().fold(0.0f32, |m, &v| m.max(v)))
            - db(mid.iter().fold(f32::MAX, |m, &v| m.min(v)))
    };
    println!(
        "\nsteady-state drift over middle 80%: {:.1} dB  (early/late {:.1} dB)",
        drift,
        db(first) - db(last)
    );
    println!(
        "interpretation: <1.5 dB => essentially no AGC on a sustained tone; \
         larger => gain tracks the signal"
    );
    println!("peak in tone: {:.4}", peak(&acq.rec[start..end]));
    Ok(())
}

fn gain_level(a: &AudioArgs, freq: f32, tone_ms: u64) -> Result<()> {
    let drives = [
        0.01f32, 0.02, 0.04, 0.06, 0.08, 0.10, 0.15, 0.20, 0.30, 0.40, 0.60, 0.80, 1.00,
    ];
    let gap = SR as usize / 2; // 500 ms silence between
    let n = tone_ms as usize * SR as usize / 1000;
    let mut body = Vec::new();
    let mut segs = Vec::new();
    body.extend(std::iter::repeat(0.0).take(SR as usize / 2));
    for &d in &drives {
        let s = body.len();
        body.extend(tone(freq, n, d));
        segs.push((s, body.len()));
        body.extend(std::iter::repeat(0.0).take(gap));
    }
    eprintln!("gain-level: {} Hz, {} ms per drive, {} drives", freq, tone_ms, drives.len());
    let acq = acquire(a, &body, &segs)?;
    eprintln!("play latency: {:.0} ms", acq.latency_ms);
    eprintln!("\n drive    rx_rms    rx_peak   rx_dB    clip%");
    let mut pts: Vec<(f32, f32)> = Vec::new();
    for (i, &d) in drives.iter().enumerate() {
        let (s, e) = acq.segs[i];
        if e <= s {
            continue;
        }
        let w = &acq.rec[s..e];
        // Measure the middle ~70% to avoid edge ramps.
        let pad = w.len() / 6;
        let core = &w[pad..w.len() - pad];
        let r = rms(core);
        let p = peak(core);
        let clip = core.iter().filter(|v| v.abs() >= 0.999).count() as f32 / core.len() as f32 * 100.0;
        println!(
            "{:>6.3}   {:>8.5}   {:>8.5}   {:>6.1}   {:>5.1}%",
            d, r, p, db(r), clip
        );
        pts.push((d, r));
    }
    // Fit the linear region (first several points) and report where it breaks.
    if pts.len() >= 4 {
        let nlin = (pts.len() / 2).max(2).min(pts.len());
        let (mut sx, mut sy, mut sxx, mut sxy) = (0.0f32, 0.0, 0.0, 0.0);
        for &(x, y) in &pts[..nlin] {
            sx += x;
            sy += y;
            sxx += x * x;
            sxy += x * y;
        }
        let denom = nlin as f32 * sxx - sx * sx;
        if denom.abs() > 1e-12 {
            let slope = (nlin as f32 * sxy - sx * sy) / denom;
            println!("\nlinear-region slope: {:.3} rx/drive (fit over first {} points)", slope, nlin);
            for &(x, y) in &pts {
                let predicted = slope * x;
                if predicted > 1e-9 && y < predicted * 0.9 {
                    println!("first compression at drive {:.3} (rx {:.4}, expected {:.4})", x, y, predicted);
                    break;
                }
            }
        }
    }
    Ok(())
}

fn impulse(a: &AudioArgs, f0: f32, f1: f32, chirp_ms: u64) -> Result<()> {
    let n = chirp_ms as usize * SR as usize / 1000;
    let refsig = chirp(f0, f1, n, a.amplitude);
    eprintln!("impulse: chirp {}-{} Hz, {} ms", f0, f1, chirp_ms);
    let acq = acquire(a, &refsig, &[(0, n)])?;
    let rec = &acq.rec;
    if rec.len() < n {
        anyhow::bail!("recording too short");
    }
    // Matched filter (direct correlation) over the recording, decimated 2x.
    let search_end = (SR as usize * 8).min(rec.len());
    let ref_norm: f32 = refsig.iter().map(|s| s * s).sum::<f32>().max(1e-12);
    let step = 4usize;
    let mut corr = Vec::new();
    let mut i = 0usize;
    while i + n <= search_end {
        let mut acc = 0.0f32;
        for j in 0..n {
            acc += rec[i + j] * refsig[j];
        }
        corr.push(acc / ref_norm.sqrt());
        i += step;
    }
    let (peak_i, peak_v) = corr
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, &v)| (i, v))
        .unwrap_or((0, 0.0));
    let abs_peak = peak_i * step;
    // Power delay profile around the peak (in correlation-index units).
    let pdp_win = corr.len().min(SR as usize / 50 / step); // ~20 ms
    let lo = peak_i.saturating_sub(pdp_win / 4);
    let hi = (peak_i + pdp_win).min(corr.len());
    let pdp: Vec<f32> = corr[lo..hi].iter().map(|c| c * c).collect();
    let total: f32 = pdp.iter().sum::<f32>().max(1e-12);
    let tau_mean: f32 = pdp
        .iter()
        .enumerate()
        .map(|(i, p)| ((lo + i) * step) as f32 / SR as f32 * p)
        .sum::<f32>()
        / total;
    let tau_rms: f32 = (pdp
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let t = ((lo + i) * step) as f32 / SR as f32;
            (t - tau_mean) * (t - tau_mean) * p
        })
        .sum::<f32>()
        / total)
        .sqrt();
    // -10 dB width around the peak.
    let peak_pow = peak_v * peak_v;
    let above = |p: &[f32]| p.iter().filter(|&&x| x >= peak_pow * 0.1).count();
    let width_10 = above(&pdp) as f32 * step as f32 / SR as f32 * 1000.0;
    let coh_bw = if tau_rms > 1e-6 { 1.0 / (2.0 * std::f32::consts::PI * tau_rms) } else { f32::INFINITY };
    println!("impulse peak at {:.0} ms into recording", abs_peak as f32 / SR as f32 * 1000.0);
    println!("acoustic loopback latency (marker): {:.1} ms", acq.latency_ms);
    println!("peak correlation: {:.4}", peak_v);
    println!("RMS delay spread: {:.3} ms", tau_rms * 1000.0);
    println!("-10 dB delay width: {:.2} ms", width_10);
    println!("coherence bandwidth (1/2pi*tau_rms): {:.0} Hz", coh_bw);
    println!("=> CP/guard must exceed roughly {:.1} ms", width_10.max(tau_rms * 1000.0 * 4.0));
    Ok(())
}

fn two_tone(a: &AudioArgs, f1: f32, f2: f32, tone_ms: u64) -> Result<()> {
    let drives = [0.05f32, 0.10, 0.20, 0.30, 0.45, 0.60, 0.80, 1.00];
    let n = tone_ms as usize * SR as usize / 1000;
    let gap = SR as usize / 2;
    let mut body = Vec::new();
    let mut segs = Vec::new();
    body.extend(std::iter::repeat(0.0).take(SR as usize / 2));
    for &d in &drives {
        let s = body.len();
        body.extend(two_tone_sig(f1, f2, n, d));
        segs.push((s, body.len()));
        body.extend(std::iter::repeat(0.0).take(gap));
    }
    eprintln!("two-tone: {} & {} Hz, {} ms, {} drives", f1, f2, tone_ms, drives.len());
    let acq = acquire(a, &body, &segs)?;
    let im1 = 2.0 * f1 - f2;
    let im2 = 2.0 * f2 - f1;
    println!("\n drive    (2f1-f2)/f1 dB   (2f2-f1)/f1 dB   f2/f1 dB");
    for (i, &d) in drives.iter().enumerate() {
        let (s, e) = acq.segs[i];
        if e <= s {
            continue;
        }
        let pad = (e - s) / 6;
        let w = &acq.rec[s + pad..e - pad];
        let e1 = goertzel(w, f1, SR as f32).max(1e-12);
        let e2 = goertzel(w, f2, SR as f32).max(1e-12);
        let i1 = goertzel(w, im1, SR as f32);
        let i2 = goertzel(w, im2, SR as f32);
        println!(
            "{:>6.3}   {:>18.1}   {:>18.1}   {:>8.1}",
            d,
            10.0 * (i1 / e1).log10(),
            10.0 * (i2 / e1).log10(),
            10.0 * (e2 / e1).log10()
        );
    }
    Ok(())
}

fn tone_snr(a: &AudioArgs, tone_ms: u64) -> Result<()> {
    let freqs = [
        300.0f32, 500.0, 800.0, 1000.0, 1500.0, 2000.0, 3000.0, 4000.0, 5000.0, 6000.0, 8000.0,
        10000.0, 12000.0, 14000.0, 16000.0, 18000.0, 20000.0,
    ];
    let n = tone_ms as usize * SR as usize / 1000;
    let gap = SR as usize * 3 / 10;
    let mut body = Vec::new();
    let mut segs = Vec::new();
    body.extend(std::iter::repeat(0.0).take(SR as usize / 2));
    for &f in &freqs {
        let s = body.len();
        body.extend(tone(f, n, a.amplitude));
        segs.push((s, body.len()));
        body.extend(std::iter::repeat(0.0).take(gap));
    }
    eprintln!("tone-snr: {} tones, {} ms each, drive {:.3}", freqs.len(), tone_ms, a.amplitude);
    let acq = acquire(a, &body, &segs)?;
    println!("\n   freq_Hz    tone_dB    noise_dB    SNR_dB");
    for (i, &f) in freqs.iter().enumerate() {
        let (s, e) = acq.segs[i];
        if e <= s {
            continue;
        }
        let pad = (e - s) / 6;
        let w = &acq.rec[s + pad..e - pad];
        let tone_p = goertzel(w, f, SR as f32).max(1e-12);
        // In-band noise near the tone: detuned probes inside the symbol band.
        let probes = [f * 0.90, f * 0.95, f * 1.05, f * 1.10];
        let noise_p: f32 = probes
            .iter()
            .map(|&p| goertzel(w, p, SR as f32))
            .sum::<f32>()
            / probes.len() as f32;
        let snr = 10.0 * (tone_p / noise_p.max(1e-12)).log10();
        println!(
            "{:>9.0}   {:>8.1}   {:>9.1}   {:>7.1}",
            f,
            10.0 * tone_p.max(1e-12).log10(),
            10.0 * noise_p.max(1e-12).log10(),
            snr
        );
    }
    Ok(())
}

fn latency(a: &AudioArgs, freq: f32, click_ms: u64) -> Result<()> {
    let n = click_ms as usize * SR as usize / 1000;
    let body = tone(freq, n, a.amplitude);
    eprintln!("latency: {} Hz {} ms click, self-loopback (marker alignment)", freq, click_ms);
    let acq = acquire(a, &body, &[(0, n)])?;
    let (s, e) = acq.segs[0];
    println!("acoustic loopback latency (marker): {:.1} ms", acq.latency_ms);
    if e > s {
        println!(
            "click energy at {:.1} ms; peak {:.4}",
            (s as f32 / SR as f32) * 1000.0,
            peak(&acq.rec[s..e])
        );
    }
    Ok(())
}
