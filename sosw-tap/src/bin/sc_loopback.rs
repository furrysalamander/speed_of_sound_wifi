//! Minimal single-carrier QPSK loopback to test the PAPR hypothesis.
//!
//! The acoustic capture path has a limiter with only ~10 dB of linear range;
//! high-PAPR OFDM gets compressed no matter the level. Single-carrier QPSK has
//! ~3-4 dB PAPR, so it should stay linear. This sends RRC-shaped SC-QPSK
//! (carrier 4 kHz, 4800 baud => ~9.6 kbps) with a known preamble for timing and
//! a one-tap channel estimate, then measures BER.

use anyhow::Result;
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use num_complex::Complex32;
use std::f32::consts::PI;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FS: f32 = 48000.0;
const FC: f32 = 4000.0;
const BAUD: f32 = 4800.0;
const SPS: usize = 10; // FS / BAUD
const BETA: f32 = 0.35;
const SPAN: usize = 10; // RRC span in symbols
const PREAMBLE_SYMS: usize = 200;
const PAYLOAD_SYMS: usize = 6000;
const AMP: f32 = 0.25;

#[derive(Parser)]
#[command(name = "sc-loopback", about = "Single-carrier QPSK loopback (PAPR test)")]
struct Args {
    /// Run without audio (tx fed straight to rx)
    #[arg(long)]
    software: bool,
    #[arg(long, default_value_t = -1.0)]
    snr_db: f32,
    #[arg(long)]
    tx_device: Option<String>,
    #[arg(long)]
    rx_device: Option<String>,
    /// TX peak amplitude (before gain)
    #[arg(long, default_value_t = AMP)]
    amp: f32,
    #[arg(long, default_value_t = 1.0)]
    gain: f32,
    /// Symbol rate in baud
    #[arg(long, default_value_t = 4800.0)]
    baud: f32,
    /// Differential QPSK (no absolute carrier phase needed)
    #[arg(long)]
    diff: bool,
}

struct Lcg(u64);
impl Lcg {
    fn next_bit(&mut self) -> u8 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) & 1) as u8
    }
    fn qpsk(&mut self) -> Complex32 {
        let re = if self.next_bit() == 0 { 1.0 } else { -1.0 };
        let im = if self.next_bit() == 0 { 1.0 } else { -1.0 };
        Complex32::new(re, im) / 2f32.sqrt()
    }
}

fn rrc(beta: f32, sps: usize, span: usize) -> Vec<f32> {
    let n = (span * sps) as isize;
    let mut h = vec![0.0f32; (2 * n + 1) as usize];
    for i in -n..=n {
        let t = i as f32 / sps as f32;
        let v = if t.abs() < 1e-6 {
            1.0 - beta + 4.0 * beta / PI
        } else if (t.abs() - 1.0 / (4.0 * beta)).abs() < 1e-4 {
            // limit at the RRC singularity
            let a = (1.0 + 2.0 / PI) * (PI / 4.0).sin();
            let b = (1.0 - 2.0 / PI) * (PI / 4.0).cos();
            beta / 2f32.sqrt() * (a - b)
        } else {
            let num = (PI * t * (1.0 - beta)).sin() + 4.0 * beta * t * (PI * t * (1.0 + beta)).cos();
            let den = PI * t * (1.0 - (4.0 * beta * t).powi(2));
            num / den
        };
        h[(i + n) as usize] = v;
    }
    let e: f32 = h.iter().map(|x| x * x).sum();
    let s = e.sqrt().max(1e-12);
    for x in h.iter_mut() {
        *x /= s;
    }
    h
}

fn conv_real(x: &[f32], h: &[f32]) -> Vec<f32> {
    let mut y = vec![0.0f32; x.len() + h.len() - 1];
    for (i, &xi) in x.iter().enumerate() {
        if xi == 0.0 { continue; }
        for (j, &hj) in h.iter().enumerate() {
            y[i + j] += xi * hj;
        }
    }
    y
}

fn conv_cpx(x: &[Complex32], h: &[f32]) -> Vec<Complex32> {
    let mut y = vec![Complex32::new(0.0, 0.0); x.len() + h.len() - 1];
    for (i, xi) in x.iter().enumerate() {
        if xi.norm_sqr() == 0.0 { continue; }
        for (j, &hj) in h.iter().enumerate() {
            y[i + j] += *xi * hj;
        }
    }
    y
}

