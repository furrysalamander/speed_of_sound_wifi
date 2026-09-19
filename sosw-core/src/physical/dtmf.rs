//! DTMF tone-pair modulation for a robust, low-rate acoustic control channel.
//!
//! Standard telephone DTMF: one tone from a low group and one from a high
//! group are summed per symbol. Narrowband detection (Goertzel) integrates
//! over the whole symbol, so this survives the low-frequency ambient noise
//! and limited mic response that break wideband OFDM on built-in laptop mics.
//!
//! 16 symbols = 4 bits each. Intended as a link-training / control channel,
//! not bulk data.

pub const LOW_FREQS: [f32; 4] = [697.0, 770.0, 852.0, 941.0];
pub const HIGH_FREQS: [f32; 4] = [1209.0, 1336.0, 1477.0, 1633.0];

/// All eight DTMF frequencies, low group first.
pub const ALL_FREQS: [f32; 8] = [
    LOW_FREQS[0],
    LOW_FREQS[1],
    LOW_FREQS[2],
    LOW_FREQS[3],
    HIGH_FREQS[0],
    HIGH_FREQS[1],
    HIGH_FREQS[2],
    HIGH_FREQS[3],
];

#[derive(Clone, Debug)]
pub struct DtmfConfig {
    pub sample_rate: u32,
    /// Duration of the tone burst for one symbol.
    pub symbol_samples: usize,
    /// Silence between symbols.
    pub gap_samples: usize,
    pub amplitude: f32,
}

impl Default for DtmfConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48000,
            // 100 ms tone / 100 ms gap = 4 bits per 200 ms (~20 bps). The gap
            // is at least one symbol so adjacent identical symbols stay
            // separable even when the channel adds noise in the gaps.
            symbol_samples: 4800,
            gap_samples: 4800,
            amplitude: 0.4,
        }
    }
}

/// Map a nibble (0-15) to its low/high tone pair.
pub fn symbol_freqs(sym: u8) -> (f32, f32) {
    let s = (sym & 0x0F) as usize;
    (LOW_FREQS[s / 4], HIGH_FREQS[s % 4])
}

/// Generate the audio for one DTMF symbol (no gap).
pub fn encode_symbol(sym: u8, cfg: &DtmfConfig) -> Vec<f32> {
    let (flo, fhi) = symbol_freqs(sym);
    let n = cfg.symbol_samples;
    let mut out = Vec::with_capacity(n);
    let ramp = (cfg.sample_rate as usize / 200).max(1); // 5 ms raised edges
    for i in 0..n {
        let t = i as f32 / cfg.sample_rate as f32;
        let mut s = 0.5 * (2.0 * std::f32::consts::PI * flo * t).sin()
            + 0.5 * (2.0 * std::f32::consts::PI * fhi * t).sin();
        // raised-cosine onset/release to avoid clicks
        if i < ramp {
            s *= 0.5 * (1.0 - (std::f32::consts::PI * i as f32 / ramp as f32).cos());
        } else if i + ramp >= n {
            let j = n - 1 - i;
            s *= 0.5 * (1.0 - (std::f32::consts::PI * j as f32 / ramp as f32).cos());
        }
        out.push(s * cfg.amplitude);
    }
    out
}

/// Generate audio for a symbol sequence, with gaps between symbols.
pub fn encode(symbols: &[u8], cfg: &DtmfConfig) -> Vec<f32> {
    let mut out = Vec::with_capacity(symbols.len() * (cfg.symbol_samples + cfg.gap_samples));
    for (i, &sym) in symbols.iter().enumerate() {
        out.extend_from_slice(&encode_symbol(sym, cfg));
        if i + 1 < symbols.len() {
            out.extend(std::iter::repeat(0.0f32).take(cfg.gap_samples));
        }
    }
    out
}

/// Goertzel power at `freq` over `samples`.
pub fn goertzel(samples: &[f32], freq: f32, sample_rate: f32) -> f32 {
    let n = samples.len() as f32;
    let k = (freq / sample_rate) * n;
    let w = 2.0 * std::f32::consts::PI * k / n;
    let coeff = 2.0 * w.cos();
    let (mut s0, mut s1, mut s2) = (0.0f32, 0.0f32, 0.0f32);
    for &x in samples {
        s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    s1 * s1 + s2 * s2 - coeff * s1 * s2
}

/// Best-guess symbol from a single window of samples, plus a confidence ratio
/// (strongest tone-pair energy / next-best row or column).
pub fn detect_symbol(window: &[f32], cfg: &DtmfConfig) -> Option<(u8, f32)> {
    let sr = cfg.sample_rate as f32;
    let low: Vec<f32> = LOW_FREQS.iter().map(|&f| goertzel(window, f, sr)).collect();
    let high: Vec<f32> = HIGH_FREQS.iter().map(|&f| goertzel(window, f, sr)).collect();

    let (li, &lmax) = low
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))?;
    let (hi, &hmax) = high
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))?;

    // Second-best in each group, for a confidence estimate.
    let l2 = low
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != li)
        .map(|(_, &v)| v)
        .fold(0.0f32, f32::max);
    let h2 = high
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != hi)
        .map(|(_, &v)| v)
        .fold(0.0f32, f32::max);

    let ratio = (lmax / (l2 + 1e-12)).min(hmax / (h2 + 1e-12));
    let sym = (li * 4 + hi) as u8;
    Some((sym, ratio))
}

