use anyhow::Result;
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::{Config, FrameAssembler, OfdmModulator};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "frame-tx", about = "Transmit framed OFDM payloads (N frames)")]
struct Args {
    /// Optional payload file; if omitted a deterministic pattern is sent
    file: Option<PathBuf>,
    #[arg(short, long, default_value = "default")]
    preset: String,
    #[arg(long)]
    device: Option<String>,
    /// Linear gain applied to the modulated audio before playback
    #[arg(short, long, default_value_t = 1.0)]
    gain: f32,
    /// Number of data frames to send
    #[arg(short = 'n', long, default_value_t = 20)]
    frames: usize,
    /// Lead-in silence seconds (gives the RX side time to start)
    #[arg(long, default_value_t = 0.5)]
    lead_in: f32,
    /// Dummy training frames prepended so the receiver's consumed_samples
    /// stride converges before the real data frames.
    #[arg(long, default_value_t = 3)]
    train: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = Config::from_preset_name(&args.preset);
    let payload_size = config.payload_size;
    let guard = config.symbol_duration_samples();

    let payload: Vec<u8> = match &args.file {
        Some(path) => std::fs::read(path)?,
        None => (0..args.frames * payload_size).map(|i| (i % 256) as u8).collect(),
    };
    let n_frames = (payload.len() + payload_size - 1) / payload_size;

    let mut assembler = FrameAssembler::new(&config);
    let mut modulator = OfdmModulator::new(&config);
    let mut audio: Vec<f32> = Vec::new();
    audio.extend(std::iter::repeat(0.0f32).take((config.sample_rate as f32 * args.lead_in) as usize));
    for _ in 0..args.train {
        let c = vec![0u8; payload_size];
        let frame = assembler.assemble_frame(&c);
        audio.extend_from_slice(&modulator.modulate_with_preamble(&frame));
        audio.extend(std::iter::repeat(0.0f32).take(guard));
    }
    for chunk in payload.chunks(payload_size) {
        let mut c = chunk.to_vec();
        c.resize(payload_size, 0);
        let frame = assembler.assemble_frame(&c);
        audio.extend_from_slice(&modulator.modulate_with_preamble(&frame));
        audio.extend(std::iter::repeat(0.0f32).take(guard));
    }
    if args.gain != 1.0 {
        for s in audio.iter_mut() {
            *s *= args.gain;
        }
    }
    let peak = audio.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    let dur_ms = (audio.len() as f64 / config.sample_rate as f64) * 1000.0;
    eprintln!(
        "frame-tx: {} data frames + {} train, {}B payload/frame, {:.0}ms audio, gain {}x peak {:.3}",
        n_frames, args.train, payload_size, dur_ms, args.gain, peak
    );

    let host = cpal::default_host();
    let device = match &args.device {
        Some(n) => {
            let lower = n.to_lowercase();
            host.output_devices()?
                .find(|d| d.id().map(|id| format!("{}", id).to_lowercase().contains(&lower)).unwrap_or(false))
                .ok_or_else(|| anyhow::anyhow!("no output device matching '{}'", n))?
        }
        None => host.default_output_device().ok_or_else(|| anyhow::anyhow!("no default output"))?,
    };
    let dev_id = device.id().map(|id| format!("{}", id)).unwrap_or_default();
    let config_out = device.default_output_config()?.config();
    let ch = config_out.channels as usize;
    eprintln!("TX on {} ({} Hz, {} ch)", dev_id, config_out.sample_rate, ch);

    let audio = Arc::new(audio);
    let off = Arc::new(AtomicUsize::new(0));
    let a = audio.clone();
    let o = off.clone();
    let stream = device.build_output_stream(
        config_out,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            let base = o.load(Ordering::Relaxed);
            let frames = data.len() / ch;
            for i in 0..frames {
                let s = a.get(base + i).copied().unwrap_or(0.0);
                for c in 0..ch {
                    data[i * ch + c] = s;
                }
            }
            o.store(base + frames, Ordering::Relaxed);
        },
        |err| eprintln!("Audio error: {}", err),
        None,
    )?;
    stream.play()?;
    std::thread::sleep(std::time::Duration::from_millis(dur_ms as u64 + 500));
    while off.load(Ordering::Relaxed) < audio.len() {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    eprintln!("frame-tx: done");
    Ok(())
}