/// Build the real passband TX audio and return (audio, preamble, payload bits).
fn build_tx(h: &[f32], amp: f32, sps: usize, diff: bool) -> (Vec<f32>, Vec<Complex32>, Vec<u8>, Vec<Complex32>) {
    let mut rng = Lcg(0x5EED_1234);
    let mut syms = Vec::with_capacity(PREAMBLE_SYMS + PAYLOAD_SYMS);
    let mut preamble = Vec::with_capacity(PREAMBLE_SYMS);
    for _ in 0..PREAMBLE_SYMS {
        let s = rng.qpsk();
        preamble.push(s);
        syms.push(s);
    }
    let mut bits = Vec::with_capacity(PAYLOAD_SYMS * 2);
    let mut acc = *preamble.last().unwrap();
    for _ in 0..PAYLOAD_SYMS {
        let re = rng.next_bit();
        let im = rng.next_bit();
        bits.push(re);
        bits.push(im);
        let d = Complex32::new(
            if re == 0 { 1.0 } else { -1.0 },
            if im == 0 { 1.0 } else { -1.0 },
        ) / 2f32.sqrt();
        if diff {
            acc *= d;
            syms.push(acc);
        } else {
            syms.push(d);
        }
    }

    let mut up = vec![Complex32::new(0.0, 0.0); syms.len() * sps];
    for (k, s) in syms.iter().enumerate() {
        up[k * sps] = *s;
    }
    let bb = conv_cpx(&up, h);
    let mut tx: Vec<f32> = bb.iter().enumerate().map(|(n, c)| {
        let w = 2.0 * PI * FC * n as f32 / FS;
        c.re * w.cos() - c.im * w.sin()
    }).collect();
    let peak = tx.iter().map(|s| s.abs()).fold(0.0f32, f32::max).max(1e-12);
    for s in tx.iter_mut() {
        *s *= amp / peak;
    }
    (tx, preamble, bits, syms)
}

fn find_device(host: &cpal::Host, name: Option<&str>, output: bool) -> Result<cpal::Device> {
    match name {
        Some(n) => {
            let lo = n.to_lowercase();
            let dev = if output { host.output_devices()? } else { host.input_devices()? }
                .find(|d| d.id().map(|i| format!("{}", i).to_lowercase().contains(&lo)).unwrap_or(false))
                .ok_or_else(|| anyhow::anyhow!("no device matching '{}'", n))?;
            Ok(dev)
        }
        None => if output {
            host.default_output_device().ok_or_else(|| anyhow::anyhow!("no output"))
        } else {
            host.default_input_device().ok_or_else(|| anyhow::anyhow!("no input"))
        },
    }
}

