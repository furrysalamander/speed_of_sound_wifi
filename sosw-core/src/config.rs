#[derive(Clone, serde::Serialize, serde::Deserialize)]
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
    /// Time-differential OFDM: data rides in the phase difference between
    /// consecutive OFDM symbols on the same subcarrier, so a static channel
    /// response and any common phase offset cancel and no absolute channel
    /// estimate is needed.
    #[serde(default)]
    pub differential: bool,
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
            differential: false,
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

    /// Preset: higher symbol rate (333 sym/s) via FFT=128, CP=16
    pub fn high_baud() -> Self {
        Self {
            sample_rate: 48000,
            fft_size: 128,
            cp_length: 16,
            sc_min: 4,
            sc_max: 31,
            preamble_symbols: 8,
            data_symbols_per_frame: 39,
            payload_size: 128,
            output_amplitude: 0.08,
            rs_nsym: 48,
            preamble_threshold: 0.05,
            cfo_clamp: 0.05,
            pll_beta: 0.08,
            pll_leak: 0.999,
            dd_alpha: 0.3,
            slope_alpha: 0.3,
            slope_clip: 0.02,
            scrambler_seed: 12345,
            preamble_seed: 42,
            differential: false,
        }
    }

    /// Preset: robust narrowband (83 sym/s) via FFT=512, CP=64, wider SC range
    pub fn robust() -> Self {
        Self {
            sample_rate: 48000,
            fft_size: 512,
            cp_length: 64,
            sc_min: 20,
            sc_max: 120,
            preamble_symbols: 8,
            data_symbols_per_frame: 21,
            payload_size: 256,
            output_amplitude: 0.10,
            rs_nsym: 32,
            preamble_threshold: 0.03,
            cfo_clamp: 0.05,
            pll_beta: 0.08,
            pll_leak: 0.999,
            dd_alpha: 0.3,
            slope_alpha: 0.3,
            slope_clip: 0.02,
            scrambler_seed: 12345,
            preamble_seed: 42,
            differential: false,
        }
    }

    /// Preset: ultrasonic range (15.0–22.5 kHz) via SC=80–120 on FFT=256
    pub fn ultrasonic() -> Self {
        Self {
            sample_rate: 48000,
            fft_size: 256,
            cp_length: 32,
            sc_min: 80,
            sc_max: 120,
            preamble_symbols: 8,
            data_symbols_per_frame: 51,
            payload_size: 256,
            output_amplitude: 0.12,
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
            differential: false,
        }
    }

    /// Preset: ultra-wide bandwidth (0.9–20.6 kHz, 176 sym/s, 37 kbps)
    pub fn ultrawide() -> Self {
        Self {
            sample_rate: 48000,
            fft_size: 256,
            cp_length: 16,
            sc_min: 5,
            sc_max: 110,
            preamble_symbols: 8,
            data_symbols_per_frame: 20,
            payload_size: 442,
            output_amplitude: 0.06,
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
            differential: false,
        }
    }

    pub fn theoretical_bps(&self) -> f32 {
        self.bits_per_ofdm_symbol() as f32 * self.symbol_rate()
    }

    pub fn frequency_min(&self) -> f32 {
        self.sc_min as f32 * self.sample_rate as f32 / self.fft_size as f32
    }

    pub fn frequency_max(&self) -> f32 {
        self.sc_max as f32 * self.sample_rate as f32 / self.fft_size as f32
    }

    pub fn occupied_bandwidth(&self) -> f32 {
        self.frequency_max() - self.frequency_min()
    }

    pub fn from_preset_name(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "high_baud" | "highbaud" => Self::high_baud(),
            "robust" => Self::robust(),
            "ultrasonic" => Self::ultrasonic(),
            "ultrawide" => Self::ultrawide(),
            _ => Self::ofdm_default(),
        }
    }

    /// Rebuild the OFDM layout and auto-fit `data_symbols_per_frame` to the
    /// full FrameAssembler output (sync + RS-FEC + CRC) for this payload size.
    pub fn with_layout(
        mut self,
        fft_size: usize,
        cp_length: usize,
        sc_min: usize,
        sc_max: usize,
        rs_nsym: usize,
        payload_size: usize,
    ) -> Self {
        self.fft_size = fft_size;
        self.cp_length = cp_length;
        self.sc_min = sc_min;
        self.sc_max = sc_max;
        self.rs_nsym = rs_nsym;
        self.payload_size = payload_size;
        let max_data = 255usize.saturating_sub(rs_nsym).max(1);
        let n_blocks = (4 + payload_size).div_ceil(max_data);
        let frame_bytes = 8 + n_blocks * 255 + 4;
        let bits_per_sym = self.bits_per_ofdm_symbol().max(1);
        // Time-differential spends the first data symbol as a known reference.
        let extra = if self.differential { 1 } else { 0 };
        self.data_symbols_per_frame = (frame_bytes * 8).div_ceil(bits_per_sym) + extra;
        self
    }
}
