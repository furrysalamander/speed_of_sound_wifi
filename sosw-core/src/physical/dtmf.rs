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
            // 100 ms tone / 100 ms gap = 4 bits per 200 ms (~20 bps). Both
            // desktop and laptop mics decode this reliably; longer symbols
            // were tried but only made every message take seconds longer.
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

/// Best-guess symbol from a single window of samples, plus a confidence in
/// `0..1`: the smaller of how strongly the winning low and high tones
/// dominate their frequency groups. A clean DTMF pair scores ~0.9+; broadband
/// noise (energy spread across all four rows/columns) scores ~0.3.
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

    let lsum: f32 = low.iter().sum::<f32>() + 1e-12;
    let hsum: f32 = high.iter().sum::<f32>() + 1e-12;
    let conf = (lmax / lsum).min(hmax / hsum);
    Some(((li * 4 + hi) as u8, conf))
}

/// Decode a DTMF symbol stream.
///
/// The transmitter uses a fixed symbol period, so the decoder locks to a
/// symbol grid within each active region and samples one symbol per period.
/// This is what makes it robust: over the air, a naive burst detector inserts
/// and deletes symbols (tones split, gaps fill with noise), which breaks any
/// frame. Sampling on the grid yields exactly one symbol per transmitted slot.
pub fn decode(audio: &[f32], cfg: &DtmfConfig) -> Vec<u8> {
    let env_win = (cfg.sample_rate as usize / 100).max(1); // 10 ms
    if audio.len() < env_win * 2 {
        return Vec::new();
    }
    let env: Vec<f32> = audio
        .windows(env_win)
        .step_by(env_win)
        .map(|w| w.iter().map(|v| v * v).sum::<f32>())
        .collect();

    // Noise-floor-relative threshold from a low percentile, so it tracks the
    // room level but ignores isolated loud transients (clicks, bumps, system
    // sounds). An absolute-epsilon fallback only guards all-silent buffers;
    // using a fraction of the *peak* here would let one spike blind the
    // detector to every real symbol.
    let mut sorted = env.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p20 = sorted[sorted.len() / 5].max(1e-12);
    let thr = p20 * 2.0;

    // Hysteresis: start a region only on a clearly-above-floor window, but keep
    // it going down to the floor. Without this, amplitude ripple and reverb
    // fragment one message into many short regions, each of which re-anchors
    // its own grid and misaligns.
    let thr_start = (p20 * 8.0).max(thr);

    let period = cfg.symbol_samples + cfg.gap_samples;
    let half = cfg.symbol_samples / 2;

    // Split the recording into active regions, allowing short dips within a
    // region so a brief amplitude ripple does not fragment a message.
    let mut out = Vec::new();
    let gap_tol = (cfg.gap_samples / 2).max(1);
    let mut i = 0;
    while i < env.len() {
        while i < env.len() && env[i] <= thr_start {
            i += 1;
        }
        if i >= env.len() {
            break;
        }
        // Back up to where the level first rose above the noise floor, so the
        // region includes the leading edge of the first tone.
        while i > 0 && env[i - 1] > thr {
            i -= 1;
        }
        let region_start = i * env_win;
        // Extend the region while there is energy, tolerating short dips.
        let mut last_active = i;
        let mut j = i;
        while j < env.len() {
            if env[j] > thr {
                last_active = j;
            } else if (j - last_active) * env_win > gap_tol {
                break;
            }
            j += 1;
        }
        let region_end = ((last_active + 1) * env_win).min(audio.len());
        i = last_active + 1;

        // Fine grid-phase search. The coarse region start is only known to
        // within one envelope window (10 ms), which is negligible for a 100 ms
        // symbol but not for a 30-40 ms one. Try sub-window offsets and keep
        // the phase whose grid slots lock onto the tones most strongly.
        let phase_step = (cfg.sample_rate as usize / 200).max(1); // 5 ms
        let base = region_start.saturating_sub(env_win);
        let search_end = region_start + env_win;
        let mut best_score = f32::MIN;
        let mut best_slots: Vec<(u8, f32)> = Vec::new();
        let mut start = base;
        loop {
            let mut slots: Vec<(u8, f32)> = Vec::new();
            let mut k = 0;
            loop {
                let center = start + half + k * period;
                if center + half > region_end {
                    break;
                }
                let a = center.saturating_sub(half);
                let b = (center + half).min(audio.len());
                if b.saturating_sub(a) < cfg.symbol_samples / 2 {
                    break;
                }
                if let Some((sym, conf)) = detect_symbol(&audio[a..b], cfg) {
                    slots.push((sym, conf));
                }
                k += 1;
            }
            // Reward both how many slots lock on and how strong the best one is.
            let score: f32 = slots.iter().map(|(_, c)| c).sum::<f32>()
                + slots.iter().map(|(_, c)| *c).fold(0.0f32, f32::max);
            if score > best_score {
                best_score = score;
                best_slots = slots;
            }
            if start >= search_end {
                break;
            }
            start = (start + phase_step).min(search_end);
        }

        // Only accept the region if at least one slot looks like a genuine
        // DTMF pair, so pure-noise regions produce nothing.
        let best = best_slots.iter().map(|(_, c)| *c).fold(0.0f32, f32::max);
        if best >= 0.85 {
            out.extend(best_slots.into_iter().map(|(s, _)| s));
        }
    }
    out
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

    fn cfg_ms(symbol_ms: u32, gap_ms: u32) -> DtmfConfig {
        DtmfConfig {
            sample_rate: 48000,
            symbol_samples: 48 * symbol_ms as usize,
            gap_samples: 48 * gap_ms as usize,
            amplitude: 0.4,
        }
    }

    fn add_noise(audio: &mut [f32], rel: f32) {
        let sig = audio.iter().map(|s| s * s).sum::<f32>() / audio.len().max(1) as f32;
        let noise = (sig * rel).sqrt();
        let mut rng = 0xC0FFEEu32;
        for s in audio.iter_mut() {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = ((rng >> 8) as f32 / 16_777_216.0) * 2.0 - 1.0;
            *s += n * noise;
        }
    }

    #[test]
    fn test_symbol_roundtrip_clean() {
        let cfg = DtmfConfig::default();
        let symbols: Vec<u8> = vec![0x0, 0x5, 0xA, 0xF, 0x3, 0xC];
        let audio = encode(&symbols, &cfg);
        let decoded = decode(&audio, &cfg);
        assert_eq!(decoded, symbols, "clean DTMF roundtrip failed");
    }

    /// The control channel does not need 100 ms symbols. Verify the grid phase
    /// search makes shorter symbols work cleanly, including an exact-of-grid
    /// capture offset (the acoustic latency) and noise.
    #[test]
    fn test_short_symbols_roundtrip() {
        let symbols: Vec<u8> = (0..16).collect();
        for (sym_ms, gap_ms) in [(60u32, 40u32), (40, 20), (30, 20), (20, 10)] {
            let cfg = cfg_ms(sym_ms, gap_ms);
            let audio = encode(&symbols, &cfg);
            assert_eq!(
                decode(&audio, &cfg),
                symbols,
                "clean {}ms/{}ms roundtrip failed",
                sym_ms,
                gap_ms
            );
        }
    }

    #[test]
    fn test_short_symbols_offset_and_noise() {
        let symbols: Vec<u8> = (0..16).collect();
        for &(sym_ms, gap_ms) in &[(40u32, 20u32), (30, 20)] {
            let cfg = cfg_ms(sym_ms, gap_ms);
            let mut audio = vec![0.0f32; 777];
            audio.extend(encode(&symbols, &cfg));
            audio.extend(std::iter::repeat(0.0).take(3000));
            add_noise(&mut audio, 0.02); // ~17 dB SNR
            assert_eq!(
                decode(&audio, &cfg),
                symbols,
                "{}ms/{}ms with offset+noise failed",
                sym_ms,
                gap_ms
            );
        }
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

    #[test]
    fn test_noise_alone_decodes_nothing() {
        let cfg = DtmfConfig::default();
        // Low-frequency-ish random walk + white noise, similar to room rumble.
        let mut rng = 12345u32;
        let mut v = 0.0f32;
        let mut audio = Vec::with_capacity(48000 * 2);
        for _ in 0..48000 * 2 {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let w = ((rng >> 8) as f32 / 16_777_216.0) * 2.0 - 1.0;
            v = v * 0.999 + w * 0.001;
            audio.push(v + w * 0.001);
        }
        let syms = decode(&audio, &cfg);
        assert!(syms.is_empty(), "noise produced symbols: {:X?}", syms);
    }
}
