//! Differentially-encoded PSK with a decision-feedback equalizer.
//!
//! Motivation: the room is frequency-selective (delay spread ~0.7-3 ms) and the
//! capture path has a time-varying gain. Non-coherent FSK survives but is
//! bandwidth-inefficient (~200 bps). OFDM fails because notched subcarriers
//! dominate. DPSK is the one untried family that fits: it carries phase
//! (which a real gain preserves), needs no absolute phase or channel estimate,
//! and a DFE can undo the short delay spread.
//!
//! Frame: known differentially-encoded preamble (timing + DFE training),
//! then payload symbols, RRC pulse-shaped and up-converted to a carrier.
//! Receiver: down-convert, RRC matched filter, preamble correlation for timing,
//! LMS-trained DFE, then differential detection.

use crate::bytes_to_bits;
use crate::link::crc;
use crate::physical::fsk::{unwrap_frame, wrap_frame};
use num_complex::Complex32;
use std::f32::consts::PI;

#[derive(Clone, Debug)]
pub struct DpskConfig {
    pub sample_rate: u32,
    /// Symbol (baud) rate.
    pub symbol_rate: f32,
    /// Passband carrier.
    pub carrier: f32,
    /// RRC roll-off.
    pub beta: f32,
    /// RRC span in symbols (one-sided).
    pub span: usize,
    pub amplitude: f32,
    /// 1 = DBPSK, 2 = DQPSK.
    pub bits_per_symbol: usize,
    pub preamble_symbols: usize,
    pub preamble_seed: u32,
    pub leading_silence: usize,
    pub guard_samples: usize,
    /// Feedforward / feedback DFE taps.
    pub dfe_ff: usize,
    pub dfe_fb: usize,
    pub dfe_mu: f32,
    pub payload_size: usize,
}

impl Default for DpskConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            symbol_rate: 1_000.0,
            carrier: 4_000.0,
            beta: 0.35,
            span: 6,
            amplitude: 0.3,
            bits_per_symbol: 2, // DQPSK
            preamble_symbols: 48,
            preamble_seed: 0x0517,
            leading_silence: 2_496,
            guard_samples: 4_800,
            dfe_ff: 6,
            dfe_fb: 3,
            dfe_mu: 0.01,
            payload_size: 128,
        }
    }
}

impl DpskConfig {
    pub fn sps(&self) -> usize {
        (self.sample_rate as f32 / self.symbol_rate).round() as usize
    }

    pub fn raw_bps(&self) -> f32 {
        self.symbol_rate * self.bits_per_symbol as f32
    }

