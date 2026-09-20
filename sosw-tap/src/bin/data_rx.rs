//! Receive framed OFDM data: record for a duration, then demodulate with the
//! same stride logic as `ota-validate`'s proven live loopback.
//!
//! Cross-machine counterpart to `frame-tx`.

use anyhow::Result;
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::config::Config;
use sosw_core::link::frame::FrameParser;
use sosw_core::physical::ofdm_demod::OfdmDemodulator;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "data-rx", about = "Record and demodulate framed OFDM data")]
struct Args {
    #[arg(short, long, default_value = "default")]
    preset: String,
    #[arg(long)]
    rx_device: Option<String>,
    #[arg(long, default_value_t = 12.0)]
    duration: f64,
    /// Expected payload size per frame (for the pattern check)
    #[arg(long, default_value_t = 0)]
    payload_size: usize,
    #[arg(long)]
    dump_rx: Option<std::path::PathBuf>,
}

fn find_input(host: &cpal::Host, name: &str) -> Result<cpal::Device> {
    let lower = name.to_lowercase();
    let devs: Vec<cpal::Device> = host.input_devices()?.collect();
    for d in &devs {
        if d.id().map(|i| format!("{}", i).to_lowercase().contains(&lower)).unwrap_or(false) {
            return Ok(d.clone());
        }
    }
    host.default_input_device().ok_or_else(|| anyhow::anyhow!("no input device matching '{}'", name))
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = Config::from_preset_name(&args.preset);
    let host = cpal::default_host();
    let dev = match &args.rx_device {
        Some(n) => find_input(&host, n)?,
        None => host.default_input_device().ok_or_else(|| anyhow::anyhow!("no default input"))?,
    };
    let cfg_in = dev.default_input_config()?.config();
    let ch = cfg_in.channels as usize;
    eprintln!(
        "data-rx: {} ({} ch, {} Hz, {:.1}s)",
        dev.id().map(|i| format!("{}", i)).unwrap_or_default(), ch, cfg_in.sample_rate, args.duration
    );

    let buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let b = buf.clone();
    let stream = dev.build_input_stream::<f32, _, _>(cfg_in, move |data: &[f32], _: &cpal::InputCallbackInfo| {
        if let Ok(mut v) = b.lock() {
            if ch > 1 {
                for f in data.chunks(ch) { v.push(f[0]); }
            } else {
                v.extend_from_slice(data);
            }
        }
    }, |e| eprintln!("in error: {}", e), None)?;
    stream.play()?;
    std::thread::sleep(Duration::from_secs_f64(args.duration));
    drop(stream);

    let captured = buf.lock().unwrap().clone();
    let max_amp = captured.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    let rms = (captured.iter().map(|s| s * s).sum::<f32>() / captured.len().max(1) as f32).sqrt();
    eprintln!("captured: {:.1}s max={:.4} RMS={:.6}", captured.len() as f64 / config.sample_rate as f64, max_amp, rms);
    if let Some(path) = &args.dump_rx {
        let mut bytes = Vec::with_capacity(captured.len() * 4);
        for s in &captured { bytes.extend_from_slice(&s.to_le_bytes()); }
        let _ = std::fs::write(path, &bytes);
    }
    if captured.len() < config.preamble_samples() {
        anyhow::bail!("too little audio captured");
    }

    // Same stride logic as ota-validate::ota_loopback.
    let mut demod = OfdmDemodulator::new(&config);
    let lead_in = (config.sample_rate as f64 * 0.5) as usize;
    let guard = config.symbol_duration_samples();
    let chunk_size = config.frame_samples() + guard * 6;
    let mut search_pos = lead_in;
    let mut detections = 0usize;
    let mut valid = 0usize;
    let mut last_len = 0usize;
    let expect = if args.payload_size > 0 { args.payload_size } else { config.payload_size };

    while search_pos + config.preamble_samples() < captured.len() {
        let chunk_end = std::cmp::min(search_pos + chunk_size, captured.len());
        let mut padded: Vec<f32> = captured[search_pos..chunk_end].to_vec();
        padded.extend(std::iter::repeat(0.0f32).take(guard * 4));
        if let Some(result) = demod.process_samples(&padded) {
            detections += 1;
            last_len = result.bytes.len();
            let mut parser = FrameParser::new(&config);
            let frames = parser.feed_bytes(&result.bytes);
            let pass = frames.iter().filter(|f| f.valid).count();
            if pass > 0 {
                valid += pass;
                let f = frames.iter().find(|f| f.valid).unwrap();
                eprintln!(
                    "VALID: type={} seq={} payload={}B (expect {}) peak={:.3} |H|={:.3}",
                    f.frame_type, f.sequence_number, f.payload.len(), expect, result.preamble_peak, result.mean_h_magnitude
                );
            } else {
                eprintln!(
                    "invalid: bytes={} peak={:.3} cfo={:.3} |H|={:.3} parser={}",
                    result.bytes.len(), result.preamble_peak, result.cfo_rad_per_sym,
                    result.mean_h_magnitude, parser.stats_summary()
                );
            }
            search_pos += result.consumed_samples;
        } else {
            search_pos += guard;
        }
        demod.reset();
    }

    eprintln!("=== data-rx: {} valid / {} detections (last frame bytes={}) ===", valid, detections, last_len);
    Ok(())
}
