//! Non-coherent M-FSK modem for a robust, low-rate acoustic data channel.
//!
//! This is the "basics first" PHY from the link-development plan: constant
//! envelope continuous-phase FSK, detected by per-symbol Goertzel energy so it
//! never needs carrier phase, channel estimation, or a settled AGC to work.
//!
//! Design:
//! * `M` tones spaced by `sample_rate / symbol_samples` (orthogonal over one
//!   symbol window), transmitted as CPFSK (phase-continuous, constant
//!   envelope) to stay in the capture's linear region.
//! * Every frame starts with a known pseudo-random preamble used both to find
//!   the symbol grid and to reject noise bursts.
//! * Detection is grid-locked: the preamble pins the symbol phase, then one
//!   tone decision is taken per symbol period. This is what stops a noisy
//!   channel from inserting or deleting symbols.
//! * Optional time-differential M-FSK (`t = (data + prev) mod M`) so a fixed
//!   per-tone gain tilt or small frequency offset cannot bias the decisions.

use crate::bytes_to_bits;
use crate::link::crc;
use crate::physical::dtmf::goertzel;
use std::f32::consts::PI;

/// Compact byte-level Reed-Solomon FEC for FSK frames. Each block is
/// `data_block` data bytes plus `nsym` parity bytes; shortened codes are used
/// so small frames do not pay for a full 255-byte RS block.
pub struct FskFec {
    pub data_block: usize,
    pub nsym: usize,
    encoder: reed_solomon::Encoder,
    decoder: reed_solomon::Decoder,
}

impl FskFec {
    pub fn new(data_block: usize, nsym: usize) -> Self {
        Self {
            data_block: data_block.max(1),
            nsym,
            encoder: reed_solomon::Encoder::new(nsym),
            decoder: reed_solomon::Decoder::new(nsym),
        }
    }

    pub fn block_len(&self) -> usize {
        self.data_block + self.nsym
    }

    pub fn n_blocks(&self, wire_len: usize) -> usize {
        wire_len.div_ceil(self.data_block).max(1)
    }

    pub fn coded_len(&self, wire_len: usize) -> usize {
        self.n_blocks(wire_len) * self.block_len()
    }

    pub fn encode(&self, wire: &[u8]) -> Vec<u8> {
        let nb = self.n_blocks(wire.len());
        let mut out = Vec::with_capacity(nb * self.block_len());
        for b in 0..nb {
            let start = b * self.data_block;
            let mut block = vec![0u8; self.data_block];
            let end = (start + self.data_block).min(wire.len());
            if start < wire.len() {
                block[..end - start].copy_from_slice(&wire[start..end]);
            }
            let buf = self.encoder.encode(&block);
            out.extend_from_slice(buf.data());
            out.extend_from_slice(buf.ecc());
        }
        out
    }

    /// Decode exactly `n_blocks` blocks from `coded`; returns (data, all_ok).
    pub fn decode(&self, coded: &[u8], n_blocks: usize) -> (Vec<u8>, bool) {
        let bl = self.block_len();
        let mut out = Vec::with_capacity(n_blocks * self.data_block);
        let mut ok = true;
        for b in 0..n_blocks {
            let start = b * bl;
            if start + bl > coded.len() {
                ok = false;
                break;
            }
            let mut block = coded[start..start + bl].to_vec();
            match self.decoder.correct(&mut block, None) {
                Ok(c) => out.extend_from_slice(c.data()),
                Err(_) => {
                    ok = false;
                    out.extend_from_slice(&block[..self.data_block]);
                }
            }
        }
        (out, ok)
    }
}

/// Wrap a payload for FSK transmission: `[len_hi, len_lo, payload..., crc32]`.
/// The FSK preamble already provides frame sync, so this is all the framing the
/// PHY needs; CRC-32 gives end-to-end integrity.
pub fn wrap_frame(payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload.len() + 6);
    v.push((payload.len() >> 8) as u8);
    v.push((payload.len() & 0xFF) as u8);
    v.extend_from_slice(payload);
    crc::compute_and_append_crc(&mut v);
    v
}