    pub fn preamble_bits(&self) -> Vec<u8> {
        let mut s = self.preamble_seed | 1;
        (0..self.preamble_symbols * self.bits_per_symbol)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s & 1) as u8
            })
            .collect()
    }

    /// Gray phase change for a bit group.
    fn phase_change(&self, bits: &[u8]) -> f32 {
        if self.bits_per_symbol == 1 {
            if bits[0] == 0 { 0.0 } else { PI }
        } else {
            match (bits[0], bits[1]) {
                (0, 0) => 0.0,
                (0, 1) => PI / 2.0,
                (1, 1) => PI,
                _ => -PI / 2.0,
            }
        }
    }

    fn bits_to_phase(&self, bits: &[u8], init: Complex32) -> Vec<Complex32> {
        let mut sym = Vec::with_capacity(bits.len() / self.bits_per_symbol);
        let mut cur = init;
        for c in bits.chunks(self.bits_per_symbol) {
            let mut b = [0u8; 2];
            for (i, &v) in c.iter().enumerate() {
                b[i] = v;
            }
            cur *= Complex32::from_polar(1.0, self.phase_change(&b[..self.bits_per_symbol]));
            sym.push(cur);
        }
        sym
    }

    fn rrc(&self) -> Vec<f32> {
        rrc_taps(self.beta, self.span, self.sps())
    }

    /// Full frame audio (leading silence + body + guard).
    pub fn encode_payload(&self, payload: &[u8]) -> Vec<f32> {
        let wire = wrap_frame(payload);
        let mut bits = self.preamble_bits();
        bits.extend_from_slice(&bytes_to_bits(&wire));
        let symbols = self.bits_to_phase(&bits, Complex32::new(1.0, 0.0));
        let body = self.render(&symbols);
        let mut out = vec![0.0f32; self.leading_silence];
        out.extend(body);
        out.extend(std::iter::repeat(0.0).take(self.guard_samples));
        out
    }

    /// Pulse-shape complex symbols and up-convert to the real passband.
    fn render(&self, symbols: &[Complex32]) -> Vec<f32> {
        let sps = self.sps();
        let rrc = self.rrc();
        let n = symbols.len() * sps;
        let mut baseband = vec![Complex32::new(0.0, 0.0); n + rrc.len()];
        for (i, &s) in symbols.iter().enumerate() {
            baseband[i * sps] = s;
        }
        let shaped = convolve_complex(&baseband, &rrc);
        let mut out = Vec::with_capacity(shaped.len());
        let w = 2.0 * PI * self.carrier / self.sample_rate as f32;
        for (i, s) in shaped.iter().enumerate() {
            let ph = w * i as f32;
            out.push(self.amplitude * (s.re * ph.cos() - s.im * ph.sin()));
        }
        out
    }

    /// Expected transmitted complex symbols for a payload (for tests/diag).
    pub fn expected_symbols(&self, payload: &[u8]) -> Vec<Complex32> {
        let wire = wrap_frame(payload);
        let mut bits = self.preamble_bits();
        bits.extend_from_slice(&bytes_to_bits(&wire));
        self.bits_to_phase(&bits, Complex32::new(1.0, 0.0))
    }
}

pub fn rrc_taps(beta: f32, span: usize, sps: usize) -> Vec<f32> {
    let n = 2 * span * sps + 1;
    let mut h = vec![0.0f32; n];
    let center = (n / 2) as isize;
    for (i, v) in h.iter_mut().enumerate() {
        let t = (i as isize - center) as f32 / sps as f32;
        if t.abs() < 1e-6 {
            *v = 1.0 - beta + 4.0 * beta / PI;
        } else if (t - 1.0 / (4.0 * beta)).abs() < 1e-6 {
            *v = (beta / 2f32.sqrt())
                * ((1.0 + 2.0 / PI) * (PI / (4.0 * beta)).sin()
                    - (1.0 - 2.0 / PI) * (PI / (4.0 * beta)).cos());
        } else {
            let num = (PI * t * (1.0 - beta)).sin()
                + 4.0 * beta * t * (PI * t * (1.0 + beta)).cos();
            let den = PI * t * (1.0 - (4.0 * beta * t).powi(2));
            *v = num / den;
        }
    }
    let e: f32 = h.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    for v in h.iter_mut() {
        *v /= e;
    }
    h
}

fn convolve_complex(a: &[Complex32], b: &[f32]) -> Vec<Complex32> {
    let mut out = vec![Complex32::new(0.0, 0.0); a.len() + b.len() - 1];
    for (i, &x) in a.iter().enumerate() {
        if x == Complex32::new(0.0, 0.0) {
            continue;
        }
        for (j, &h) in b.iter().enumerate() {
            out[i + j] += x * h;
        }
    }
    out
}

#[derive(Clone, Debug)]
pub struct DpskDecoded {
    pub bytes: Vec<u8>,
    pub n_symbols: usize,
    pub mean_evm: f32,
    pub preamble_match: f32,
    pub consumed_samples: usize,
}

pub struct DpskDemodulator {
    cfg: DpskConfig,
}

impl DpskDemodulator {
    pub fn new(cfg: DpskConfig) -> Self {
        Self { cfg }
    }

