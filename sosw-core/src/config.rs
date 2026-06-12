#[derive(Clone)]
pub struct Config {
    pub sample_rate: u32,
    pub fft_size: usize,
    pub cp_length: usize,
    pub sc_min: usize,
    pub sc_max: usize,
    pub preamble_symbols: usize,
    pub data_symbols_per_frame: usize,
    pub payload_size: usize,
    pub output_amplitude: f32,
    pub rs_nsym: usize,
    pub preamble_threshold: f32,
    pub cfo_clamp: f32,
    pub pll_beta: f32,
    pub pll_leak: f32,
    pub dd_alpha: f32,
    pub slope_alpha: f32,
    pub slope_clip: f32,
    pub scrambler_seed: u64,
    pub preamble_seed: u64,
}

impl Config {
    pub fn ofdm_default() -> Self {
        Self {
            sample_rate: 48000,
            fft_size: 256,
            cp_length: 32,
            sc_min: 10,
            sc_max: 69,
            preamble_symbols: 8,
            data_symbols_per_frame: 35,
            payload_size: 442,
            output_amplitude: 0.08,
            rs_nsym: 32,
            preamble_threshold: 0.05,
            cfo_clamp: 0.05,
            pll_beta: 0.08,
            pll_leak: 0.999,
            dd_alpha: 0.3,
            slope_alpha: 0.3,
            slope_clip: 0.02,
            scrambler_seed: 12345,
            preamble_seed: 42,
        }
    }

    pub fn active_subcarriers(&self) -> usize {
        if self.sc_max >= self.sc_min {
            self.sc_max - self.sc_min + 1
        } else {
            0
        }
    }

    pub fn bits_per_ofdm_symbol(&self) -> usize {
        self.active_subcarriers() * 2
    }

    pub fn symbol_duration_samples(&self) -> usize {
        self.fft_size + self.cp_length
    }

    pub fn preamble_samples(&self) -> usize {
        self.preamble_symbols * self.symbol_duration_samples()
    }

    pub fn frame_samples(&self) -> usize {
        let total_syms = self.preamble_symbols + self.data_symbols_per_frame;
        total_syms * self.symbol_duration_samples()
    }

    pub fn symbol_rate(&self) -> f32 {
        self.sample_rate as f32 / self.symbol_duration_samples() as f32
    }

    pub fn theoretical_bps(&self) -> f32 {
        self.bits_per_ofdm_symbol() as f32 * self.symbol_rate()
    }
}
