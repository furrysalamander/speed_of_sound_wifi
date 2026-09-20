//! Parallel multi-tone M-FSK: many narrowband FSK sub-channels transmitted at
//! once across a wide acoustic band.
//!
//! The measured channel only behaves coherently over a span of ~350 Hz, which
//! caps a single wideband M-FSK stream at ~333 bps. Rather than fight that with
//! coherent equalization on a time-varying channel, this sends many independent
//! narrowband 2-FSK sub-channels in parallel, each occupying a coherent span.
//! Every tone sits on a single `sample_rate/symbol_samples` grid, so all tones
//! are mutually orthogonal over one symbol and sub-channels do not leak into
//! each other even when adjacent.

use crate::bytes_to_bits;
use crate::physical::dtmf::goertzel;
use crate::physical::fsk::{wrap_frame, FskFec};
use std::f32::consts::PI;

/// Result of demodulating one parallel-FSK frame.
#[derive(Clone, Debug)]
pub struct BankDecoded {
    pub bytes: Vec<u8>,
    pub n_periods: usize,
    pub n_channels: usize,
    pub mean_snr_db: f32,
    pub min_snr_db: f32,
    pub mean_confidence: f32,
    pub fec_ok: bool,
    /// Per-channel mean tone SNR over the preamble (sounding).
    pub per_channel_snr: Vec<f32>,
    /// Per-channel preamble symbol match fraction (sounding).
    pub per_channel_match: Vec<f32>,
    pub consumed_samples: usize,
}

#[derive(Clone, Debug)]
pub struct FskBankConfig {
    pub sample_rate: u32,
    pub n_channels: usize,
    /// Tones per sub-channel (2 or 4). 2 is the robust default.
    pub tones_per_channel: usize,
    pub symbol_samples: usize,
    pub base_freq: f32,
    /// Grid units between adjacent tones inside a channel.
    pub tone_stride: usize,
    /// Grid units between the starts of adjacent channels (guard if larger).
    pub channel_stride: usize,
    /// Optional explicit absolute base frequency per channel. When non-empty
    /// this overrides base_freq/channel_stride, letting link training place
    /// channels only on carriers that survived the room's notches.
    pub carrier_freqs: Vec<f32>,
    pub amplitude: f32,
    pub preamble_symbols: usize,
    pub preamble_seed: u32,
    pub leading_silence: usize,
    pub guard_samples: usize,
    /// Reed-Solomon parity bytes per block (0 disables FEC).
    pub rs_nsym: usize,
    pub fec_data_block: usize,
    pub payload_size: usize,
}

impl Default for FskBankConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            n_channels: 4,
            tones_per_channel: 2,
            symbol_samples: 144, // 3 ms -> 333 Hz grid
            base_freq: 1_200.0,
            tone_stride: 1,
            channel_stride: 3, // one guard tone between channels
            carrier_freqs: Vec::new(),
            amplitude: 0.5,
            preamble_symbols: 24,
            preamble_seed: 0xBA11,
            leading_silence: 2_400,
            guard_samples: 4_800,
            rs_nsym: 0,
            fec_data_block: 32,
            payload_size: 64,
        }
    }
}

impl FskBankConfig {
    pub fn bits_per_channel_symbol(&self) -> usize {
        (self.tones_per_channel as f32).log2() as usize
    }

    pub fn grid_spacing(&self) -> f32 {
        self.sample_rate as f32 / self.symbol_samples as f32
    }

    pub fn tone_freq(&self, channel: usize, tone: usize) -> f32 {
        if !self.carrier_freqs.is_empty() {
            self.carrier_freqs[channel] + tone as f32 * self.grid_spacing()
        } else {
            let idx = channel * self.channel_stride + tone * self.tone_stride;
            self.base_freq + idx as f32 * self.grid_spacing()
        }
    }

    /// Number of channels actually used (explicit carriers override n_channels).
    pub fn channels(&self) -> usize {
        if self.carrier_freqs.is_empty() {
            self.n_channels
        } else {
            self.carrier_freqs.len()
        }
    }

    pub fn highest_freq(&self) -> f32 {
        self.tone_freq(
            self.channels().saturating_sub(1),
            self.tones_per_channel.saturating_sub(1),
        )
    }

    pub fn channel_span(&self) -> f32 {
        (self.tones_per_channel.saturating_sub(1)) as f32 * self.grid_spacing()
    }

    pub fn raw_bps(&self) -> f32 {
        (self.channels() * self.bits_per_channel_symbol()) as f32
            * self.sample_rate as f32
            / self.symbol_samples as f32
    }