    pub fn config(&self) -> &DpskConfig {
        &self.cfg
    }

    /// Decode every frame in a capture (offline analysis).
    pub fn decode_capture(&self, audio: &[f32]) -> Vec<DpskDecoded> {
        let mut out = Vec::new();
        for (s, e) in self.find_regions(audio) {
            if std::env::var("DPSK_DEBUG").is_ok() {
                eprintln!("[dpsk] region s={} e={} len={} s%48={}", s, e, e - s, s % 48);
            }
            if let Some(d) = self.decode_burst(&audio[s..e]) {
                out.push(d);
            }
        }
        out
    }

    fn find_regions(&self, audio: &[f32]) -> Vec<(usize, usize)> {
        let win = self.cfg.sps().max(16);
        if audio.len() < win * 3 {
            return Vec::new();
        }
        let env: Vec<f32> = audio
            .chunks(win)
            .map(|w| (w.iter().map(|x| x * x).sum::<f32>() / w.len() as f32).sqrt())
            .collect();
        let mut sorted = env.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let pk = sorted[sorted.len() - 1].max(1e-12);
        let mut fl = sorted[sorted.len() / 10];
        if fl > pk * 0.5 {
            fl = 0.0;
        }
        let thr = (fl * 2.0).max(pk * 0.05);
        let st = (fl * 3.0).max(pk * 0.12).max(thr);
        let mut out = Vec::new();
        let mut i = 0;
        while i < env.len() {
            while i < env.len() && env[i] <= st {
                i += 1;
            }
            if i >= env.len() {
                break;
            }
            let mut j = i;
            let mut last = i;
            while j < env.len() {
                if env[j] > thr {
                    last = j;
                } else if (j - last) * win > win * 3 {
                    break;
                }
                j += 1;
            }
            out.push((i * win, ((last + 1) * win).min(audio.len())));
            i = last + 1;
        }
        out
    }

