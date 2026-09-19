use anyhow::Result;
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::{Config, FrameParser, OfdmDemodulator};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "rx-listen", about = "Listen for framed OFDM frames and report parsing")]
struct Args {
    #[arg(short, long, default_value = "default")]
    preset: String,
    #[arg(long)]
    rx_device: Option<String>,
    #[arg(short, long, default_value_t = 30.0)]
    duration: f64,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let cfg = Config::from_preset_name(&args.preset);

    let host = cpal::default_host();
    let dev = match &args.rx_device {
        Some(n) => {
            let lower = n.to_lowercase();
            host.input_devices()?
                .find(|d| d.id().map(|id| format!("{}", id).to_lowercase().contains(&lower)).unwrap_or(false))
                .ok_or_else(|| anyhow::anyhow!("no input device matching '{}'", n))?
        }
        None => host.default_input_device().ok_or_else(|| anyhow::anyhow!("no default input"))?,
    };

    let config_in = dev.default_input_config()?.config();
    let num_channels = config_in.channels as usize;
    eprintln!("RX device: {} ({} ch, {} Hz)", 
        dev.id().map(|id| format!("{}", id)).unwrap_or_default(),
        num_channels, config_in.sample_rate);

    let buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let buf_clone = buf.clone();
    let stream = dev.build_input_stream(config_in,
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            if let Ok(mut b) = buf_clone.lock() {
                if num_channels > 1 {
                    for frame in data.chunks(num_channels) {
                        b.push(frame[0]);
                    }
                } else {
                    b.extend_from_slice(data);
                }
            }
        },
        |err| eprintln!("Audio error: {}", err), None,
    )?;
    stream.play()?;

    let mut demod = OfdmDemodulator::new(&cfg);
    let mut rx_batch: Vec<f32> = Vec::new();
    let mut total_detections = 0usize;
    let mut valid_frames = 0usize;
    let mut rx_skip: usize = 0;
    let start = Instant::now();

    while start.elapsed().as_secs_f64() < args.duration {
        // Pull new samples into the batch.
        {
            let mut b = buf.lock().unwrap();
            rx_batch.extend(b.drain(..));
        }

        if rx_skip > 0 {
            let to_drain = rx_skip.min(rx_batch.len());
            rx_batch.drain(..to_drain);
            rx_skip = 0;
        }

        if rx_batch.len() < cfg.preamble_samples() {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }

        // Cap batch size to avoid unbounded growth.
        if rx_batch.len() > cfg.frame_samples() * 50 {
            let excess = rx_batch.len() - cfg.frame_samples() * 50;
            rx_batch.drain(..excess);
            demod.reset();
        }

        match demod.process_samples(&rx_batch) {
            Some(result) => {
                total_detections += 1;
                let consumed = result.consumed_samples.min(rx_batch.len());
                let mut parser = FrameParser::new(&cfg);
                let frames = parser.feed_bytes(&result.bytes);
                let valid = frames.iter().filter(|f| f.valid).count();
                if valid > 0 {
                    valid_frames += valid;
                    for f in &frames {
                        if f.valid {
                            eprintln!("VALID frame: type={} seq={} payload={}B peak={:.4} cfo={:.4} |H|={:.6}",
                                f.frame_type, f.sequence_number, f.payload.len(),
                                result.preamble_peak, result.cfo_rad_per_sym, result.mean_h_magnitude);
                        }
                    }
                } else {
                    let preview: String = result.bytes[..16.min(result.bytes.len())]
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<Vec<_>>()
                        .join("");
                    eprintln!("INVALID frame: peak={:.4} cfo={:.4} |H|={:.6} bytes={} head={} parser_stats={}",
                        result.preamble_peak, result.cfo_rad_per_sym, result.mean_h_magnitude,
                        result.bytes.len(), preview, parser.stats_summary());
                }
                rx_batch.drain(..consumed);
                demod.reset();
            }
            None => {
                // No preamble found; advance a little and try again.
                rx_skip = cfg.preamble_samples() / 4;
                demod.reset();
            }
        }
    }

    eprintln!();
    eprintln!("=== Done: {} valid out of {} preamble detections ===", valid_frames, total_detections);
    Ok(())
}
