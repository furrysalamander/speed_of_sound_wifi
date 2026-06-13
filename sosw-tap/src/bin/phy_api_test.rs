use anyhow::Result;
use clap::Parser;
use sosw_core::Config;
use sosw_tap::phy::Phy;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "phy-api-test", about = "Test Phy receive_frame directly")]
struct Args {
    #[arg(short, long, default_value = "default")]
    preset: String,
    #[arg(long)]
    tx_device: Option<String>,
    #[arg(long)]
    rx_device: Option<String>,
    #[arg(short, long, default_value_t = 5)]
    frames: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let cfg = Config::from_preset_name(&args.preset);

    let mut phy = Phy::new(cfg.clone(), args.tx_device.as_deref(), args.rx_device.as_deref())?;
    let payload_size = cfg.payload_size;

    eprintln!("=== Phy API Test ({} frames, {} preset) ===", args.frames, args.preset);
    eprintln!("Payload: {} B/frame", payload_size);

    let mut total_valid = 0;
    let mut total_lost = 0;
    let mut total_corrupt = 0;

    for i in 0..args.frames {
        let mut payload = vec![0u8; payload_size];
        payload[0] = (i & 0xFF) as u8;
        payload[1] = ((i >> 8) & 0xFF) as u8;

        // Flush, then transmit
        phy.flush_rx();
        std::thread::sleep(Duration::from_millis(50));
        // Generate and play the frame audio (transmit_frame adds guard internally)
        let audio = phy.transmit_frame(&payload, 0);
        let audio_dur = Duration::from_secs_f64(audio.len() as f64 / cfg.sample_rate as f64);
        eprintln!("Frame {}: TX {:.0}ms audio", i, audio_dur.as_secs_f64() * 1000.0);
        phy.play_samples(&audio);
        // There's no begin/end_tx_mute here — we want to receive our own echo
        // Wait for the audio to play + loop back through mic
        std::thread::sleep(audio_dur + Duration::from_millis(800));
        // Debug: check what's in the RX buffer via stats
        let s = phy.stats();
        eprintln!("  RX buf: {} batch: {}", s.rx_buffered, s.rx_batch_pending);

        let timeout = Duration::from_secs(5);
        let start = Instant::now();
        let mut found = None;
        while start.elapsed() < timeout {
            if let Some(rx_frame) = phy.receive_frame() {
                found = Some(rx_frame);
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        match found {
            None => {
                total_lost += 1;
                let s = phy.stats();
                eprintln!("  LOST (rx_buf={} batch={} tx_buf={})",
                    s.rx_buffered, s.rx_batch_pending, s.tx_buffered);
            }
            Some(rx_frame) if rx_frame.payload == payload => {
                total_valid += 1;
                eprintln!("  OK ({} B, valid={})", rx_frame.payload.len(), rx_frame.valid);
            }
            Some(rx_frame) => {
                total_corrupt += 1;
                let errs = rx_frame.payload.iter()
                    .zip(payload.iter())
                    .filter(|(a, b)| a != b)
                    .count();
                let s = phy.stats();
                eprintln!("  CORRUPT ({} byte errs, rx_buf={} batch={} tx_buf={})",
                    errs, s.rx_buffered, s.rx_batch_pending, s.tx_buffered);
            }
        }
    }

    let total = total_valid + total_lost + total_corrupt;
    eprintln!("=== Results: {}/{} valid, {} lost, {} corrupt ===",
        total_valid, total, total_lost, total_corrupt);
    if total_lost + total_corrupt > 0 {
        std::process::exit(1);
    }
    Ok(())
}
