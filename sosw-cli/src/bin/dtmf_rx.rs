use anyhow::Result;
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::physical::dtmf::{self, DtmfConfig};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "dtmf-rx", about = "Receive and decode DTMF control messages")]
struct Args {
    #[arg(long)]
    device: Option<String>,
    #[arg(short, long, default_value_t = 15.0)]
    duration: f64,
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

    let samples = capture(&args)?;

    eprintln!("Captured {} samples ({:.2}s)", samples.len(), samples.len() as f64 / cfg.sample_rate as f64);
    let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt();
    eprintln!("peak {:.4} rms {:.5}", peak, rms);

    let symbols = dtmf::decode(&samples, &cfg);
    eprintln!("Decoded {} symbols: {:X?}", symbols.len(), symbols);

    // Find the sync marker [A,5,A,5] and decode the payload nibbles after it.
    let sync = [0xAu8, 0x5, 0xA, 0x5];
    if let Some(pos) = symbols.windows(sync.len()).position(|w| w == sync) {
        let payload = &symbols[pos + sync.len()..];
        let bytes = dtmf::nibbles_to_bytes(payload);
        let text = String::from_utf8_lossy(&bytes);
        eprintln!("Payload ({} bytes): {:02x?}", bytes.len(), bytes);
        eprintln!("Text: {:?}", text);
    } else {
        eprintln!("No sync marker found");
    }
    Ok(())
}

fn capture(args: &Args) -> Result<Vec<f32>> {
    let host = cpal::default_host();
    let device = match &args.device {
        Some(n) => {
            let lower = n.to_lowercase();
            host.input_devices()?
                .find(|d| d.id().map(|id| format!("{}", id).to_lowercase().contains(&lower)).unwrap_or(false))
                .ok_or_else(|| anyhow::anyhow!("no input device matching '{}'", n))?
        }
        None => host.default_input_device().ok_or_else(|| anyhow::anyhow!("no default input"))?,
    };
    let in_cfg = device.default_input_config()?.config();
    let ch = in_cfg.channels as usize;
    eprintln!("RX on {} ({} Hz, {} ch)", device.id().map(|i| format!("{}", i)).unwrap_or_default(), in_cfg.sample_rate, ch);

    let buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let b = buf.clone();
    let stream = device.build_input_stream::<f32, _, _>(
        in_cfg,
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            if let Ok(mut v) = b.lock() {
                if ch > 1 {
                    for frame in data.chunks(ch) {
                        v.push(frame[0]);
                    }
                } else {
                    v.extend_from_slice(data);
                }
            }
        },
        |e| eprintln!("audio error: {}", e),
        None,
    )?;
    stream.play()?;
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < args.duration {
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(stream);
    let out = buf.lock().unwrap().clone();
    Ok(out)
}
