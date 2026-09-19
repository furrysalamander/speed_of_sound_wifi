use anyhow::Result;
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::physical::dtmf::{self, DtmfConfig};

const SYNC: [u8; 4] = [0xA, 0x5, 0xA, 0x5];

#[derive(Parser)]
#[command(name = "dtmf-tx", about = "Transmit a DTMF-encoded control message")]
struct Args {
    /// Message to send (UTF-8 bytes). If omitted, sends a fixed test pattern.
    message: Option<String>,
    #[arg(long)]
    device: Option<String>,
    #[arg(short, long, default_value_t = 1.0)]
    gain: f32,
    /// Repeat the whole burst N times
    #[arg(short, long, default_value_t = 1)]
    repeat: usize,
    #[arg(long, default_value_t = 4800)]
    symbol_samples: usize,
    #[arg(long, default_value_t = 4800)]
    gap_samples: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut cfg = DtmfConfig::default();
    cfg.symbol_samples = args.symbol_samples;
    cfg.gap_samples = args.gap_samples;

    let bytes: Vec<u8> = match &args.message {
        Some(m) => m.as_bytes().to_vec(),
        None => b"SONICWIFI".to_vec(),
    };
    let mut nibbles = Vec::new();
    nibbles.extend_from_slice(&SYNC);
    nibbles.extend_from_slice(&dtmf::bytes_to_nibbles(&bytes));

    eprintln!(
        "DTMF: {} bytes -> {} symbols ({} ms/sym, {} ms gap)",
        bytes.len(),
        nibbles.len(),
        cfg.symbol_samples * 1000 / cfg.sample_rate as usize,
        cfg.gap_samples * 1000 / cfg.sample_rate as usize,
    );

    let one = dtmf::encode(&nibbles, &cfg);
    let mut audio = Vec::new();
    // Lead-in silence so a receiver that starts capturing slightly late still
    // catches the sync marker.
    audio.extend(std::iter::repeat(0.0f32).take(cfg.sample_rate as usize * 3 / 10));
    for _ in 0..args.repeat.max(1) {
        audio.extend_from_slice(&one);
        audio.extend(std::iter::repeat(0.0f32).take(cfg.symbol_samples));
    }
    if args.gain != 1.0 {
        for s in audio.iter_mut() {
            *s *= args.gain;
        }
    }
    let peak = audio.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    eprintln!("Audio: {:.2}s, peak {:.3}", audio.len() as f64 / cfg.sample_rate as f64, peak);

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
    let out_cfg = device.default_output_config()?.config();
    let ch = out_cfg.channels as usize;
    eprintln!("TX on {} ({} Hz, {} ch)", device.id().map(|i| format!("{}", i)).unwrap_or_default(), out_cfg.sample_rate, ch);

    let audio = std::sync::Arc::new(audio);
    let mut off = 0usize;
    let a = audio.clone();
    let stream = device.build_output_stream::<f32, _, _>(
        out_cfg,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            let frames = data.len() / ch;
            for i in 0..frames {
                let s = a.get(off + i).copied().unwrap_or(0.0);
                for c in 0..ch {
                    data[i * ch + c] = s;
                }
            }
            off += frames;
        },
        |e| eprintln!("audio error: {}", e),
        None,
    )?;
    stream.play()?;
    let dur_ms = (audio.len() as f64 / cfg.sample_rate as f64 * 1000.0) as u64;
    std::thread::sleep(std::time::Duration::from_millis(dur_ms + 400));
    eprintln!("TX done ({} ms)", dur_ms);
    Ok(())
}
