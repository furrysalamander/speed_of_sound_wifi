//! Diagnostic: how tolerant is the OFDM demod to a sample-rate offset between
//! transmitter and receiver? Software/monitor loopbacks share one clock; the
//! acoustic path has two (speaker DAC vs mic ADC), so a clock mismatch shows
//! up as symbol-timing drift that a single-tap equalizer cannot undo.

use sosw_core::config::Config;
use sosw_core::link::frame::{FrameAssembler, FrameParser};
use sosw_core::physical::ofdm_demod::OfdmDemodulator;
use sosw_core::physical::ofdm_mod::OfdmModulator;

fn build_audio(config: &Config, n_frames: usize) -> Vec<f32> {
    let mut assembler = FrameAssembler::new(config);
    let mut modulator = OfdmModulator::new(config);
    let guard = config.symbol_duration_samples();
    let payload_size = config.payload_size;
    let mut audio = Vec::new();
    audio.extend(std::iter::repeat(0.0f32).take((config.sample_rate as f64 * 0.5) as usize));
    for i in 0..3 {
        let train = vec![(i % 256) as u8; payload_size];
        audio.extend_from_slice(&modulator.modulate_with_preamble(&assembler.assemble_frame(&train)));
        audio.extend(std::iter::repeat(0.0f32).take(guard));
    }
    for i in 0..n_frames {
        let p: Vec<u8> = (0..payload_size).map(|j| ((i * payload_size + j) % 256) as u8).collect();
        audio.extend_from_slice(&modulator.modulate_with_preamble(&assembler.assemble_frame(&p)));
        audio.extend(std::iter::repeat(0.0f32).take(guard));
    }
    audio
}

fn resample(x: &[f32], factor: f64) -> Vec<f32> {
    let out_len = ((x.len() as f64) / factor) as usize;
    let mut y = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * factor;
        let idx = pos as usize;
        let frac = (pos - idx as f64) as f32;
        let a = x.get(idx).copied().unwrap_or(0.0);
        let b = x.get(idx + 1).copied().unwrap_or(0.0);
        y.push(a + (b - a) * frac);
    }
    y
}

fn main() {
    let config = Config::from_preset_name("default");
    let guard = config.symbol_duration_samples();
    let audio = build_audio(&config, 10);
    println!("clean software demod as a function of RX clock offset (ppm):");
    for ppm in [0i32, 50, 100, 200, 300, 500, 800, 1200, 2000, 4000] {
        let factor = 1.0 + ppm as f64 / 1e6;
        let rx = resample(&audio, factor);
        let mut demod = OfdmDemodulator::new(&config);
        let lead_in = (config.sample_rate as f64 * 0.5) as usize;
        let chunk_size = config.frame_samples() + guard * 6;
        let mut search_pos = lead_in;
        let mut valid = 0usize;
        let mut det = 0usize;
        while search_pos + config.preamble_samples() < rx.len() {
            let chunk_end = std::cmp::min(search_pos + chunk_size, rx.len());
            let mut padded: Vec<f32> = rx[search_pos..chunk_end].to_vec();
            padded.extend(std::iter::repeat(0.0f32).take(guard * 4));
            if let Some(r) = demod.process_samples(&padded) {
                det += 1;
                let mut parser = FrameParser::new(&config);
                valid += parser.feed_bytes(&r.bytes).iter().filter(|f| f.valid).count();
                search_pos += r.consumed_samples;
            } else {
                search_pos += guard;
            }
            demod.reset();
        }
        println!("  {:+5} ppm: {}/{} valid frames", ppm, valid, det);
    }
}
