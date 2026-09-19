use anyhow::Result;
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::{Config, FrameAssembler, OfdmModulator};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "frame-tx", about = "Transmit a framed OFDM payload")]
struct Args {
    file: PathBuf,
    #[arg(short, long, default_value = "default")]
    preset: String,
    #[arg(long)]
    device: Option<String>,
    /// Linear gain applied to the modulated audio before playback
    #[arg(short, long, default_value_t = 1.0)]
    gain: f32,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let config = Config::from_preset_name(&args.preset);
    let payload = std::fs::read(&args.file)?;

    let mut assembler = FrameAssembler::new(&config);
    let frame = assembler.assemble_frame(&payload);
    eprintln!(
        "Frame: {}B payload → {}B with FEC+CRC ({} data syms)",
        payload.len(),
        frame.len(),
        config.data_symbols_per_frame
    );

    let mut modulator = OfdmModulator::new(&config);
    let mut audio = modulator.modulate_with_preamble(&frame);
    if args.gain != 1.0 {
        for s in audio.iter_mut() {
            *s *= args.gain;
        }
    }
    let peak = audio.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    eprintln!("Gain {}x -> peak {:.3}", args.gain, peak);
    let audio = std::sync::Arc::new(audio);
    let audio_len = audio.len();

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
    eprintln!("TX on {} ({} Hz, {} ch)", dev_id, config_out.sample_rate, config_out.channels);

    let stream = device.build_output_stream(
        config_out,
        {
            let audio = audio.clone();
            move |data_out: &mut [f32], _: &cpal::OutputCallbackInfo| {
                for (i, sample) in data_out.iter_mut().enumerate() {
                    *sample = *audio.get(i).unwrap_or(&0.0);
                }
            }
        },
        |err| eprintln!("Audio error: {}", err),
        None,
    )?;
    stream.play()?;
    let dur_ms = (audio_len as f64 / config.sample_rate as f64) * 1000.0;
    std::thread::sleep(std::time::Duration::from_millis(dur_ms as u64 + 500));
    eprintln!("TX done ({} ms)", dur_ms as u64);
    Ok(())
}