/// Decode a DTMF symbol stream by scanning overlapping windows, then keeping
/// only the strongest detection in each neighborhood (non-max suppression).
/// This tolerates arbitrary alignment and noisy gaps, and keeps adjacent
/// identical symbols separate as long as the gap is a reasonable fraction of
/// a symbol.
pub fn decode(audio: &[f32], cfg: &DtmfConfig) -> Vec<u8> {
    let win = (cfg.symbol_samples * 6 / 10).max(1);
    if audio.len() < win {
        return Vec::new();
    }
    let hop = (cfg.sample_rate as usize / 200).max(1); // 5 ms
    let energy = |s: &[f32]| s.iter().map(|v| v * v).sum::<f32>();
    // Energy floor: a tone burst must be well above the quietest window, or
    // numerical noise in silence yields bogus high tone ratios.
    let max_energy = audio
        .windows(win)
        .step_by(hop)
        .map(energy)
        .fold(0.0f32, f32::max)
        .max(1e-12);
    let floor = max_energy * 0.05;

    let mut dets: Vec<(f32, u8, usize)> = Vec::new();
    let mut i = 0;
    while i + win <= audio.len() {
        if energy(&audio[i..i + win]) >= floor {
            if let Some((sym, ratio)) = detect_symbol(&audio[i..i + win], cfg) {
                if ratio > 3.0 {
                    dets.push((ratio, sym, i));
                }
            }
        }
        i += hop;
    }

    // Greedy non-max suppression over ratio, then merge detections that are
    // closer than one symbol period. Symbols are separated by a full gap, so
    // picking the strongest detection per ~1.3-symbol cluster is safe and
    // collapses edge detections into one symbol.
    dets.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let min_sep = (cfg.symbol_samples * 13 / 10).max(1);
    let mut chosen: Vec<(usize, u8)> = Vec::new();
    for (_, sym, pos) in dets {
        if chosen.iter().all(|(p, _)| p.abs_diff(pos) >= min_sep) {
            chosen.push((pos, sym));
        }
    }
    chosen.sort_by_key(|(p, _)| *p);
    chosen.into_iter().map(|(_, s)| s).collect()
}

/// Pack nibbles (each < 16) into bytes, two per byte (high nibble first).
pub fn nibbles_to_bytes(nibbles: &[u8]) -> Vec<u8> {
    nibbles
        .chunks(2)
        .map(|c| {
            let hi = c[0] & 0x0F;
            let lo = c.get(1).copied().unwrap_or(0) & 0x0F;
            (hi << 4) | lo
        })
        .collect()
}

/// Unpack bytes into nibbles (high nibble first).
pub fn bytes_to_nibbles(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push((b >> 4) & 0x0F);
        out.push(b & 0x0F);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_roundtrip_clean() {
        let cfg = DtmfConfig::default();
        let symbols: Vec<u8> = vec![0x0, 0x5, 0xA, 0xF, 0x3, 0xC];
        let audio = encode(&symbols, &cfg);
        let decoded = decode(&audio, &cfg);
        assert_eq!(decoded, symbols, "clean DTMF roundtrip failed");
    }

    #[test]
    fn test_symbol_roundtrip_with_noise() {
        let cfg = DtmfConfig::default();
        let symbols: Vec<u8> = vec![0x1, 0x2, 0x3, 0x4, 0x5, 0x6, 0x7, 0x8];
        let mut audio = encode(&symbols, &cfg);
        let sig = audio.iter().map(|s| s * s).sum::<f32>() / audio.len() as f32;
        let noise = (sig * 0.05).sqrt(); // ~13 dB SNR
        let mut rng = 0u32;
        for s in audio.iter_mut() {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = ((rng >> 8) as f32 / 16_777_216.0) * 2.0 - 1.0;
            *s += n * noise;
        }
        let decoded = decode(&audio, &cfg);
        assert_eq!(decoded, symbols, "noisy DTMF roundtrip failed");
    }

    #[test]
    fn test_nibble_bytes_roundtrip() {
        let bytes = vec![0x12, 0xAB, 0x00, 0xFF];
        assert_eq!(nibbles_to_bytes(&bytes_to_nibbles(&bytes)), bytes);
    }

    #[test]
    fn test_all_symbols_distinct() {
        let cfg = DtmfConfig::default();
        let all: Vec<u8> = (0..16).collect();
        let audio = encode(&all, &cfg);
        assert_eq!(decode(&audio, &cfg), all);
    }
}
