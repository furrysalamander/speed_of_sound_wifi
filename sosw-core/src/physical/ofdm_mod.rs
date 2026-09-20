use crate::config::Config;
use crate::link::scrambler::Scrambler;
use crate::physical::preamble::generate_preamble_audio;
use crate::physical::qpsk;
use ndarray::Array1;
use num_complex::Complex32;
use rustfft::FftPlanner;
use std::sync::Arc;

pub struct OfdmModulator {
    config: Config,
    inner: OfdmModulatorInner,
    fft: Arc<dyn rustfft::Fft<f32>>,
    ifft: Arc<dyn rustfft::Fft<f32>>,
    scrambler: Scrambler,
    preamble_audio: Vec<f32>,
}

pub struct OfdmModulatorInner {
    pub config: Config,
}

impl OfdmModulatorInner {
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.clone(),
        }
    }

    pub fn build_ofdm_symbol(&self, fd_data: &[Complex32]) -> Vec<f32> {
        let fft_size = self.config.fft_size;
        let sc_min = self.config.sc_min;
        let cp_length = self.config.cp_length;

        let mut fd = Array1::<Complex32>::zeros(fft_size);
        let n_sc = fd_data.len();
        for (i, &val) in fd_data.iter().enumerate() {
            fd[sc_min + i] = val;
        }
        for (i, &val) in fd_data.iter().enumerate() {
            let idx = fft_size - sc_min - i;
            if idx < fft_size {
                fd[idx] = val.conj();
            }
        }

        let scale = 1.0 / (fft_size as f32).sqrt();
        let mut td = Array1::<Complex32>::zeros(fft_size);
        let mut planner = FftPlanner::new();
        let ifft = planner.plan_fft_inverse(fft_size);
        ifft.process(fd.as_slice_mut().unwrap_or(&mut []));
        for (i, &v) in fd.iter().enumerate() {
            td[i] = v * scale;
        }

        let mut symbol = Vec::with_capacity(fft_size + cp_length);
        for i in fft_size - cp_length..fft_size {
            symbol.push(td[i].re);
        }
        for i in 0..fft_size {
            symbol.push(td[i].re);
        }
        symbol
    }
}

impl OfdmModulator {
    pub fn new(config: &Config) -> Self {
        let fft_size = config.fft_size;
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(fft_size);
        let ifft = planner.plan_fft_inverse(fft_size);
        let preamble_audio = generate_preamble_audio(config);
        let scrambler = Scrambler::new(config.scrambler_seed);
        Self {
            config: config.clone(),
            inner: OfdmModulatorInner::new(config),
            fft,
            ifft,
            scrambler,
            preamble_audio,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn modulate_with_preamble(&mut self, data: &[u8]) -> Vec<f32> {
        let config = &self.config;
        let n_sc = config.active_subcarriers();
        let bits_per_sym = config.bits_per_ofdm_symbol();
        let total_frame_bits = if config.differential {
            // First data symbol is a known reference for time-differential.
            config.data_symbols_per_frame.saturating_sub(1) * bits_per_sym
        } else {
            config.data_symbols_per_frame * bits_per_sym
        };

        let data_bits = data.len() * 8;
        let pad_bits = total_frame_bits.saturating_sub(data_bits);
        let pad_full_bytes = pad_bits / 8;
        let pad_remain_bits = pad_bits % 8;

        let scrambled = self.scrambler.scramble(data);
        let pad_total = pad_full_bytes + if pad_remain_bits > 0 { 1 } else { 0 };
        let mask_all = self.scrambler.mask(data.len() + pad_total);
        let mut bit_vec = Vec::with_capacity(total_frame_bits);
        for &byte in &scrambled {
            for b in (0..8).rev() {
                bit_vec.push((byte >> b) & 1);
            }
        }
        if pad_total > 0 {
            let pad_mask = &mask_all[data.len()..data.len() + pad_total];
            for &byte in &pad_mask[..pad_full_bytes] {
                for b in (0..8).rev() {
                    bit_vec.push((byte >> b) & 1);
                }
            }
            for b in (0..pad_remain_bits).rev() {
                bit_vec.push((pad_mask[pad_full_bytes] >> b) & 1);
            }
        }

        let mut audio = Vec::with_capacity(
            config.preamble_samples() + config.data_symbols_per_frame * config.symbol_duration_samples(),
        );

        audio.extend_from_slice(&self.preamble_audio);

        if config.differential {
            // Time-differential: the first data symbol is a known all-ones
            // reference; each following symbol is the previous one rotated per
            // subcarrier by its QPSK data. Data lives in the phase difference
            // between consecutive symbols on the same subcarrier, so an
            // arbitrary static channel cancels.
            let ref_fd = vec![Complex32::new(1.0, 0.0); n_sc];
            audio.extend_from_slice(&self.inner.build_ofdm_symbol(&ref_fd));
            let mut acc = ref_fd;
            for chunk in bit_vec.chunks(bits_per_sym) {
                let d = qpsk::qpsk_map(chunk);
                for i in 0..n_sc {
                    acc[i] *= d[i];
                }
                let td = self.inner.build_ofdm_symbol(&acc);
                audio.extend_from_slice(&td);
            }
        } else {
            for chunk in bit_vec.chunks(bits_per_sym) {
                let fd = qpsk::qpsk_map(chunk);
                let td = self.inner.build_ofdm_symbol(&fd);
                audio.extend_from_slice(&td);
            }
        }

        let ramp_len = config.cp_length;
        if ramp_len > 0 && audio.len() > ramp_len {
            crate::physical::preamble::apply_raised_cosine_onset(&mut audio, ramp_len);
        }

        let max_val = audio
            .iter()
            .map(|&s| s.abs())
            .fold(0.0f32, f32::max)
            .max(1e-12);
        let gain = config.output_amplitude / max_val;
        for s in audio.iter_mut() {
            *s *= gain;
        }

        audio
    }

    pub fn preamble_samples(&self) -> usize {
        self.config.preamble_samples()
    }

    pub fn frame_audio_duration(&self) -> usize {
        self.config.frame_samples()
    }
}
