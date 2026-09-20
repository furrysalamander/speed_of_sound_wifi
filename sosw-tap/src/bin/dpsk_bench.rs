//! DPSK (+DFE) benchmark: software / self / tx / rx, like fsk_bench.

use anyhow::Result;
use clap::Parser;
use sosw_core::physical::dpsk::{DpskConfig, DpskDemodulator};
use sosw_tap::audio::DuplexAudio;
use std::time::Duration;

const SR: u32 = 48_000;

#[derive(Parser)]
#[command(name = "dpsk-bench", about = "DPSK+DFE acoustic benchmark")]
struct Args {
    #[arg(long, default_value = "software")]
    mode: String,
    #[arg(long, default_value_t = 1000.0)]
    symbol_rate: f32,
    #[arg(long, default_value_t = 4000.0)]
    carrier: f32,
    #[arg(long, default_value_t = 2)]
    bps: usize,
    #[arg(long, default_value_t = 0.3)]
    amplitude: f32,
    #[arg(long, default_value_t = 128)]
    payload: usize,
    #[arg(long, default_value_t = 6)]
    frames: usize,
    #[arg(long, default_value_t = 48)]
    preamble: usize,
    #[arg(long, default_value_t = 120)]
    guard_ms: u64,
    #[arg(long)]
    snr_db: Option<f32>,
    #[arg(long)]
    tx_device: Option<String>,
    #[arg(long)]
    rx_device: Option<String>,
    #[arg(long, default_value_t = 12000)]
    rx_ms: u64,
    #[arg(long, default_value_t = 6000)]
    latency_ms: u64,
    #[arg(long)]
    dump: Option<String>,
    #[arg(long)]
    load: Option<String>,
}

fn make_cfg(a: &Args) -> DpskConfig {
    DpskConfig {
        symbol_rate: a.symbol_rate,
        carrier: a.carrier,
        bits_per_symbol: a.bps,
        amplitude: a.amplitude,
        payload_size: a.payload,
        preamble_symbols: a.preamble,
        guard_samples: (a.guard_ms as usize * SR as usize) / 1000,
        ..DpskConfig::default()
    }
}

fn payloads(a: &Args) -> Vec<Vec<u8>> {
    (0..a.frames)
        .map(|i| (0..a.payload).map(|j| ((j * 17 + 7) % 256) as u8).collect())
        .collect()
}

fn build_script(cfg: &DpskConfig, ps: &[Vec<u8>]) -> Vec<f32> {
    let mut out = Vec::new();
    for p in ps {
        out.extend(cfg.encode_payload(p));
    }
    out
}

fn add_noise(audio: &mut [f32], snr_db: f32) {
    let sig = audio.iter().map(|s| s * s).sum::<f32>() / audio.len().max(1) as f32;
    let noise = (sig / 10f32.powf(snr_db / 10.0)).sqrt();
    let mut rng = 0x9E37_79B9u32;
    for s in audio.iter_mut() {
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let n = ((rng >> 8) as f32 / 16_777_216.0) * 2.0 - 1.0;
        *s += n * noise;
    }
}

fn main() -> Result<()> {
    let a = Args::parse();
    let cfg = make_cfg(&a);
    let ps = payloads(&a);
    let mut audio = build_script(&cfg, &ps);

    match a.mode.as_str() {
        "software" => {
            if let Some(snr) = a.snr_db {
                add_noise(&mut audio, snr);
            }
            report(&a, &cfg, &audio, &ps);
        }
        "tx" => {
            let dev = DuplexAudio::new(a.tx_device.as_deref(), a.rx_device.as_deref())?;
            println!("TOTAL_MS={}", (audio.len() as f64 / SR as f64 * 1000.0) as u64);
            dev.play_blocking(&audio, Duration::from_millis(200));
        }
        "rx" | "self" => {
            let rec = if let Some(path) = &a.load {
                sosw_tap::audio::load_capture(path)?
            } else {
                let dev = DuplexAudio::new(a.tx_device.as_deref(), a.rx_device.as_deref())?;
                dev.clear_rx();
                if a.mode == "self" {
                    dev.play_now(&audio);
                }
                let total = if a.mode == "self" {
                    audio.len() as f32 / SR as f32 * 1000.0 + a.latency_ms as f32 + 1000.0
                } else {
                    a.rx_ms as f32
                };
                eprintln!("recording {:.0} ms", total);
                std::thread::sleep(Duration::from_millis(total as u64));
                let rec = dev.take_rx();
                if let Some(path) = &a.dump {
                    sosw_tap::audio::save_f32(path, &rec)?;
                }
                rec
            };
            report(&a, &cfg, &rec, &ps);
        }
        other => anyhow::bail!("unknown mode '{}'", other),
    }
    Ok(())
}

fn report(a: &Args, cfg: &DpskConfig, audio: &[f32], ps: &[Vec<u8>]) {
    let dem = DpskDemodulator::new(cfg.clone());
    let frames = dem.decode_capture(audio);
    let mut valid = 0usize;
    let mut evm_sum = 0.0;
    let mut match_sum = 0.0;
    for d in &frames {
        if ps.iter().any(|p| p == &d.bytes) {
            valid += 1;
        }
        evm_sum += d.mean_evm;
        match_sum += d.preamble_match;
    }
    let cond = if frames.is_empty() {
        0.0
    } else {
        valid as f32 / frames.len() as f32 * 100.0
    };
    println!(
        "DPSK bps={} baud={:.0} car={:.0} raw={:.0}bps sent={} decoded={} valid={} cond={:.0}% evm={:.3} pre={:.2}",
        a.bps,
        cfg.symbol_rate,
        cfg.carrier,
        cfg.raw_bps(),
        ps.len(),
        frames.len(),
        valid,
        cond,
        evm_sum / frames.len().max(1) as f32,
        match_sum / frames.len().max(1) as f32,
    );
}
