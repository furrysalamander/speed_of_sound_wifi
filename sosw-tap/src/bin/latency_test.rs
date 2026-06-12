use anyhow::Result;
use clap::Parser;
use sosw_core::Config;
use sosw_tap::phy::Phy;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "latency-test", about = "Measure round-trip audio echo latency")]
struct Args {
    #[arg(long)]
    tx_device: Option<String>,
    #[arg(long)]
    rx_device: Option<String>,
    #[arg(short, long, default_value = "default")]
    preset: String,
    #[arg(short, long, default_value_t = 5)]
    frames: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let cfg = Config::from_preset_name(&args.preset);
    let mut phy = Phy::new(cfg.clone(), args.tx_device.as_deref(), args.rx_device.as_deref())?;
    let payload_size = cfg.payload_size;

    eprintln!("=== Latency Test ({} frames, {} preset) ===", args.frames, args.preset);
    eprintln!("Payload: {} B/frame, sample_rate={}Hz", payload_size, cfg.sample_rate);
    eprintln!("quantum={}, BufferSize=Fixed(256)", 1024u32);
    eprintln!("");

    let mut latencies = Vec::new();

    for i in 0..args.frames {
        let mut payload = vec![0u8; payload_size];
        payload[0] = (i & 0xFF) as u8;
        payload[1] = ((i >> 8) & 0xFF) as u8;

        phy.flush_rx();
        std::thread::sleep(Duration::from_millis(50));

        let audio = phy.transmit_frame(&payload, 0);
        let audio_dur_s = audio.len() as f64 / cfg.sample_rate as f64;
        let audio_dur = Duration::from_secs_f64(audio_dur_s);

        let tx_ready = Instant::now();
        phy.play_samples(&audio);

        // Small delay to let audio leave the output buffer
        std::thread::sleep(Duration::from_millis(30));
        let rx_start = Instant::now();

        let timeout = Duration::from_secs(5);
        let mut found = None;
        let mut polls = 0;
        while rx_start.elapsed() < timeout {
            polls += 1;
            if let Some(rx_frame) = phy.receive_frame() {
                found = Some(rx_frame);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        match found {
            None => eprintln!("Frame {}: LOST (no echo after {}s)", i, rx_start.elapsed().as_secs_f64()),
            Some(rx_frame) if rx_frame.payload == payload => {
                let elapsed = rx_start.elapsed(); // time from rx_start to detection
                let done = rx_start + elapsed;
                let since_play = done.duration_since(tx_ready);
                let audio_path = since_play.saturating_sub(audio_dur);
                latencies.push(audio_path);
                eprintln!("Frame {}: OK: play→detect={}ms audio_dur={}ms audio_path≈{}ms polls={}",
                    i, since_play.as_secs_f64() * 1000.0,
                    audio_dur_s * 1000.0,
                    audio_path.as_secs_f64() * 1000.0, polls);
            }
            Some(rx_frame) => {
                let errs = rx_frame.payload.iter()
                    .zip(payload.iter())
                    .filter(|(a, b)| a != b)
                    .count();
                eprintln!("Frame {}: CORRUPT ({} byte errs)", i, errs);
            }
        }
    }

    if !latencies.is_empty() {
        let vals: Vec<f64> = latencies.iter().map(|d| d.as_secs_f64()).collect();
        let mean = vals.iter().sum::<f64>() / vals.len() as f64;
        let min = vals.iter().fold(f64::MAX, |a, &b| a.min(b));
        let max = vals.iter().fold(f64::MIN, |a, &b| a.max(b));
        eprintln!("=== Round-trip latencies: min={:.0}ms, max={:.0}ms, mean={:.0}ms ({}/{} OK) ===",
            min * 1000.0, max * 1000.0, mean * 1000.0,
            latencies.len(), args.frames);
    }
    Ok(())
}