    pub fn preamble_samples(&self) -> usize {
        self.preamble_symbols * self.symbol_samples
    }

    /// Preamble symbol sequence, applied identically to every channel. It is a
    /// shuffled permutation (not `i % m`, which is periodic and makes the grid
    /// search lock onto the wrong symbol period) so it both timings-locks
    /// unambiguously and covers each tone about equally for gain learning.
    pub fn preamble(&self) -> Vec<u8> {
        let m = self.tones_per_channel;
        let mut s = self.preamble_seed | 1;
        let mut out = Vec::with_capacity(self.preamble_symbols);
        while out.len() < self.preamble_symbols {
            let mut perm: Vec<u8> = (0..m as u8).collect();
            for i in (1..perm.len()).rev() {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                let j = (s as usize) % (i + 1);
                perm.swap(i, j);
            }
            out.extend_from_slice(&perm);
        }
        out.truncate(self.preamble_symbols);
        out
    }

    /// Distribute a byte payload across channels, MSB-first, one symbol per
    /// channel per period.
    pub fn bytes_to_matrix(&self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let bps = self.bits_per_channel_symbol();
        let bits = bytes_to_bits(bytes);
        let per_period = self.channels() * bps;
        let mut rows = Vec::new();
        let mut i = 0;
        while i < bits.len() {
            let mut row = vec![0u8; self.channels()];
            for (c, slot) in row.iter_mut().enumerate() {
                let mut v = 0u8;
                for b in 0..bps {
                    let idx = i + c * bps + b;
                    let bit = if idx < bits.len() { bits[idx] } else { 0 };
                    v = (v << 1) | bit;
                }
                *slot = v;
            }
            rows.push(row);
            i += per_period;
        }
        rows
    }

    /// Inverse of [`bytes_to_matrix`].
    pub fn matrix_to_bytes(&self, rows: &[Vec<u8>]) -> Vec<u8> {
        let bps = self.bits_per_channel_symbol();
        let mut bits = Vec::new();
        for row in rows {
            for &s in row {
                for i in (0..bps).rev() {
                    bits.push((s >> i) & 1);
                }
            }
        }
        let n_bytes = bits.len() / 8;
        let mut out = Vec::with_capacity(n_bytes);
        for c in bits[..n_bytes * 8].chunks(8) {
            let mut v = 0u8;
            for &b in c {
                v = (v << 1) | b;
            }
            out.push(v);
        }
        out
    }

    pub fn fec(&self) -> Option<FskFec> {
        if self.rs_nsym > 0 {
            Some(FskFec::new(self.fec_data_block, self.rs_nsym))
        } else {
            None
        }
    }

    pub fn wire_len(&self) -> usize {
        self.payload_size + 6
    }

    pub fn wire_bytes(&self, payload: &[u8]) -> Vec<u8> {
        let p = if payload.len() > self.payload_size {
            &payload[..self.payload_size]
        } else {
            payload
        };
        let wire = wrap_frame(p);
        match self.fec() {
            Some(fec) => fec.encode(&wire),
            None => wire,
        }
    }

    /// FEC-decode a variable-length wire frame (length lives in block 0).
    pub fn decode_wire_fec(&self, raw: &[u8], fec: &FskFec) -> (Vec<u8>, bool) {
        let bl = fec.block_len();
        if raw.len() < bl {
            return (raw.to_vec(), false);
        }
        let (first, ok1) = fec.decode(&raw[..bl], 1);
        if first.len() < 2 {
            return (first, false);
        }
        let len = ((first[0] as usize) << 8) | first[1] as usize;
        let wire_len = 2 + len + 4;
        let nb = fec.n_blocks(wire_len);
        let need = (nb * bl).min(raw.len());
        let (data, ok2) = fec.decode(&raw[..need], nb);
        let wire = data[..wire_len.min(data.len())].to_vec();
        (wire, ok1 && ok2)
    }

    pub fn encode_payload(&self, payload: &[u8]) -> Vec<f32> {
        self.encode_frame(&self.wire_bytes(payload))
    }

