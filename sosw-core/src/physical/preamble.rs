use crate::config::Config;
use crate::physical::ofdm_mod::OfdmModulatorInner;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha12Rng;

pub fn generate_preamble_symbols(config: &Config) -> Vec<Vec<num_complex::Complex32>> {
    let n_sc = config.active_subcarriers();
    let n_preamble = config.preamble_symbols;
    let mut rng = ChaCha12Rng::seed_from_u64(config.preamble_seed);
    let mut symbols = Vec::with_capacity(n_preamble);
    for _ in 0..n_preamble {
        let sym: Vec<num_complex::Complex32> = (0..n_sc)
            .map(|_| {
                let b0: u8 = rng.random_range(0..2);
                let b1: u8 = rng.random_range(0..2);
                match (b0, b1) {
                    (0, 0) => num_complex::Complex32::new(1.0, 1.0),
                    (1, 0) => num_complex::Complex32::new(-1.0, 1.0),
                    (1, 1) => num_complex::Complex32::new(-1.0, -1.0),
                    (0, 1) => num_complex::Complex32::new(1.0, -1.0),
                    _ => unreachable!(),
                }
                .unscale(std::f32::consts::SQRT_2)
            })
            .collect();
        symbols.push(sym);
    }
    symbols
}

pub fn generate_preamble_audio(config: &Config) -> Vec<f32> {
    let symbols = generate_preamble_symbols(config);
    let inner = OfdmModulatorInner::new(config);
    let mut audio = Vec::with_capacity(config.preamble_samples());
    for fs in &symbols {
        let td_sym = inner.build_ofdm_symbol(fs);
        audio.extend_from_slice(&td_sym);
    }
    audio
}

/// Cross-correlate signal with preamble (direct, not reversed).
/// Equivalent to np.convolve(signal, preamble[::-1], mode="valid") in Python,
/// which internally reverses the kernel once, producing direct correlation:
/// corr[i] = sum_k signal[i+k] * preamble[k]
pub fn compute_cross_correlation(signal: &[f32], preamble: &[f32]) -> Vec<f32> {
    if signal.len() < preamble.len() {
        return Vec::new();
    }
    let n = signal.len() - preamble.len() + 1;
    let mut corr = Vec::with_capacity(n);
    for i in 0..n {
        let sum: f32 = signal[i..i + preamble.len()]
            .iter()
            .zip(preamble.iter())
            .map(|(s, p)| s * p)
            .sum();
        corr.push(sum);
    }
    corr
}

pub fn find_preamble_peak(
    corr: &[f32],
    preamble_energy: f32,
    signal: &[f32],
    preamble_len: usize,
) -> Option<(usize, f32)> {
    if corr.is_empty() || preamble_energy <= 0.0 {
        return None;
    }
    let peak_idx = corr
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)?;
    let peak_val = corr[peak_idx];
    let window_start = peak_idx;
    let window_end = std::cmp::min(window_start + preamble_len, signal.len());
    let signal_window: f32 = signal[window_start..window_end]
        .iter()
        .map(|&s| s * s)
        .sum::<f32>()
        .max(1e-12);
    let norm_peak = peak_val / (preamble_energy * signal_window).sqrt();
    Some((peak_idx, norm_peak))
}

pub fn apply_raised_cosine_onset(audio: &mut [f32], ramp_len: usize) {
    if ramp_len == 0 || audio.len() < ramp_len {
        return;
    }
    for i in 0..ramp_len {
        let gain = (1.0 - (std::f32::consts::PI * (ramp_len - i) as f32 / ramp_len as f32).cos())
            * 0.5;
        audio[i] *= gain;
    }
}
