use sosw_core::config::Config;
use sosw_core::physical::ofdm_demod::OfdmDemodulator;
use sosw_core::physical::ofdm_mod::OfdmModulator;

fn roundtrip_ok(config: &Config, payload: &[u8]) -> bool {
    let mut modulator = OfdmModulator::new(config);
    let samples = modulator.modulate_with_preamble(payload);
    let mut demodulator = OfdmDemodulator::new(config);
    let result = demodulator.process_samples(&samples);
    match result {
        Some(dr) => {
            let n = payload.len().min(config.payload_size);
            dr.bytes.len() >= n && dr.bytes[..n] == payload[..n]
        }
        None => false,
    }
}

fn test_payload(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 256) as u8).collect()
}

#[test]
fn sweep_default_config() {
    let config = Config::ofdm_default();
    let payload = test_payload(config.payload_size);
    assert!(roundtrip_ok(&config, &payload));
}

#[test]
fn sweep_fft_sizes() {
    let payload = test_payload(64);
    for fft in [128usize, 256, 512] {
        for cp in [16usize, 32, 64] {
            if cp >= fft { continue; }
            let mut c = Config::ofdm_default();
            c.fft_size = fft;
            c.cp_length = cp;
            c.sc_min = fft / 16;
            c.sc_max = fft / 4 + fft / 8;
            c.payload_size = payload.len();
            let ok = roundtrip_ok(&c, &payload);
            let fs = c.sample_rate as f64;
            let bin_w = fs / c.fft_size as f64;
            let f_min = c.sc_min as f64 * bin_w;
            let f_max = c.sc_max as f64 * bin_w;
            let sr = fs / (c.fft_size + c.cp_length) as f64;
            let nsc = c.active_subcarriers();
            let status = if ok { "PASS" } else { "FAIL" };
            println!("FFT={:3} CP={:3} SC={:3}-{:<3}  {:5.0}-{:5.0}Hz  sym_rate={:.1}  SCs={}  {}",
                     fft, cp, c.sc_min, c.sc_max, f_min, f_max, sr, nsc, status);
            assert!(ok, "FFT={} CP={} SC={}-{} failed", fft, cp, c.sc_min, c.sc_max);
        }
    }
}

#[test]
fn sweep_subcarrier_ranges() {
    let cp_length = 32usize;
    for fft_size in [256usize, 512] {
        for sc_max in [31usize, 63, 95, 127] {
            let sc_min = std::cmp::max(1, sc_max.saturating_sub(40));
            let mut c = Config::ofdm_default();
            c.fft_size = fft_size;
            c.cp_length = cp_length;
            c.sc_min = sc_min;
            c.sc_max = sc_max;
            c.payload_size = 64;
            let payload = test_payload(c.payload_size);
            let ok = roundtrip_ok(&c, &payload);
            let fs = c.sample_rate as f64;
            let bin_w = fs / c.fft_size as f64;
            let f_min = c.sc_min as f64 * bin_w;
            let f_max = c.sc_max as f64 * bin_w;
            let sr = fs / (c.fft_size + c.cp_length) as f64;
            let nsc = c.active_subcarriers();
            let status = if ok { "PASS" } else { "FAIL" };
            println!("FFT={fft_size} SC={sc_min:3}-{sc_max:<3}  {:5.0}-{:5.0}Hz  sym_rate={:.1}  SCs={}  {}",
                     f_min, f_max, sr, nsc, status);
            if f_max > 20000.0 {
                let label = if f_min > 20000.0 { "ULTRASONIC" } else { "PARTIAL-ULTRA" };
                println!("  ^^ {label}");
            }
        }
    }
}

#[test]
fn sweep_baud_rate_presets() {
    let tests: Vec<(&str, Config)> = {
        let mut d = Config::ofdm_default(); d.payload_size = 64;
        let mut hb = Config::ofdm_default(); hb.fft_size = 128; hb.cp_length = 16; hb.sc_min = 4; hb.sc_max = 31; hb.payload_size = 64;
        let mut rb = Config::ofdm_default(); rb.fft_size = 512; rb.cp_length = 64; rb.sc_min = 20; rb.sc_max = 120; rb.payload_size = 64;
        let mut us = Config::ofdm_default(); us.fft_size = 256; us.cp_length = 32; us.sc_min = 80; us.sc_max = 120; us.payload_size = 64;
        let mut uw = Config::ofdm_default(); uw.fft_size = 256; uw.cp_length = 16; uw.sc_min = 5; uw.sc_max = 110; uw.payload_size = 64;
        vec![("default", d), ("high_baud", hb), ("robust", rb), ("ultrasonic", us), ("ultrawide", uw)]
    };

    for (name, c) in &tests {
        let payload = test_payload(c.payload_size);
        let ok = roundtrip_ok(c, &payload);
        let fs = c.sample_rate as f64;
        let bin_w = fs / c.fft_size as f64;
        let f_min = c.sc_min as f64 * bin_w;
        let f_max = c.sc_max as f64 * bin_w;
        let sr = fs / (c.fft_size + c.cp_length) as f64;
        let bps = c.active_subcarriers() as f64 * 2.0 * sr;
            let status = if ok { "PASS" } else { "FAIL" };
            println!("{name:<12} FFT={:3} CP={:3} SC={:3}-{:<3}  {:5.0}-{:5.0}Hz  sym={:.0}  bps={:.0}  {}",
                     c.fft_size, c.cp_length, c.sc_min, c.sc_max,
                     f_min, f_max, sr, bps, status);
        if !ok {
            println!("  ^^ FAILED");
        }
    }
}