/// Recover the payload from wrapped bytes, verifying the CRC-32.
pub fn unwrap_frame(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 6 {
        return None;
    }
    let len = ((bytes[0] as usize) << 8) | bytes[1] as usize;
    let end = 2usize.checked_add(len)?;
    if end + 4 > bytes.len() {
        return None;
    }
    if !crc::verify_crc32(&bytes[..end], &bytes[end..end + 4]) {
        return None;
    }
    Some(bytes[2..end].to_vec())
}

/// Result of demodulating one FSK frame.
#[derive(Clone, Debug)]
pub struct FskDecoded {
    /// Payload symbols (preamble stripped, differential decoded).
    pub symbols: Vec<u8>,
    /// Payload bytes (symbols packed to bytes; trailing pad bits dropped).
    pub bytes: Vec<u8>,
    /// Mean per-symbol tone SNR estimate, in dB.
    pub mean_snr_db: f32,
    /// Worst per-symbol tone SNR estimate, in dB. This is the headroom metric.
    pub min_snr_db: f32,
    /// Mean per-symbol decision confidence in 0..1.
    pub mean_confidence: f32,
    /// Number of symbols over which the stats were computed (payload only).
    pub n_symbols: usize,
    /// Whether the RS FEC decoded without flagged block failures.
    pub fec_ok: bool,
    /// Symbol-grid offset (samples into the detected burst) chosen by the demod.
    pub grid_offset: usize,
    /// Preamble symbols matched at that offset.
    pub preamble_matches: usize,
    /// Samples of `process_samples` input consumed by this frame.
    pub consumed_samples: usize,
}

impl FskDecoded {
    /// Estimated raw goodput of the payload, in bits per second, using the
    /// payload symbol count and the configured symbol rate.
    pub fn raw_bps(&self, cfg: &FskConfig) -> f32 {
        self.symbols.len() as f32 * cfg.bits_per_symbol() as f32 * cfg.symbol_rate()
    }
}

#[derive(Clone, Debug)]
pub struct FskConfig {
    pub sample_rate: u32,
    /// Number of tones (2, 4, 8, or 16).
    pub m: usize,
    /// Samples per symbol (tone burst).
    pub symbol_samples: usize,
    /// Frequency of tone 0.
    pub base_freq: f32,
    pub amplitude: f32,
    /// Number of known symbols prefixing every frame.
    pub preamble_symbols: usize,
    pub preamble_seed: u32,
    /// Transmit data as the difference from the previous symbol.
    pub differential: bool,
    /// Silence inserted before the frame body.
    pub leading_silence: usize,
    /// Silence inserted after the frame body (defines the burst boundary).
    pub guard_samples: usize,
    /// Reed-Solomon parity bytes per block (0 disables FEC).
    pub rs_nsym: usize,
    /// Data bytes per RS block when FEC is enabled.
    pub fec_data_block: usize,
    /// Fixed payload size (wire framing is sized from this).
    pub payload_size: usize,
}

impl Default for FskConfig {
    fn default() -> Self {
        // Stage-1 baseline: 2-FSK, 20 ms symbols, ~50 bps raw.
        Self {
            sample_rate: 48_000,
            m: 2,
            symbol_samples: 960,
            base_freq: 1_000.0,
            amplitude: 0.25,
            preamble_symbols: 24,
            preamble_seed: 0x5A17,
            differential: false,
            leading_silence: 2_400,
            guard_samples: 4_800,
            rs_nsym: 0,
            fec_data_block: 32,
            payload_size: 128,
        }
    }
}

impl FskConfig {
    pub fn bits_per_symbol(&self) -> usize {
        (self.m as f32).log2() as usize
    }

    pub fn tone_spacing(&self) -> f32 {
        self.sample_rate as f32 / self.symbol_samples as f32
    }

    pub fn tone_freq(&self, i: usize) -> f32 {
        self.base_freq + i as f32 * self.tone_spacing()
    }

    pub fn highest_freq(&self) -> f32 {
        self.tone_freq(self.m.saturating_sub(1))
    }