    fn decode_burst(&self, region: &[f32]) -> Option<DpskDecoded> {
        let sps = self.cfg.sps();
        let rrc = self.cfg.rrc();
        let w = 2.0 * PI * self.cfg.carrier / self.cfg.sample_rate as f32;
        // Down-convert to complex baseband, then RRC matched filter.
        let mut bb = Vec::with_capacity(region.len());
        for (i, &x) in region.iter().enumerate() {
            let ph = -w * i as f32;
            bb.push(Complex32::new(x * ph.cos(), x * ph.sin()));
        }
        let mf = convolve_complex(&bb, &rrc);
        let delay = rrc.len() / 2;

        // Preamble reference symbols.
        let pre_bits = self.cfg.preamble_bits();
        let pre = self.cfg.bits_to_phase(&pre_bits, Complex32::new(1.0, 0.0));
        let np = pre.len();
        if np == 0 {
            return None;
        }

        // Correlate the matched-filter output against the expected preamble at
        // symbol spacing, searching timing phase and frame start.
        let mut best = (0usize, 0isize, f32::MIN);
        for phase in 0..sps {
            for start in (0..mf.len().saturating_sub(np * sps)).step_by(sps.max(1)) {
                let mut acc = Complex32::new(0.0, 0.0);
                let mut e = 0.0f32;
                for k in 0..np {
                    let idx = start + k * sps + phase;
                    if idx >= mf.len() {
                        break;
                    }
                    let r = mf[idx];
                    acc += r * pre[k].conj();
                    e += r.norm_sqr();
                }
                let score = if e > 1e-9 { acc.norm_sqr() / e } else { 0.0 };
                if score > best.2 {
                    best = (phase, start as isize, score);
                }
            }
        }
        let (phase, start, score) = best;
        if std::env::var("DPSK_DEBUG").is_ok() {
            eprintln!(
                "[dpsk] region={} mf={} best phase={} start={} score={:.3}",
                region.len(),
                mf.len(),
                phase,
                start,
                score
            );
        }
        if score < 0.05 {
            return None;
        }
        let match_frac = score.sqrt();

        // Fine timing: maximise the sampled symbol energy near the coarse
        // offset (the correlation peak is flat).
        let base0 = (start.max(0) as usize) + phase;
        let lo = base0.saturating_sub(sps / 2);
        let hi = base0 + sps / 2;
        let mut best_m = f32::MIN;
        let mut base = base0;
        for cand in lo..=hi {
            let mut acc = Complex32::new(0.0, 0.0);
            let mut k = 0;
            while k < np && cand + k * sps < mf.len() {
                acc += mf[cand + k * sps] * pre[k].conj();
                k += 1;
            }
            let m = acc.norm();
            if m > best_m {
                best_m = m;
                base = cand;
            }
        }

        let mut samples: Vec<Complex32> = Vec::new();
        let mut idx = base;
        while idx < mf.len() {
            samples.push(mf[idx]);
            idx += sps;
        }
        if samples.len() < np + 2 {
            return None;
        }
        let avg: f32 = samples[..np].iter().map(|s| s.norm()).sum::<f32>() / np as f32;
        let avg = avg.max(1e-9);
        for s in samples.iter_mut() {
            *s /= avg;
        }

        let ff = self.cfg.dfe_ff;
        let fb = self.cfg.dfe_fb;
        let mut wf = vec![Complex32::new(0.0, 0.0); ff];
        let mut wb = vec![Complex32::new(0.0, 0.0); fb];
        if ff > 0 {
            wf[0] = Complex32::new(1.0, 0.0);
        }
        let mu = self.cfg.dfe_mu;
        let mut decisions: Vec<Complex32> = Vec::new();
        let mut evm_sum = 0.0f32;
        let mut evm_n = 0usize;
        let n = samples.len();
        let mut out_syms: Vec<Complex32> = Vec::with_capacity(n);
        for k in 0..n {
            let mut y = Complex32::new(0.0, 0.0);
            for i in 0..ff {
                if k >= i {
                    y += wf[i] * samples[k - i];
                }
            }
            for j in 1..=fb {
                if k >= j {
                    y += wb[j - 1] * decisions[k - j];
                }
            }
            let target = if k < np {
                pre[k]
            } else {
                quantize(y, self.cfg.bits_per_symbol)
            };
            let e = target - y;
            evm_sum += e.norm_sqr();
            evm_n += 1;
            for i in 0..ff {
                if k >= i {
                    wf[i] += Complex32::new(mu, 0.0) * e * samples[k - i].conj();
                }
            }
            for j in 1..=fb {
                if k >= j {
                    wb[j - 1] += Complex32::new(mu, 0.0) * e * decisions[k - j].conj();
                }
            }
            decisions.push(target);
            out_syms.push(target);
        }
        if out_syms.len() <= np {
            return None;
        }

        // Differential decode the payload symbols (after the preamble).
        let mut bits: Vec<u8> = Vec::new();
        for k in np..out_syms.len() {
            let d = out_syms[k] * out_syms[k - 1].conj();
            bits.extend_from_slice(&quantize_bits(d, self.cfg.bits_per_symbol));
        }
        let n = out_syms.len();
        let bytes = crate::bits_to_bytes(&bits);
        let decoded = unwrap_frame(&bytes);
        let mean_evm = if evm_n > 0 {
            (evm_sum / evm_n as f32).sqrt()
        } else {
            0.0
        };
        if std::env::var("DPSK_DEBUG").is_ok() {
            eprintln!(
                "[dpsk] frame base={} nsym={} evm={:.3} ok={}",
                base0,
                n,
                mean_evm,
                decoded.is_some()
            );
        }
        Some(DpskDecoded {
            bytes: decoded.unwrap_or(bytes),
            n_symbols: n,
            mean_evm,
            preamble_match: match_frac,
            consumed_samples: region.len(),
        })
    }
}

