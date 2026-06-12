use sosw_core::Config;

pub struct Preset {
    pub name: &'static str,
    pub description: &'static str,
    pub maker: fn() -> Config,
}

pub const PRESETS: &[Preset] = &[
    Preset { name: "Default", description: "FFT=256 CP=32 SC=10-69  (1.9–12.9 kHz, 167 sym/s)", maker: Config::ofdm_default },
    Preset { name: "High Baud", description: "FFT=128 CP=16 SC=4-31  (1.5–11.6 kHz, 333 sym/s)", maker: Config::high_baud },
    Preset { name: "Robust", description: "FFT=512 CP=64 SC=20-120  (1.9–11.3 kHz, 83 sym/s)", maker: Config::robust },
    Preset { name: "Ultrasonic", description: "FFT=256 CP=32 SC=80-120  (15.0–22.5 kHz, 167 sym/s)", maker: Config::ultrasonic },
    Preset { name: "Ultrawide", description: "FFT=256 CP=16 SC=5-110  (0.9–20.6 kHz, 176 sym/s, 37 kbps)", maker: Config::ultrawide },
];

const STORAGE_KEY: &str = "sosw_config";

pub fn load_saved_config() -> Config {
    let window = match web_sys::window() {
        Some(w) => w,
        None => return Config::ofdm_default(),
    };
    let storage = match window.local_storage() {
        Ok(Some(s)) => s,
        _ => return Config::ofdm_default(),
    };
    let json = match storage.get_item(STORAGE_KEY) {
        Ok(Some(v)) => v,
        _ => return Config::ofdm_default(),
    };
    serde_json::from_str(&json).unwrap_or_else(|_| Config::ofdm_default())
}

pub fn save_config(config: &Config) {
    let window = match web_sys::window() {
        Some(w) => w,
        _ => return,
    };
    let storage = match window.local_storage() {
        Ok(Some(s)) => s,
        _ => return,
    };
    if let Ok(json) = serde_json::to_string(config) {
        let _ = storage.set_item(STORAGE_KEY, &json);
    }
}

pub fn which_preset(config: &Config) -> &'static str {
    for p in PRESETS {
        let preset_cfg = (p.maker)();
        // Check if all fields match (use partial equality to avoid float issues)
        if configs_equal(config, &preset_cfg) {
            return p.name;
        }
    }
    "Custom"
}

fn configs_equal(a: &Config, b: &Config) -> bool {
    a.sample_rate == b.sample_rate
        && a.fft_size == b.fft_size
        && a.cp_length == b.cp_length
        && a.sc_min == b.sc_min
        && a.sc_max == b.sc_max
        && a.preamble_symbols == b.preamble_symbols
        && a.data_symbols_per_frame == b.data_symbols_per_frame
        && a.payload_size == b.payload_size
        && (a.output_amplitude - b.output_amplitude).abs() < 0.001
        && a.rs_nsym == b.rs_nsym
        && (a.preamble_threshold - b.preamble_threshold).abs() < 0.001
        && (a.cfo_clamp - b.cfo_clamp).abs() < 0.001
        && (a.pll_beta - b.pll_beta).abs() < 0.001
        && (a.pll_leak - b.pll_leak).abs() < 0.001
        && (a.dd_alpha - b.dd_alpha).abs() < 0.001
        && (a.slope_alpha - b.slope_alpha).abs() < 0.001
        && (a.slope_clip - b.slope_clip).abs() < 0.001
        && a.scrambler_seed == b.scrambler_seed
        && a.preamble_seed == b.preamble_seed
}