    pub fn encode_body(&self, bytes: &[u8]) -> Vec<f32> {
        let pre = self.preamble();
        let payload = self.bytes_to_matrix(bytes);
        let n = self.symbol_samples;
        let gain = self.amplitude / (self.channels() as f32).sqrt();
        let mut phases = vec![0.0f32; self.channels()];
        let mut out = Vec::with_capacity((pre.len() + payload.len()) * n);

        let mut period = |row: &[u8], out: &mut Vec<f32>| {
            for i in 0..n {
                let mut acc = 0.0f32;
                for c in 0..self.channels() {
                    let f = self.tone_freq(c, (row[c] as usize) % self.tones_per_channel);
                    acc += phases[c].sin();
                    phases[c] += 2.0 * PI * f / self.sample_rate as f32;
                    if phases[c] >= 2.0 * PI {
                        phases[c] -= 2.0 * PI;
                    }
                }
                out.push(acc * gain);
                let _ = i;
            }
        };

        for &p in &pre {
            let row = vec![p; self.channels()];
            period(&row, &mut out);
        }
        for row in &payload {
            period(row, &mut out);
        }
        out
    }

    pub fn encode_frame(&self, bytes: &[u8]) -> Vec<f32> {
        let mut out = vec![0.0f32; self.leading_silence];
        out.extend(self.encode_body(bytes));
        out.extend(std::iter::repeat(0.0).take(self.guard_samples));
        out
    }
}

pub struct FskBankDemodulator {
    cfg: FskBankConfig,
    buffer: Vec<f32>,
}

impl FskBankDemodulator {
    pub fn new(cfg: FskBankConfig) -> Self {
        Self { cfg, buffer: Vec::new() }
    }

    pub fn config(&self) -> &FskBankConfig {
        &self.cfg
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
    }

    pub fn process_samples(&mut self, samples: &[f32]) -> Option<BankDecoded> {
        self.buffer.extend_from_slice(samples);
        let cap = self.cfg.sample_rate as usize * 20;
        if self.buffer.len() > cap {
            let excess = self.buffer.len() - cap;
            self.buffer.drain(..excess);
        }
        let regions = self.find_regions(&self.buffer);
        if regions.is_empty() {
            return None;
        }
        let (start, end) = regions[0];
        // If the region runs to the end of the buffer it may still be arriving;
        // wait for its guard silence before judging it. Draining here would
        // throw away the beginning of a real frame (its preamble).
        let slack = self.env_win();
        if end + slack >= self.buffer.len() {
            return None;
        }
        if let Some(mut d) = self.decode_burst(&self.buffer[start..end]) {
            d.consumed_samples = end;
            self.buffer.drain(..end);
            return Some(d);
        }
        // A complete region that is not a frame: drop it and keep listening.
        self.buffer.drain(..end);
        None
    }

    pub fn decode_capture(&self, audio: &[f32]) -> Vec<BankDecoded> {
        let mut out = Vec::new();
        for (start, end) in self.find_regions(audio) {
            if let Some(d) = self.decode_burst(&audio[start..end]) {
                out.push(d);
            }
        }
        out
    }

    fn env_win(&self) -> usize {
        (self.cfg.symbol_samples / 2).clamp(8, 480)
    }