    pub fn symbol_rate(&self) -> f32 {
        self.sample_rate as f32 / self.symbol_samples as f32
    }

    pub fn raw_bps(&self) -> f32 {
        self.bits_per_symbol() as f32 * self.symbol_rate()
    }

    pub fn preamble_samples(&self) -> usize {
        self.preamble_symbols * self.symbol_samples
    }

    pub fn frame_body_samples(&self) -> usize {
        self.preamble_samples()
    }

    /// Deterministic pseudo-random preamble that contains every tone about
    /// equally often. Equal coverage lets the demodulator learn a per-tone
    /// channel gain from the preamble, which is what lets M-FSK survive the
    /// measured frequency selectivity beyond a ~350 Hz span.
    pub fn preamble(&self) -> Vec<u8> {
        let mut s = self.preamble_seed | 1;
        let mut out = Vec::with_capacity(self.preamble_symbols);
        while out.len() < self.preamble_symbols {
            // Fisher-Yates shuffle of 0..M from the xorshift stream.
            let mut perm: Vec<u8> = (0..self.m as u8).collect();
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

    /// Build a `Config` for the outer FrameAssembler/FrameParser (RS + CRC).
    pub fn framing_config(&self) -> crate::config::Config {
        let mut c = crate::config::Config::ofdm_default();
        c.rs_nsym = self.rs_nsym;
        c.payload_size = self.payload_size;
        c
    }

    /// Split bytes into base-M symbols, MSB first, zero-padded.
    pub fn bytes_to_symbols(&self, bytes: &[u8]) -> Vec<u8> {
        let bps = self.bits_per_symbol();
        let bits = bytes_to_bits(bytes);
        let mut out = Vec::with_capacity(bits.len().div_ceil(bps));
        for chunk in bits.chunks(bps) {
            let mut v = 0u8;
            for (i, &b) in chunk.iter().enumerate() {
                v |= b << (bps - 1 - i);
            }
            out.push(v & (self.m as u8 - 1));
        }
        out
    }

    /// Pack symbols into bytes, MSB first. Partial trailing symbols are padded.
    pub fn symbols_to_bytes(&self, symbols: &[u8]) -> Vec<u8> {
        let bps = self.bits_per_symbol();
        let mut bits = Vec::with_capacity(symbols.len() * bps);
        for &s in symbols {
            for i in (0..bps).rev() {
                bits.push((s >> i) & 1);
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

    /// Encode one payload symbol to its transmitted (possibly differential) value.
    pub fn map_symbol(&self, data: u8, prev: u8) -> u8 {
        if self.differential {
            (data + prev) & (self.m as u8 - 1)
        } else {
            data & (self.m as u8 - 1)
        }
    }

    /// Inverse of [`map_symbol`].
    pub fn unmap_symbol(&self, tx: u8, prev: u8) -> u8 {
        if self.differential {
            (tx + self.m as u8 - prev) & (self.m as u8 - 1)
        } else {
            tx & (self.m as u8 - 1)
        }
    }

    /// Frame body (preamble + payload symbols) as CPFSK audio, no silence.
    pub fn encode_body(&self, bytes: &[u8]) -> Vec<f32> {
        let mut symbols = self.preamble();
        let mut prev = *symbols.last().unwrap_or(&0);
        for d in self.bytes_to_symbols(bytes) {
            let tx = self.map_symbol(d, prev);
            symbols.push(tx);
            prev = tx;
        }
        self.render_symbols(&symbols, 0.0)
    }

    /// Full frame audio: leading silence + body + guard silence.
    pub fn encode_frame(&self, bytes: &[u8]) -> Vec<f32> {
        let mut out = vec![0.0f32; self.leading_silence];
        out.extend(self.encode_body(bytes));
        out.extend(std::iter::repeat(0.0).take(self.guard_samples));
        out
    }

    pub fn fec(&self) -> Option<FskFec> {
        if self.rs_nsym > 0 {
            Some(FskFec::new(self.fec_data_block, self.rs_nsym))
        } else {
            None
        }
    }

    /// Length of the wire frame (length+crc header plus payload).
    pub fn wire_len(&self) -> usize {
        self.payload_size + 6
    }

    /// Build the transmitted wire bytes for a payload: append length+crc, then
    /// RS-code if enabled. No padding to a fixed size, so a short ACK stays
    /// short instead of costing as much air time as a full DATA frame.
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

    /// FEC-decode a variable-length wire frame. The length lives in the first
    /// data block, so decode that, read it, then decode the remaining blocks.
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

    /// Encode a payload end to end (framing + optional FEC + modulation).
    pub fn encode_payload(&self, payload: &[u8]) -> Vec<f32> {
        self.encode_frame(&self.wire_bytes(payload))
    }

    fn render_symbols(&self, symbols: &[u8], phase0: f32) -> Vec<f32> {
        let n = self.symbol_samples;
        let mut out = Vec::with_capacity(symbols.len() * n);
        let mut phase = phase0;
        for &s in symbols {
            let f = self.tone_freq(s as usize);
            let step = 2.0 * PI * f / self.sample_rate as f32;
            for _ in 0..n {
                out.push(self.amplitude * phase.sin());
                phase += step;
                if phase >= 2.0 * PI {
                    phase -= 2.0 * PI;
                }
            }
        }
        out
    }

    /// Goertzel energy at every tone for one symbol window.
    pub fn tone_energies(&self, window: &[f32]) -> Vec<f32> {
        let sr = self.sample_rate as f32;
        (0..self.m)
            .map(|i| goertzel(window, self.tone_freq(i), sr))
            .collect()
    }

    fn decide(energies: &[f32]) -> (u8, f32, f32) {
        let m = energies.len().max(2);
        let mut best = 0usize;
        let mut best_e = f32::MIN;
        let mut sum = 0.0f32;
        for (i, &e) in energies.iter().enumerate() {
            sum += e;
            if e > best_e {
                best_e = e;
                best = i;
            }
        }
        let mean_other = ((sum - best_e) / (m - 1) as f32).max(1e-12);
        let snr_db = 10.0 * (best_e / mean_other).log10();
        let conf = best_e / sum.max(1e-12);
        (best as u8, conf, snr_db)
    }

    /// Decide one symbol from a window: `(symbol, confidence, snr_db)`.
    pub fn detect_symbol(&self, window: &[f32]) -> (u8, f32, f32) {
        Self::decide(&self.tone_energies(window))
    }

    /// Decide one symbol with per-tone gain correction, so a frequency-selective
    /// channel (some tones attenuated) does not bias the decision.
    pub fn detect_with_gains(&self, window: &[f32], gains: &[f32]) -> (u8, f32, f32) {
        let e = self.tone_energies(window);
        let corr: Vec<f32> = e
            .iter()
            .enumerate()
            .map(|(i, &v)| v / gains.get(i).copied().unwrap_or(1.0).max(1e-12))
            .collect();
        Self::decide(&corr)
    }
}

/// Streaming demodulator. Feed audio in arbitrary chunks; complete frames come
/// out one at a time. Silence between frames bounds each burst.
pub struct FskDemodulator {
    cfg: FskConfig,
    buffer: Vec<f32>,
}

impl FskDemodulator {
    pub fn new(cfg: FskConfig) -> Self {
        Self { cfg, buffer: Vec::new() }
    }

    pub fn config(&self) -> &FskConfig {
        &self.cfg
    }

    pub fn reset(&mut self) {
        self.buffer.clear();
    }

    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Append samples and return the next complete frame, if any.
    pub fn process_samples(&mut self, samples: &[f32]) -> Option<FskDecoded> {
        self.buffer.extend_from_slice(samples);
        // Never let a stalled stream grow without bound.
        let cap = (self.cfg.sample_rate as usize * 20).max(self.cfg.guard_samples * 40);
        if self.buffer.len() > cap {
            let excess = self.buffer.len() - cap;
            self.buffer.drain(..excess);
        }
        let slack = self.env_win();
        loop {
            let regions = self.find_regions(&self.buffer);
            if regions.is_empty() {
                // No burst yet. Keep everything: the frame may still be
                // arriving, and the top-of-function cap bounds memory.
                return None;
            }
            let (start, end) = regions[0];
            // A region that runs to the end of the buffer is still being
            // received; wait for the guard silence before judging it.
            if end + slack >= self.buffer.len() {
                return None;
            }
            let slice = self.buffer[start..end].to_vec();
            match self.decode_burst(&slice) {
                Some(mut d) => {
                    d.consumed_samples = end;
                    self.buffer.drain(..end);
                    return Some(d);
                }
                None => {
                    // Not a frame (noise burst, or a partial capture): drop it
                    // and look again once more audio has arrived.
                    self.buffer.drain(..end);
                    if self.buffer.len() < self.cfg.symbol_samples * 2 {
                        return None;
                    }
                }
            }
        }
    }

    /// Decode every complete frame in a recorded capture (offline analysis).
    pub fn decode_capture(&self, audio: &[f32]) -> Vec<FskDecoded> {
        let mut out = Vec::new();
        for (start, end) in self.find_regions(audio) {
            if let Some(mut d) = self.decode_burst(&audio[start..end]) {
                d.consumed_samples = end;
                out.push(d);
            }
        }
        out
    }

    fn env_win(&self) -> usize {
        (self.cfg.symbol_samples / 8).clamp(16, 480)
    }

    /// Contiguous active regions, each a candidate frame burst.
    pub fn find_regions(&self, audio: &[f32]) -> Vec<(usize, usize)> {
        let env_win = self.env_win();
        if audio.len() < env_win * 2 {
            return Vec::new();
        }
        let env: Vec<f32> = audio
            .chunks(env_win)
            .map(|w| w.iter().map(|v| v * v).sum::<f32>() / w.len() as f32)
            .collect();

        let mut sorted = env.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        // Noise floor from a low percentile, but frames may dominate the
        // capture (unlike DTMF, FSK has almost no internal silence), so the
        // floor must not be mistaken for the signal. Anchor the thresholds to
        // a high percentile as well and take whichever is more discriminating.
        let peak = sorted[(sorted.len() * 99 / 100).min(sorted.len() - 1)].max(1e-12);
        // Use the true minimum for the floor: a frame with a short guard is
        // >95% signal, so a percentile would mistake the signal for the floor.
        // If even the minimum is near the peak there is no silence at all, in
        // which case anchor purely to the peak.
        let mut floor = sorted[0].max(1e-12);
        if floor > peak * 0.5 {
            floor = 0.0;
        }
        let thr = (floor * 4.0).max(peak * 0.02);
        let thr_start = (floor * 8.0).max(peak * 0.08).max(thr);
        // A gap must be a real silence run, not a one-window ripple.
        let gap_tol = env_win * 3;

        let mut regions = Vec::new();
        let mut i = 0;
        while i < env.len() {
            while i < env.len() && env[i] <= thr_start {
                i += 1;
            }
            if i >= env.len() {
                break;
            }
            let mut j = i;
            let mut last_active = i;
            while j < env.len() {
                if env[j] > thr {
                    last_active = j;
                } else if (j - last_active) * env_win > gap_tol {
                    break;
                }
                j += 1;
            }
            let start = i * env_win;
            let end = ((last_active + 1) * env_win).min(audio.len());
            if end.saturating_sub(start) >= self.cfg.preamble_samples() + self.cfg.symbol_samples {
                regions.push((start, end));
            }
            i = last_active + 1;
        }
        regions
    }

    /// Decode a single active burst: find the symbol grid on the preamble, then
    /// take one decision per symbol.
    fn decode_burst(&self, region: &[f32]) -> Option<FskDecoded> {
        let n = self.cfg.symbol_samples;
        let pre = self.cfg.preamble();
        if region.len() < n * pre.len() {
            return None;
        }

        // Grid search: the offset whose preamble slots best match the known
        // preamble. Scoring on both symbol identity and confidence rejects
        // noise that happens to trigger the energy detector.
        //
        // The coarse pass steps a fraction of a symbol; tone orthogonality
        // needs single-sample alignment, so a fine pass then refines around the
        // coarse winner. Without the fine pass a half-step error leaks energy
        // between adjacent tones and corrupts marginal symbols.
        let eval = |off: usize| -> Option<(f32, usize)> {
            let mut score = 0.0f32;
            let mut matches = 0usize;
            let mut cnt = 0usize;
            for (i, &ps) in pre.iter().enumerate() {
                let a = off + i * n;
                if a + n > region.len() {
                    break;
                }
                let (sym, conf, snr) = self.cfg.detect_symbol(&region[a..a + n]);
                cnt += 1;
                if sym == ps {
                    matches += 1;
                    score += conf + 0.5 + (snr / 20.0).min(1.0);
                }
            }
            if cnt == pre.len() {
                Some((score, matches))
            } else {
                None
            }
        };

        let step = (n / 32).max(1);
        let mut best: Option<(usize, f32, usize)> = None;
        let mut off = 0usize;
        loop {
            if let Some((score, matches)) = eval(off) {
                if matches * 4 >= pre.len() * 3
                    && best.map_or(true, |(_, bs, _)| score > bs)
                {
                    best = Some((off, score, matches));
                }
            }
            if off + step > n {
                break;
            }
            off = (off + step).min(n);
        }
        let coarse = best?.0;
        let lo = coarse.saturating_sub(step);
        let hi = (coarse + step).min(n);
        let mut fine: Option<(usize, f32, usize)> = None;
        for o in lo..=hi {
            if let Some((score, matches)) = eval(o) {
                if matches * 4 >= pre.len() * 3
                    && fine.map_or(true, |(_, bs, _)| score > bs)
                {
                    fine = Some((o, score, matches));
                }
            }
        }
        let off = fine.unwrap_or((coarse, 0.0, 0)).0;

        // Estimate a per-tone gain from the preamble. Every tone appears about
        // equally there, so accumulating the received energy at each known
        // preamble tone gives a direct estimate of that tone's channel gain.
        let mut gain_acc = vec![0.0f32; self.cfg.m];
        let mut gain_cnt = vec![0usize; self.cfg.m];
        let mut tx_symbols = Vec::with_capacity(pre.len());
        let mut snrs = Vec::with_capacity(pre.len());
        let mut confs = Vec::with_capacity(pre.len());
        for (i, &ps) in pre.iter().enumerate() {
            let a = off + i * n;
            if a + n > region.len() {
                break;
            }
            let e = self.cfg.tone_energies(&region[a..a + n]);
            let (sym, conf, snr) = FskConfig::decide(&e);
            tx_symbols.push(sym);
            snrs.push(snr);
            confs.push(conf);
            let p = ps as usize;
            gain_acc[p] += e[p];
            gain_cnt[p] += 1;
        }
        if tx_symbols.len() < pre.len() {
            return None;
        }
        let matches = tx_symbols
            .iter()
            .zip(pre.iter())
            .filter(|(a, b)| a == b)
            .count();
        if matches * 4 < pre.len() * 3 {
            return None;
        }
        let present_mean = {
            let mut s = 0.0f32;
            let mut c = 0usize;
            for i in 0..self.cfg.m {
                if gain_cnt[i] > 0 {
                    s += gain_acc[i] / gain_cnt[i] as f32;
                    c += 1;
                }
            }
            if c > 0 {
                s / c as f32
            } else {
                1.0
            }
        };
        let gains: Vec<f32> = (0..self.cfg.m)
            .map(|i| {
                if gain_cnt[i] > 0 {
                    (gain_acc[i] / gain_cnt[i] as f32).max(present_mean * 0.05)
                } else {
                    present_mean
                }
            })
            .collect();

        // Slice the rest of the region on the winning grid, correcting each
        // tone by its learned gain.
        let mut a = off + pre.len() * n;
        while a + n <= region.len() {
            let (sym, conf, snr) = self.cfg.detect_with_gains(&region[a..a + n], &gains);
            tx_symbols.push(sym);
            snrs.push(snr);
            confs.push(conf);
            a += n;
        }

        // Differential decode, carrying the last preamble symbol as reference.
        let mut prev = pre[pre.len() - 1];
        let mut data_symbols = Vec::with_capacity(tx_symbols.len() - pre.len());
        let mut data_snr = Vec::with_capacity(tx_symbols.len() - pre.len());
        let mut data_conf = Vec::with_capacity(tx_symbols.len() - pre.len());
        for i in pre.len()..tx_symbols.len() {
            let d = self.cfg.unmap_symbol(tx_symbols[i], prev);
            data_symbols.push(d);
            data_snr.push(snrs[i]);
            data_conf.push(confs[i]);
            prev = tx_symbols[i];
        }
        let n_sym = data_symbols.len();
        if n_sym == 0 {
            return None;
        }
        let mean_snr = data_snr.iter().sum::<f32>() / n_sym as f32;
        let min_snr = data_snr.iter().copied().fold(f32::INFINITY, f32::min);
        let mean_conf = data_conf.iter().sum::<f32>() / n_sym as f32;
        let raw = self.cfg.symbols_to_bytes(&data_symbols);
        let (bytes, fec_ok) = match self.cfg.fec() {
            Some(fec) => self.cfg.decode_wire_fec(&raw, &fec),
            None => (raw, true),
        };
        Some(FskDecoded {
            symbols: data_symbols,
            bytes,
            mean_snr_db: mean_snr,
            min_snr_db: min_snr,
            mean_confidence: mean_conf,
            n_symbols: n_sym,
            fec_ok,
            grid_offset: off,
            preamble_matches: matches,
            consumed_samples: region.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(m: usize, symbol_samples: usize) -> FskConfig {
        FskConfig {
            m,
            symbol_samples,
            ..FskConfig::default()
        }
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
    fn clean_roundtrip_all_m() {
        for m in [2usize, 4, 8, 16] {
            let c = cfg(m, 960);
            let payload: Vec<u8> = (0..32u8).collect();
            let audio = c.encode_frame(&payload);
            let dem = FskDemodulator::new(c.clone());
            let frames = dem.decode_capture(&audio);
            assert_eq!(frames.len(), 1, "M={} expected one frame", m);
            assert_eq!(frames[0].bytes, payload, "M={} payload mismatch", m);
            assert!(frames[0].min_snr_db > 10.0, "M={} weak: {:?}", m, frames[0]);
        }
    }

    #[test]
    fn differential_roundtrip() {
        let c = FskConfig {
            m: 4,
            differential: true,
            ..cfg(4, 960)
        };
        let payload: Vec<u8> = (0..48u8).collect();
        let audio = c.encode_frame(&payload);
        let dem = FskDemodulator::new(c);
        let frames = dem.decode_capture(&audio);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].bytes, payload);
    }

    #[test]
    fn offset_and_noise_roundtrip() {
        for &snr in &[30.0f32, 20.0, 15.0, 10.0] {
            let c = cfg(4, 960);
            let payload: Vec<u8> = (0..16u8).map(|i| i.wrapping_mul(7)).collect();
            let mut audio = vec![0.0f32; 1234];
            audio.extend(c.encode_frame(&payload));
            add_noise(&mut audio, snr);
            let dem = FskDemodulator::new(c);
            let frames = dem.decode_capture(&audio);
            assert_eq!(frames.len(), 1, "snr {} expected frame", snr);
            assert_eq!(frames[0].bytes, payload, "snr {} payload mismatch", snr);
        }
    }

    #[test]
    fn multiple_frames_in_capture() {
        let c = cfg(2, 960);
        let a: Vec<u8> = (0..10u8).collect();
        let b: Vec<u8> = (100..110u8).collect();
        let mut audio = c.encode_frame(&a);
        audio.extend(c.encode_frame(&b));
        let dem = FskDemodulator::new(c);
        let frames = dem.decode_capture(&audio);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].bytes, a);
        assert_eq!(frames[1].bytes, b);
    }

    #[test]
    fn streaming_reassembles_chunked_input() {
        let c = cfg(4, 960);
        let payload: Vec<u8> = (0..24u8).collect();
        let audio = c.encode_frame(&payload);
        let mut dem = FskDemodulator::new(c);
        let mut got = Vec::new();
        for chunk in audio.chunks(777) {
            if let Some(f) = dem.process_samples(chunk) {
                got.push(f);
            }
        }
        if got.is_empty() {
            if let Some(f) = dem.process_samples(&[]) {
                got.push(f);
            }
        }
        assert_eq!(got.len(), 1, "streaming decode failed");
        assert_eq!(got[0].bytes, payload);
    }

    #[test]
    fn noise_alone_decodes_nothing() {
        let c = cfg(4, 960);
        let mut rng = 99u32;
        let mut audio = Vec::with_capacity(48_000);
        for _ in 0..48_000 {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = ((rng >> 8) as f32 / 16_777_216.0) * 2.0 - 1.0;
            audio.push(n * 0.01);
        }
        let dem = FskDemodulator::new(c);
        assert!(dem.decode_capture(&audio).is_empty());
    }

    #[test]
    fn fec_corrects_block_errors() {
        let fec = FskFec::new(32, 16);
        let data: Vec<u8> = (0..100u8).collect();
        let mut coded = fec.encode(&data);
        // Corrupt up to 8 bytes in each block (rs can fix nsym/2 per block).
        let bl = fec.block_len();
        let nb = fec.n_blocks(data.len());
        for b in 0..nb {
            for k in 0..8 {
                let i = b * bl + k * 3;
                if i < coded.len() {
                    coded[i] ^= 0xA5;
                }
            }
        }
        let (out, ok) = fec.decode(&coded, nb);
        assert!(ok, "FEC reported failure");
        assert_eq!(&out[..data.len()], &data[..], "FEC data mismatch");
    }

    #[test]
    fn payload_fec_roundtrip_through_symbols() {
        let c = FskConfig {
            m: 4,
            symbol_samples: 960,
            rs_nsym: 24,
            fec_data_block: 32,
            payload_size: 64,
            ..FskConfig::default()
        };
        let payload: Vec<u8> = (0..64u8).collect();
        let audio = c.encode_payload(&payload);
        let dem = FskDemodulator::new(c.clone());
        let frames = dem.decode_capture(&audio);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].fec_ok);
        assert_eq!(unwrap_frame(&frames[0].bytes), Some(payload));
    }

    #[test]
    fn payload_fec_roundtrip_m2() {
        let c = FskConfig {
            m: 2,
            symbol_samples: 144,
            rs_nsym: 16,
            fec_data_block: 32,
            payload_size: 32,
            ..FskConfig::default()
        };
        let payload: Vec<u8> = (0..32u8).collect();
        let audio = c.encode_payload(&payload);
        let dem = FskDemodulator::new(c.clone());
        let frames = dem.decode_capture(&audio);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].fec_ok);
        assert_eq!(unwrap_frame(&frames[0].bytes), Some(payload));
    }

    #[test]
    fn per_tone_calibration_survives_multipath() {
        // A two-path channel gives each tone a different static gain, creating
        // a frequency-selective tilt across the 16-tone set. Per-tone
        // calibration learned from the preamble must recover it.
        let c = cfg(16, 960);
        let payload: Vec<u8> = (0..16u8).collect();
        let audio = c.encode_frame(&payload);
        let d = 64usize;
        let a = 0.55f32;
        let rx: Vec<f32> = audio
            .iter()
            .enumerate()
            .map(|(n, &s)| s + if n >= d { a * audio[n - d] } else { 0.0 })
            .collect();
        let dem = FskDemodulator::new(c);
        let frames = dem.decode_capture(&rx);
        assert_eq!(frames.len(), 1, "multipath frame not detected");
        assert_eq!(frames[0].bytes, payload, "multipath payload mismatch");
    }

    #[test]
    fn bit_packing_roundtrip() {
        for m in [2usize, 4, 8, 16] {
            let c = cfg(m, 960);
            let bytes: Vec<u8> = (0..37u8).collect();
            let syms = c.bytes_to_symbols(&bytes);
            let back = c.symbols_to_bytes(&syms);
            assert_eq!(&back[..bytes.len()], &bytes[..], "M={} packing mismatch", m);
        }
    }
}