fn record_and_play(tx: &[f32], tx_dev: Option<&str>, rx_dev: Option<&str>) -> Result<Vec<f32>> {
    let host = cpal::default_host();
    let out = find_device(&host, tx_dev, true)?;
    let inp = find_device(&host, rx_dev, false)?;
    let oc = out.default_output_config()?.config();
    let ic = inp.default_input_config()?.config();
    let och = oc.channels as usize;
    let ich = ic.channels as usize;
    eprintln!("TX {} ({} Hz {}ch) | RX {} ({} Hz {}ch)",
        out.id().map(|i| format!("{}", i)).unwrap_or_default(), oc.sample_rate, och,
        inp.id().map(|i| format!("{}", i)).unwrap_or_default(), ic.sample_rate, ich);

    let audio = Arc::new(tx.to_vec());
    let off = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let a = audio.clone();
    let o = off.clone();
    let out_stream = out.build_output_stream::<f32, _, _>(oc, move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        let base = o.load(std::sync::atomic::Ordering::Relaxed);
        let frames = data.len() / och;
        for i in 0..frames {
            let s = a.get(base + i).copied().unwrap_or(0.0);
            for c in 0..och { data[i * och + c] = s; }
        }
        o.store(base + frames, std::sync::atomic::Ordering::Relaxed);
    }, |e| eprintln!("out err {}", e), None)?;

    let buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let b = buf.clone();
    let in_stream = inp.build_input_stream::<f32, _, _>(ic, move |data: &[f32], _: &cpal::InputCallbackInfo| {
        if let Ok(mut v) = b.lock() {
            if ich > 1 {
                for f in data.chunks(ich) { v.push(f[0]); }
            } else {
                v.extend_from_slice(data);
            }
        }
    }, |e| eprintln!("in err {}", e), None)?;
    in_stream.play()?;
    std::thread::sleep(Duration::from_millis(300));
    out_stream.play()?;
    let dur = tx.len() as f64 / FS as f64;
    std::thread::sleep(Duration::from_secs_f64(dur + 0.6));
    drop(out_stream);
    std::thread::sleep(Duration::from_millis(300));
    drop(in_stream);
    let captured = buf.lock().unwrap().clone();
    Ok(captured)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let sps = (FS / args.baud).round().max(2.0) as usize;
    let h = rrc(BETA, sps, SPAN);
    let (mut tx, preamble, bits, syms) = build_tx(&h, args.amp * args.gain, sps, args.diff);
    if args.gain != 1.0 {
        for s in tx.iter_mut() { *s *= 1.0; } // gain already folded into build_tx amp
    }
    let peak = tx.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    let rms = (tx.iter().map(|s| s * s).sum::<f32>() / tx.len() as f32).sqrt();
    eprintln!("TX: {} symbols, {:.2}s, PAPR={:.1} dB peak={:.3} rms={:.3}",
        PREAMBLE_SYMS + PAYLOAD_SYMS, tx.len() as f64 / FS as f64,
        20.0 * (peak / rms).log10(), peak, rms);

    let captured: Vec<f32> = if args.software {
        let mut c = tx.clone();
        if args.snr_db > 0.0 {
            let p = rms;
            let n = (p / 10f32.powf(args.snr_db / 10.0)).sqrt();
            let mut rng = Lcg(0xC0FFEE);
            for s in c.iter_mut() {
                let u = (rng.0 >> 11) as f32 / (1u64 << 53) as f32;
                *s += (u * 2.0 - 1.0) * n;
            }
        }
        c
    } else {
        record_and_play(&tx, args.tx_device.as_deref(), args.rx_device.as_deref())?
    };

    // Downconvert + matched filter
    let n = captured.len();
    let mut i = vec![0.0f32; n];
    let mut q = vec![0.0f32; n];
    for (k, &x) in captured.iter().enumerate() {
        let w = 2.0 * PI * FC * k as f32 / FS;
        i[k] = x * w.cos() * 2.0;
        q[k] = -x * w.sin() * 2.0;
    }
    let fi = conv_real(&i, &h);
    let fq = conv_real(&q, &h);
    let y: Vec<Complex32> = (0..fi.len().min(fq.len())).map(|k| Complex32::new(fi[k], fq[k])).collect();

    // onset: first block with meaningful energy
    let w = 480;
    let energies: Vec<f32> = (0..y.len().saturating_sub(w)).step_by(w)
        .map(|s| y[s..s + w].iter().map(|c| c.norm_sqr()).sum()).collect();
    let max_e = energies.iter().cloned().fold(0.0f32, f32::max).max(1e-12);
    let onset = energies.iter().position(|&e| e > 0.1 * max_e).map(|i| i * w).unwrap_or(0);
    // search a window for the preamble
    let lo = onset.saturating_sub(4 * SPAN * sps);
    let hi = (onset + 4 * SPAN * sps).min(y.len().saturating_sub(PREAMBLE_SYMS * sps));
    let mut best = (0.0f32, lo);
    for start in lo..hi {
        let mut acc = Complex32::new(0.0, 0.0);
        for k in 0..PREAMBLE_SYMS {
            let idx = start + k * sps;
            if idx >= y.len() { break; }
            acc += y[idx] * preamble[k].conj();
        }
        let m = acc.norm();
        if m > best.0 { best = (m, start); }
    }
    let start = best.1;
    // channel estimate from preamble
    let mut num = Complex32::new(0.0, 0.0);
    let mut den = 0.0f32;
    for k in 0..PREAMBLE_SYMS {
        let idx = start + k * sps;
        if idx >= y.len() { break; }
        num += y[idx] * preamble[k].conj();
        den += preamble[k].norm_sqr();
    }
    let hhat = num / den.max(1e-12);
    eprintln!("sync: onset={} start={} |H|={:.4} arg={:+.3} corr={:.1}", onset, start, hhat.norm(), hhat.arg(), best.0);

    // demap payload
    let mut errs = 0usize;
    let mut total = 0usize;
    let mut evm = 0.0f32;
    let mut cnt = 0usize;
    let mut prev = if start + (PREAMBLE_SYMS - 1) * sps < y.len() {
        y[start + (PREAMBLE_SYMS - 1) * sps]
    } else {
        Complex32::new(0.0, 0.0)
    };
    for j in 0..PAYLOAD_SYMS {
        let idx = start + (PREAMBLE_SYMS + j) * sps;
        if idx >= y.len() { break; }
        let dec = if args.diff {
            let d = y[idx] * prev.conj();
            prev = y[idx];
            d
        } else {
            y[idx] / (hhat + Complex32::new(1e-12, 0.0))
        };
        let e = dec.norm() * 2f32.sqrt();
        evm += (e - 1.0).powi(2);
        cnt += 1;
        let re = if dec.re >= 0.0 { 0 } else { 1 };
        let im = if dec.im >= 0.0 { 0 } else { 1 };
        if re != bits[j * 2] { errs += 1; }
        if im != bits[j * 2 + 1] { errs += 1; }
        total += 2;
    }
    // per-symbol channel y/s over payload, for offline-style diagnostics
    {
        let mut hs: Vec<Complex32> = Vec::new();
        for j in 0..PAYLOAD_SYMS {
            let idx = start + (PREAMBLE_SYMS + j) * sps;
            if idx >= y.len() { break; }
            hs.push(y[idx] / syms[PREAMBLE_SYMS + j]);
        }
        if hs.len() > 10 {
            let mags: Vec<f32> = hs.iter().map(|c| c.norm()).collect();
            let mm = mags.iter().sum::<f32>() / mags.len() as f32;
            let mstd = (mags.iter().map(|m| (m - mm).powi(2)).sum::<f32>() / mags.len() as f32).sqrt();
            let ph: Vec<f32> = hs.iter().map(|c| c.arg()).collect();
            // unwrap + linear fit
            let mut up = vec![0.0f32; ph.len()];
            up[0] = ph[0];
            let mut off = 0.0;
            for k in 1..ph.len() {
                let mut d = ph[k] - ph[k - 1];
                while d > PI { d -= 2.0 * PI; }
                while d < -PI { d += 2.0 * PI; }
                off += d;
                up[k] = up[0] + off;
            }
            let n = up.len() as f32;
            let mk = (up.len() - 1) as f32 / 2.0;
            let my = up.iter().sum::<f32>() / n;
            let mut num = 0.0; let mut den = 0.0;
            for (k, &v) in up.iter().enumerate() {
                num += (k as f32 - mk) * (v - my);
                den += (k as f32 - mk).powi(2);
            }
            let slope = num / den.max(1e-9);
            let resid: Vec<f32> = up.iter().enumerate().map(|(k, &v)| v - (my + slope * (k as f32 - mk))).collect();
            let rstd = (resid.iter().map(|r| r * r).sum::<f32>() / n).sqrt();
            eprintln!("diag: |H| mean={:.4} std/mean={:.3}  phase slope={:.5} rad/sym ({:.1} Hz)  resid std={:.3} rad",
                mm, mstd / mm.max(1e-9), slope, slope / (2.0 * PI) * args.baud, rstd);
        }
    }
    let ber = errs as f64 / total.max(1) as f64;
    let evm_rms = (evm / cnt.max(1) as f32).sqrt();
    eprintln!("RESULT: symbols={} BER={:.4} ({} err) EVM~{:.1}% (~{:.1} dB SNR) bps~{:.0}",
        PAYLOAD_SYMS, ber, errs, evm_rms * 100.0,
        if evm_rms > 0.0 { 20.0 * (1.0 / evm_rms).log10() } else { 99.0 },
        args.baud * 2.0 * (1.0 - ber as f32).max(0.0));
    Ok(())
}
