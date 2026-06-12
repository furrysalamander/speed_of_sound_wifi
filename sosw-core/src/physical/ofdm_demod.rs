use crate::config::Config;
use crate::link::scrambler::Scrambler;
use crate::physical::preamble;
use crate::physical::qpsk;
use ndarray::Array1;
use num_complex::Complex32;
use rustfft::FftPlanner;
use std::sync::Arc;

pub struct DemodResult {
    pub bytes: Vec<u8>,
    pub preamble_peak: f32,
    pub cfo_rad_per_sym: f32,
    pub mean_h_magnitude: f32,
    pub per_sc_h: Vec<f32>,
    pub consumed_samples: usize,
}

pub struct OfdmDemodulator {
    config: Config,
    fft: Arc<dyn rustfft::Fft<f32>>,
    preamble_audio: Vec<f32>,
    preamble_energy: f32,
    preamble_symbols_fd: Vec<Vec<num_complex::Complex32>>,
    scrambler: Scrambler,
    channel_est: Array1<Complex32>,
    dd_common: f32,
    dd_slope: f32,
    cfo_freq: f32,
    subcarrier_indices: Vec<f32>,
    n_sc: usize,
    sc_min: usize,
    total_sym_samples: usize,
    preamble_samples_len: usize,
}

impl OfdmDemodulator {
    pub fn new(config: &Config) -> Self {
        let fft_size = config.fft_size;
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(fft_size);

        let preamble_audio = preamble::generate_preamble_audio(config);
        let preamble_energy: f32 = preamble_audio.iter().map(|&s| s * s).sum::<f32>().max(1e-12);

        let preamble_symbols_fd = preamble::generate_preamble_symbols(config);
        let scrambler = Scrambler::new(config.scrambler_seed);

        let n_sc = config.active_subcarriers();
        let sc_min = config.sc_min;
        let sub_ref = (sc_min + sc_min + n_sc - 1) as f32 * 0.5;
        let subcarrier_indices: Vec<f32> =
            (sc_min..sc_min + n_sc).map(|i| i as f32 - sub_ref).collect();
        let total_sym_samples = config.symbol_duration_samples();

        Self {
            config: config.clone(),
            fft,
            preamble_audio,
            preamble_energy,
            preamble_symbols_fd,
            scrambler,
            channel_est: Array1::zeros(fft_size),
            dd_common: 0.0,
            dd_slope: 0.0,
            cfo_freq: 0.0,
            subcarrier_indices,
            n_sc,
            sc_min,
            total_sym_samples,
            preamble_samples_len: config.preamble_samples(),
        }
    }

    fn energy_coarse_search(&self, samples: &[f32]) -> Option<usize> {
        let win_size = 64usize;
        if samples.len() < win_size {
            return None;
        }
        let search_limit = samples.len() - win_size;
        let step = win_size / 2;
        let mut i = 0;
        while i < search_limit {
            let energy: f32 = samples[i..i + win_size].iter().map(|&s| s * s).sum();
            if energy > 1e-4_f32 * win_size as f32 {
                return Some(i);
            }
            i += step;
        }
        None
    }

    fn extract_ofdm_symbol(&mut self, samples: &[f32], start: usize) -> Array1<Complex32> {
        let fft_size = self.config.fft_size;
        let cp_length = self.config.cp_length;
        let mut td = Array1::<Complex32>::zeros(fft_size);
        for i in 0..fft_size {
            td[i] = Complex32::new(samples[start + cp_length + i], 0.0);
        }
        let mut fd = td.clone();
        self.fft.process(fd.as_slice_mut().unwrap_or(&mut []));
        let scale = 1.0 / (fft_size as f32).sqrt();
        for v in fd.iter_mut() {
            *v *= scale;
        }
        fd
    }