/// Quantize a complex value to the nearest QPSK/BPSK constellation point.
fn quantize(y: Complex32, bps: usize) -> Complex32 {
    if bps == 1 {
        Complex32::new(if y.re >= 0.0 { 1.0 } else { -1.0 }, 0.0)
    } else {
        // Constellation is at 0/90/180/270 degrees (phase accumulation from
        // angle 0), so pick the nearest axis, not the diagonal.
        if y.re.abs() >= y.im.abs() {
            Complex32::new(if y.re >= 0.0 { 1.0 } else { -1.0 }, 0.0)
        } else {
            Complex32::new(0.0, if y.im >= 0.0 { 1.0 } else { -1.0 })
        }
    }
}

/// Differential bits from the phase of `d = r[k] * conj(r[k-1])` (Gray).
fn quantize_bits(d: Complex32, bps: usize) -> Vec<u8> {
    let mut ang = d.im.atan2(d.re);
    if ang < 0.0 {
        ang += 2.0 * PI;
    }
    if bps == 1 {
        vec![if ang < PI / 2.0 || ang >= 3.0 * PI / 2.0 { 0 } else { 1 }]
    } else {
        let q = (((ang + PI / 4.0) / (PI / 2.0)).floor() as i32).rem_euclid(4);
        match q {
            0 => vec![0, 0],
            1 => vec![0, 1],
            2 => vec![1, 1],
            _ => vec![1, 0],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> DpskConfig {
        DpskConfig::default()
    }

    fn add_noise(audio: &mut [f32], snr_db: f32) {
        let sig = audio.iter().map(|s| s * s).sum::<f32>() / audio.len().max(1) as f32;
        let noise = (sig / 10f32.powf(snr_db / 10.0)).sqrt();
        let mut rng = 0x1234_5678u32;
        for s in audio.iter_mut() {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = ((rng >> 8) as f32 / 16_777_216.0) * 2.0 - 1.0;
            *s += n * noise;
        }
    }

    #[test]
    fn clean_roundtrip() {
        for bps in [1usize, 2] {
            let c = DpskConfig { bits_per_symbol: bps, ..cfg() };
            let payload: Vec<u8> = (0..64u8).collect();
            let audio = c.encode_payload(&payload);
            let dem = DpskDemodulator::new(c);
            let frames = dem.decode_capture(&audio);
            assert_eq!(frames.len(), 1, "bps={} no frame", bps);
            assert_eq!(frames[0].bytes, payload, "bps={} payload mismatch", bps);
        }
    }

    #[test]
    fn awgn_roundtrip() {
        let c = cfg();
        let payload: Vec<u8> = (0..48u8).collect();
        for snr in [30.0f32, 20.0, 15.0, 10.0] {
            let mut audio = c.encode_payload(&payload);
            add_noise(&mut audio, snr);
            let dem = DpskDemodulator::new(c.clone());
            let frames = dem.decode_capture(&audio);
            assert_eq!(frames.len(), 1, "snr {} no frame", snr);
            assert_eq!(frames[0].bytes, payload, "snr {} mismatch", snr);
        }
    }

    #[test]
    fn multipath_roundtrip() {
        let c = cfg();
        let payload: Vec<u8> = (0..48u8).collect();
        let audio = c.encode_payload(&payload);
        // Two-path channel with a delay of ~0.5 ms (24 samples).
        for d in [12usize, 24, 48] {
            let rx: Vec<f32> = audio
                .iter()
                .enumerate()
                .map(|(n, &s)| s + if n >= d { 0.5 * audio[n - d] } else { 0.0 })
                .collect();
            let dem = DpskDemodulator::new(c.clone());
            let frames = dem.decode_capture(&rx);
            assert_eq!(frames.len(), 1, "d={} no frame", d);
            assert_eq!(frames[0].bytes, payload, "d={} mismatch", d);
        }
    }
}
