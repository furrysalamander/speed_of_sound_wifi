//! Diagnostic: OFDM demod performance over synthetic channels (multipath,
//! frequency-selective fade) in software, isolating the modem from acoustics.

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

fn demod_valid(config: &Config, rx: &[f32]) -> (usize, usize) {
    let mut demod = OfdmDemodulator::new(config);
    let guard = config.symbol_duration_samples();
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
            let mut parser = FrameParser::new(config);
            valid += parser.feed_bytes(&r.bytes).iter().filter(|f| f.valid).count();
            search_pos += r.consumed_samples;
        } else {
            search_pos += guard;
        }
        demod.reset();
    }
    (valid, det)
}

fn main() {
    let config = Config::from_preset_name("default");
    let audio = build_audio(&config, 6);
    let cp = config.cp_length;

    println!("multipath: y[n] = x[n] + 0.5*x[n-d]   (cp={})", cp);
    for d in [0usize, 2, 4, 8, 16, 24, 32, 40, 64, 128] {
        let rx: Vec<f32> = audio.iter().enumerate()
            .map(|(n, &s)| s + if n >= d { 0.5 * audio[n - d] } else { 0.0 })
            .collect();
        let (v, det) = demod_valid(&config, &rx);
        println!("  d={:>3}: {}/{} valid", d, v, det);
    }

    println!("single notch: y[n] = x[n] - 0.9*x[n-d]  (freq-selective null)");
    for d in [4usize, 8, 16, 32] {
        let rx: Vec<f32> = audio.iter().enumerate()
            .map(|(n, &s)| s - if n >= d { 0.9 * audio[n - d] } else { 0.0 })
            .collect();
        let (v, det) = demod_valid(&config, &rx);
        println!("  d={:>3}: {}/{} valid", d, v, det);
    }

    println!("amplitude clipping (models speaker/amp saturation): y=clip(g*x)");
    for g in [1.0f32, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0] {
        let peak = audio.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        let lvl = peak * g;
        let rx: Vec<f32> = audio.iter().map(|&s| (s * g).clamp(-1.0, 1.0)).collect();
        let (v, det) = demod_valid(&config, &rx);
        println!("  g={:>4} (drive {:.2} FS): {}/{} valid", g, lvl, v, det);
    }

    println!("additive white noise: valid frames vs in-band SNR");
    for snr in [40.0f32, 30.0, 25.0, 20.0, 15.0, 10.0] {
        let sig: f32 = audio.iter().map(|s| s * s).sum::<f32>() / audio.len() as f32;
        let noise = (sig / 10f32.powf(snr / 10.0)).sqrt();
        let mut rng = 0x1234_5678u32;
        let rx: Vec<f32> = audio.iter().map(|&s| {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let n = ((rng >> 8) as f32 / 16_777_216.0) * 2.0 - 1.0;
            s + n * noise
        }).collect();
        let (v, det) = demod_valid(&config, &rx);
        println!("  SNR {:>4.0} dB: {}/{} valid", snr, v, det);
    }

    println!("fractional delay (linear interp): delay = d + f");
    for d in [0usize, 1, 2, 4, 16, 32] {
        for f in [0.0f32, 0.25, 0.5, 0.75] {
            let rx: Vec<f32> = audio.iter().enumerate()
                .map(|(n, &s)| {
                    let a = if n >= d { audio[n - d] } else { 0.0 };
                    let b = if n > d { audio[n - d - 1] } else { 0.0 };
                    a * (1.0 - f) + b * f
                })
                .collect();
            let (v, det) = demod_valid(&config, &rx);
            print!("   d={d:>2}+{f:.2}: {v}/{det}  ");
            if f == 0.75 { println!(); }
        }
    }
}