    pub fn process_samples(&mut self, samples: &[f32]) -> Option<DemodResult> {
        let cfg = self.config.clone();
        let sc_min = self.sc_min;
        let n_sc = self.n_sc;
        let total_sym_samples = self.total_sym_samples;

        if samples.len() < self.preamble_samples_len {
            return None;
        }

        let coarse_start = self.energy_coarse_search(samples)?;
        let search_start = if coarse_start > self.preamble_samples_len {
            coarse_start - self.preamble_samples_len
        } else {
            0
        };
        let search_end = std::cmp::min(
            search_start + self.preamble_samples_len * 2,
            samples.len(),
        );

        if search_end <= search_start {
            return None;
        }

        let search_region = &samples[search_start..search_end];
        let corr = preamble::compute_cross_correlation(search_region, &self.preamble_audio);
        let (peak_offset, norm_peak) =
            preamble::find_preamble_peak(&corr, self.preamble_energy, search_region, self.preamble_samples_len)?;

        if norm_peak < cfg.preamble_threshold {
            return None;
        }

        let preamble_start = search_start + peak_offset;

        if samples.len() <= preamble_start + self.preamble_samples_len {
            return None;
        }

        let mut h_accum = Array1::<Complex32>::zeros(cfg.fft_size);
        for sym_idx in 0..cfg.preamble_symbols {
            let sym_start = preamble_start + sym_idx * total_sym_samples;
            if sym_start + total_sym_samples > samples.len() {
                break;
            }
            let fd = self.extract_ofdm_symbol(samples, sym_start);
            if sym_idx < self.preamble_symbols_fd.len() {
                let p_sym = &self.preamble_symbols_fd[sym_idx];
                for (i, sc_idx) in (sc_min..sc_min + n_sc).enumerate() {
                    if i < p_sym.len() {
                        let pv = p_sym[i];
                        if pv.norm_sqr() > 0.0 {
                            h_accum[sc_idx] = h_accum[sc_idx] + fd[sc_idx] * pv.conj() / pv.norm_sqr();
                        }
                    }
                }
            }
        }
        let n_preamble_used = cfg.preamble_symbols.min(
            (samples.len().saturating_sub(preamble_start)) / total_sym_samples,
        );
        if n_preamble_used > 0 {
            let inv_n = 1.0 / n_preamble_used as f32;
            for sc_idx in sc_min..sc_min + n_sc {
                h_accum[sc_idx] = h_accum[sc_idx] * inv_n;
            }
        }
        self.channel_est = h_accum.clone();

        let mut cfo_est = 0.0f32;
        if cfg.preamble_symbols >= 2 {
            let mut cfo_phases = Vec::new();
            for p_idx in 0..cfg.preamble_symbols - 1 {
                let sym0 = self.extract_ofdm_symbol(samples, preamble_start + p_idx * total_sym_samples);
                let sym1 = self.extract_ofdm_symbol(samples, preamble_start + (p_idx + 1) * total_sym_samples);
                if p_idx >= self.preamble_symbols_fd.len() || p_idx + 1 >= self.preamble_symbols_fd.len() {
                    continue;
                }
                let known0 = &self.preamble_symbols_fd[p_idx];
                let known1 = &self.preamble_symbols_fd[p_idx + 1];
                let mut sum_phase = 0.0f32;
                let mut count = 0u32;
                for (i, sc_idx) in (sc_min..sc_min + n_sc).enumerate() {
                    if i >= known0.len() || i >= known1.len() { continue; }
                    let h0 = sym0[sc_idx] / (known0[i] + num_complex::Complex32::new(1e-10, 0.0));
                    let h1 = sym1[sc_idx] / (known1[i] + num_complex::Complex32::new(1e-10, 0.0));
                    let cross = h1 * h0.conj();
                    if cross.norm_sqr() > 0.0 {
                        sum_phase += cross.arg();
                        count += 1;
                    }
                }
                if count > 0 {
                    cfo_phases.push(sum_phase / count as f32);
                }
            }
            if !cfo_phases.is_empty() {
                cfo_est = cfo_phases.iter().sum::<f32>() / cfo_phases.len() as f32;
                cfo_est = cfo_est.clamp(-cfg.cfo_clamp, cfg.cfo_clamp);
            }
        }
        self.cfo_freq = cfo_est;
        self.dd_common = cfo_est * (cfg.preamble_symbols as f32 - 1.5);
        self.dd_slope = 0.0;

        let n_data_syms = (samples.len() - preamble_start) / total_sym_samples;
        let n_data = n_data_syms.saturating_sub(cfg.preamble_symbols);
        if n_data == 0 {
            return None;
        }
        let n_data = std::cmp::min(n_data, cfg.data_symbols_per_frame);

        let total_bits = n_data * n_sc * 2;
        let mut all_bits = Vec::with_capacity(total_bits);

        let per_sc_h: Vec<f32> = (sc_min..sc_min + n_sc)
            .map(|i| self.channel_est[i].norm())
            .collect();

        for data_idx in 0..n_data {
            let sym_idx = cfg.preamble_symbols + data_idx;
            let sym_start = preamble_start + sym_idx * total_sym_samples;
            if sym_start + total_sym_samples > samples.len() {
                break;
            }

            let mut fd = self.extract_ofdm_symbol(samples, sym_start);
            let fd_slice = fd.as_slice_mut().unwrap_or(&mut []);

            let common = self.dd_common;
            let slope = self.dd_slope;
            for (i, sc_idx) in (sc_min..sc_min + n_sc).enumerate() {
                let k_offset = self.subcarrier_indices[i];
                let phase = common + slope * k_offset;
                let correction = Complex32::from_polar(1.0, -phase);
                fd_slice[sc_idx] *= correction;
            }

            let mut equalized = Vec::with_capacity(n_sc);
            for (_, sc_idx) in (sc_min..sc_min + n_sc).enumerate() {
                let h = self.channel_est[sc_idx];
                if h.norm_sqr() > 1e-20 {
                    equalized.push(fd_slice[sc_idx] / h);
                } else {
                    equalized.push(Complex32::new(0.0, 0.0));
                }
            }

            let bits = qpsk::qpsk_demap(&equalized);
            all_bits.extend_from_slice(&bits);

            let re_encoded = qpsk::qpsk_hard_decision(&equalized);
            let mut phase_errors = Vec::with_capacity(n_sc);
            for sc_idx in sc_min..sc_min + n_sc {
                let expected = self.channel_est[sc_idx] * re_encoded[sc_idx - sc_min];
                let received = fd_slice[sc_idx];
                let cross = received * expected.conj();
                if cross.norm_sqr() > 0.0 {
                    phase_errors.push((cross.arg(), (sc_idx - sc_min) as f32, equalized[sc_idx - sc_min].norm()));
                } else {
                    phase_errors.push((0.0, (sc_idx - sc_min) as f32, 0.0));
                }
            }

            if phase_errors.len() > 1 {
                let sum_w: f32 = phase_errors.iter().map(|(_, _, w)| w).sum::<f32>().max(1e-12);
                let w_mean_k: f32 = phase_errors
                    .iter()
                    .map(|(_, k, w)| k * w)
                    .sum::<f32>()
                    / sum_w;
                let w_mean_y: f32 = phase_errors
                    .iter()
                    .map(|(y, _, w)| y * w)
                    .sum::<f32>()
                    / sum_w;

                let mut num = 0.0f32;
                let mut den = 0.0f32;
                for (y, k, w) in &phase_errors {
                    let dk = k - w_mean_k;
                    num += dk * (y - w_mean_y) * w;
                    den += dk * dk * w;
                }
                let slope_est = if den.abs() > 1e-12 { num / den } else { 0.0 };
                let common_est = w_mean_y - slope_est * w_mean_k;

                self.cfo_freq = cfg.pll_leak * self.cfo_freq + cfg.pll_beta * common_est;
                self.cfo_freq = self.cfo_freq.clamp(-cfg.cfo_clamp, cfg.cfo_clamp);

                self.dd_common += self.cfo_freq + cfg.dd_alpha * common_est;
                self.dd_common = self.dd_common.sin().atan2(self.dd_common.cos());
                self.dd_slope = (1.0 - cfg.slope_alpha) * self.dd_slope
                    + cfg.slope_alpha * slope_est;
                self.dd_slope = self.dd_slope.clamp(-cfg.slope_clip, cfg.slope_clip);
            } else if phase_errors.len() == 1 {
                self.dd_common += self.cfo_freq + cfg.dd_alpha * phase_errors[0].0;
            }
        }

        let byte_count = all_bits.len() / 8;
        let mut bytes = Vec::with_capacity(byte_count);
        for byte_idx in 0..byte_count {
            let mut byte = 0u8;
            for bit_idx in 0..8 {
                let bit = all_bits.get(byte_idx * 8 + bit_idx).copied().unwrap_or(0);
                byte = (byte << 1) | bit;
            }
            bytes.push(byte);
        }

        let descrambled = self.scrambler.descramble(&bytes);

        let mean_h_magnitude = if !per_sc_h.is_empty() {
            per_sc_h.iter().sum::<f32>() / per_sc_h.len() as f32
        } else {
            0.0
        };

        let consumed_samples = preamble_start + (cfg.preamble_symbols + n_data) * total_sym_samples;
        Some(DemodResult {
            bytes: descrambled,
            preamble_peak: norm_peak,
            cfo_rad_per_sym: cfo_est,
            mean_h_magnitude,
            per_sc_h,
            consumed_samples,
        })
    }

    pub fn reset(&mut self) {
        let fft_size = self.config.fft_size;
        self.channel_est = Array1::zeros(fft_size);
        self.dd_common = 0.0;
        self.dd_slope = 0.0;
        self.cfo_freq = 0.0;
    }
}