    pub fn find_regions(&self, audio: &[f32]) -> Vec<(usize, usize)> {
        let win = self.env_win();
        if audio.len() < win * 3 {
            return Vec::new();
        }
        let env: Vec<f32> = audio
            .chunks(win)
            .map(|w| w.iter().map(|v| v * v).sum::<f32>() / w.len() as f32)
            .collect();
        let mut sorted = env.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let pk = sorted[sorted.len() - 1].max(1e-12);
        // Floor from a low percentile, NOT the minimum: a single quiet sample
        // in a noisy recording would drag the minimum to ~0 and make the
        // threshold too low, so an ambient-filled guard never registers as
        // silence and consecutive frames merge into one region.
        let mut fl = sorted[sorted.len() / 10];
        if fl > pk * 0.5 {
            fl = 0.0;
        }
        let thr = (fl * 2.0).max(pk * 0.05).max(1e-9);
        let thr_start = (fl * 3.0).max(pk * 0.12).max(thr);
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
            let s = start * win;
            let e = ((last + 1) * win).min(audio.len());
            if e - s >= self.cfg.preamble_samples() + self.cfg.symbol_samples {
                out.push((s, e));
            }
            i = last + 1;
        }
        out
    }

    fn tone_energies(&self, window: &[f32], channel: usize) -> Vec<f32> {
        let sr = self.cfg.sample_rate as f32;
        (0..self.cfg.tones_per_channel)
            .map(|t| goertzel(window, self.cfg.tone_freq(channel, t), sr))
            .collect()
    }

    fn decide(energies: &[f32], gains: &[f32]) -> (u8, f32, f32) {
        let m = energies.len().max(2);
        let mut best = 0usize;
        let mut best_e = f32::MIN;
        let mut sum = 0.0f32;
        let mut corr = Vec::with_capacity(m);
        for (i, &e) in energies.iter().enumerate() {
            let c = e / gains.get(i).copied().unwrap_or(1.0).max(1e-12);
            corr.push(c);
            sum += c;
            if c > best_e {
                best_e = c;
                best = i;
            }
        }
        let mean_other = ((sum - best_e) / (m - 1) as f32).max(1e-12);
        ((best as u8), best_e / sum.max(1e-12), 10.0 * (best_e / mean_other).log10())
    }

    fn decode_burst(&self, region: &[f32]) -> Option<BankDecoded> {
        let n = self.cfg.symbol_samples;
        let pre = self.cfg.preamble();

        // Grid search across all channels simultaneously. Score a candidate
        // offset by how many channel/preamble slots match.
        let total_slots = pre.len() * self.cfg.channels();
        let score = |off: usize| -> (usize, usize) {
            let mut matches = 0usize;
            let mut seen = 0usize;
            for (i, &ps) in pre.iter().enumerate() {
                let a = off + i * n;
                if a + n > region.len() {
                    break;
                }
                for c in 0..self.cfg.channels() {
                    let e = self.tone_energies(&region[a..a + n], c);
                    let ones = vec![1.0f32; e.len()];
                    let (sym, _, _) = Self::decide(&e, &ones);
                    seen += 1;
                    if sym == ps % self.cfg.tones_per_channel as u8 {
                        matches += 1;
                    }
                }
            }
            (matches, seen)
        };

        let step = (n / 24).max(1);
        let mut best: Option<(usize, usize)> = None;
        let mut off = 0usize;
        loop {
            let (matches, seen) = score(off);
            if seen == total_slots {
                if best.map_or(true, |(_, bm)| matches > bm) {
                    best = Some((off, matches));
                }
            }
            if off + step > n {
                break;
            }
            off = (off + step).min(n);
        }
        let (coarse, _) = best?;
        // Fine refinement to single-sample alignment. Without this, a
        // half-step error breaks tone orthogonality and adjacent bank channels
        // leak into each other.
        let lo = coarse.saturating_sub(step);
        let hi = (coarse + step).min(n);
        let mut best_fine = (coarse, usize::MIN);
        for o in lo..=hi {
            let (matches, seen) = score(o);
            if seen == total_slots && matches > best_fine.1 {
                best_fine = (o, matches);
            }
        }
        let off = best_fine.0;
        if best_fine.1 * 5 < total_slots * 2 {
            return None;
        }

        // Learn per-tone gains per channel from the preamble.
        let mut gains = vec![vec![1.0f32; self.cfg.tones_per_channel]; self.cfg.channels()];
        for c in 0..self.cfg.channels() {
            let mut acc = vec![0.0f32; self.cfg.tones_per_channel];
            let mut cnt = vec![0usize; self.cfg.tones_per_channel];
            for (i, &ps) in pre.iter().enumerate() {
                let a = off + i * n;
                if a + n > region.len() {
                    break;
                }
                let e = self.tone_energies(&region[a..a + n], c);
                let t = ps as usize % self.cfg.tones_per_channel;
                acc[t] += e[t];
                cnt[t] += 1;
            }
            let mean = {
                let mut s = 0.0;
                let mut k = 0;
                for t in 0..self.cfg.tones_per_channel {
                    if cnt[t] > 0 {
                        s += acc[t] / cnt[t] as f32;
                        k += 1;
                    }
                }
                if k > 0 {
                    s / k as f32
                } else {
                    1.0
                }
            };
            for t in 0..self.cfg.tones_per_channel {
                gains[c][t] = if cnt[t] > 0 {
                    (acc[t] / cnt[t] as f32).max(mean * 0.05)
                } else {
                    mean
                };
            }
        }

        // Per-channel preamble statistics (used for carrier sounding).
        let mut per_ch_snr = vec![0.0f32; self.cfg.channels()];
        let mut per_ch_match = vec![0.0f32; self.cfg.channels()];
        for c in 0..self.cfg.channels() {
            let mut snr_sum = 0.0f32;
            let mut matches = 0usize;
            let mut cnt = 0usize;
            for (i, &ps) in pre.iter().enumerate() {
                let a = off + i * n;
                if a + n > region.len() {
                    break;
                }
                let e = self.tone_energies(&region[a..a + n], c);
                let (sym, _, snr) = Self::decide(&e, &gains[c]);
                snr_sum += snr;
                cnt += 1;
                if sym == ps {
                    matches += 1;
                }
            }
            if cnt > 0 {
                per_ch_snr[c] = snr_sum / cnt as f32;
                per_ch_match[c] = matches as f32 / cnt as f32;
            }
        }

        // Decode payload periods.
        let n_periods = (region.len().saturating_sub(off + pre.len() * n)) / n;
        if n_periods == 0 {
            return None;
        }
        let mut rows = Vec::with_capacity(n_periods);
        let mut snrs = Vec::new();
        let mut confs = Vec::new();
        for p in 0..n_periods {
            let a = off + (pre.len() + p) * n;
            let mut row = vec![0u8; self.cfg.channels()];
            for c in 0..self.cfg.channels() {
                let e = self.tone_energies(&region[a..a + n], c);
                let (sym, conf, snr) = Self::decide(&e, &gains[c]);
                row[c] = sym;
                snrs.push(snr);
                confs.push(conf);
            }
            rows.push(row);
        }
        let raw = self.cfg.matrix_to_bytes(&rows);
        let (bytes, fec_ok) = match self.cfg.fec() {
            Some(fec) => self.cfg.decode_wire_fec(&raw, &fec),
            None => (raw, true),
        };
        let mean_snr = snrs.iter().sum::<f32>() / snrs.len().max(1) as f32;
        let min_snr = snrs.iter().copied().fold(f32::INFINITY, f32::min);
        let mean_conf = confs.iter().sum::<f32>() / confs.len().max(1) as f32;
        Some(BankDecoded {
            bytes,
            n_periods,
            n_channels: self.cfg.channels(),
            mean_snr_db: mean_snr,
            min_snr_db: if min_snr.is_finite() { min_snr } else { 0.0 },
            mean_confidence: mean_conf,
            fec_ok,
            per_channel_snr: per_ch_snr,
            per_channel_match: per_ch_match,
            consumed_samples: region.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> FskBankConfig {
        FskBankConfig::default()
    }

    #[test]
    fn clean_roundtrip_software() {
        for channels in [1usize, 2, 4, 8] {
            let c = FskBankConfig {
                n_channels: channels,
                ..cfg()
            };
            let payload: Vec<u8> = (0..32u8).collect();
            let audio = c.encode_frame(&crate::physical::fsk::wrap_frame(&payload));
            let dem = FskBankDemodulator::new(c);
            let frames = dem.decode_capture(&audio);
            assert_eq!(frames.len(), 1, "channels={} not decoded", channels);
            let got = crate::physical::fsk::unwrap_frame(&frames[0].bytes);
            assert_eq!(got, Some(payload), "channels={} payload mismatch", channels);
        }
    }

    #[test]
    fn roundtrip_with_noise() {
        let c = cfg();
        let payload: Vec<u8> = (0..16u8).collect();
        let mut audio = c.encode_frame(&crate::physical::fsk::wrap_frame(&payload));
        let sig = audio.iter().map(|s| s * s).sum::<f32>() / audio.len() as f32;
        let noise = (sig / 10f32.powf(20.0 / 10.0)).sqrt();
        let mut rng = 7u32;
        for s in audio.iter_mut() {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            *s += ((rng >> 8) as f32 / 16_777_216.0 - 0.5) * 2.0 * noise;
        }
        let dem = FskBankDemodulator::new(c);
        let frames = dem.decode_capture(&audio);
        assert_eq!(frames.len(), 1);
        assert_eq!(
            crate::physical::fsk::unwrap_frame(&frames[0].bytes),
            Some(payload)
        );
    }

    #[test]
    fn streaming_reassembles_chunked_input() {
        let c = FskBankConfig::default();
        let payload: Vec<u8> = (0..16u8).collect();
        let audio = c.encode_frame(&crate::physical::fsk::wrap_frame(&payload));
        let mut dem = FskBankDemodulator::new(c);
        let mut got = None;
        for chunk in audio.chunks(500) {
            if let Some(f) = dem.process_samples(chunk) {
                got = Some(f);
                break;
            }
        }
        let f = got.expect("streaming bank decode failed");
        assert_eq!(
            crate::physical::fsk::unwrap_frame(&f.bytes),
            Some(payload)
        );
    }

    #[test]
    fn matrix_roundtrip() {
        let c = cfg();
        let bytes: Vec<u8> = (0..33u8).collect();
        let rows = c.bytes_to_matrix(&bytes);
        let back = c.matrix_to_bytes(&rows);
        assert_eq!(&back[..bytes.len()], &bytes[..]);
    }
}
