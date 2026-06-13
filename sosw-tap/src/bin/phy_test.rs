use anyhow::Result;
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::{Config, FrameAssembler, FrameParser, OfdmDemodulator, OfdmModulator};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "phy-test", about = "PHY loopback test using real audio hardware")]
struct Args {
    #[arg(short, long, default_value = "default")]
    preset: String,
    #[arg(long)]
    tx_device: Option<String>,
    #[arg(long)]
    rx_device: Option<String>,
    #[arg(short, long, default_value_t = 10)]
    frames: usize,
}

fn main() -> Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .init();

    let args = Args::parse();
    let cfg = Config::from_preset_name(&args.preset);

    let host = cpal::default_host();
    let out_device = find_device(&host, args.tx_device.as_deref(), true)?;
    let in_device = find_device(&host, args.rx_device.as_deref(), false)?;
    let out_config = out_device.default_output_config()?.config();
    let in_config = in_device.default_input_config()?.config();

    let sample_rate = cfg.sample_rate;
    let guard = cfg.symbol_duration_samples();
    let conv_frames = 0;
    let payload_size = cfg.payload_size;

    // Build test payloads
    let test_payloads: Vec<Vec<u8>> = (0..args.frames)
        .map(|i| {
            let mut p = vec![0u8; payload_size];
            p[0] = (i & 0xFF) as u8;
            p[1] = ((i >> 8) & 0xFF) as u8;
            p
        })
        .collect();

    // Build all audio upfront (ota_validate style)
    let mut modulator = OfdmModulator::new(&cfg);
    let mut assembler = FrameAssembler::new(&cfg);
    let mut audio = Vec::new();
    audio.extend(std::iter::repeat(0.0f32).take((sample_rate as f64 * 0.5) as usize));
    for i in 0..conv_frames {
        let train = vec![(i % 256) as u8; payload_size];
        let frame = assembler.assemble_frame(&train);
        audio.extend_from_slice(&modulator.modulate_with_preamble(&frame));
        audio.extend(std::iter::repeat(0.0f32).take(guard));
    }
    for payload in &test_payloads {
        let frame = assembler.assemble_frame(payload);
        audio.extend_from_slice(&modulator.modulate_with_preamble(&frame));
        audio.extend(std::iter::repeat(0.0f32).take(guard));
    }

    let audio_dur = Duration::from_secs_f64(audio.len() as f64 / sample_rate as f64);
    eprintln!("=== PHY Loopback Test ({} frames, {} preset, no training) ===",
        args.frames, args.preset);
    eprintln!("Audio: {:.1}s ({} samples), payload {} B/frame",
        audio_dur.as_secs_f64(), audio.len(), payload_size);

    // Play audio
    let audio_arc = Arc::new(audio);
    let tx_offset = Arc::new(AtomicUsize::new(0));
    let tx_done = Arc::new(AtomicBool::new(false));
    let tx_is_stereo = out_config.channels >= 2;

    let o = tx_offset.clone();
    let d = tx_done.clone();
    let a = audio_arc.clone();
    let out_stream = out_device.build_output_stream::<f32, _, _>(
        out_config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            let off = o.load(Ordering::SeqCst);
            let consumed = if tx_is_stereo {
                let frames = data.len() / 2;
                for i in 0..frames {
                    let s = a.get(off + i).copied().unwrap_or(0.0);
                    data[i * 2] = s;
                    data[i * 2 + 1] = s;
                }
                frames
            } else {
                let n = data.len().min(a.len().saturating_sub(off));
                for (i, s) in data[..n].iter_mut().enumerate() {
                    *s = a.get(off + i).copied().unwrap_or(0.0);
                }
                n
            };
            let new_off = off + consumed;
            o.store(new_off, Ordering::SeqCst);
            if new_off >= a.len() { d.store(true, Ordering::SeqCst); }
        },
        |err| eprintln!("TX error: {}", err),
        None,
    )?;
    out_stream.play()?;

    // Capture audio
    let rx_buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let rx = rx_buf.clone();
    let in_stream = in_device.build_input_stream::<f32, _, _>(
        in_config,
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            if let Ok(mut b) = rx.lock() {
                b.extend_from_slice(data);
            }
        },
        |err| eprintln!("RX error: {}", err),
        None,
    )?;
    in_stream.play()?;

    std::thread::sleep(Duration::from_millis(500));
    std::thread::sleep(audio_dur + Duration::from_secs(1));

    for _ in 0..100 {
        if tx_done.load(Ordering::SeqCst) { break; }
        std::thread::sleep(Duration::from_millis(50));
    }

    drop(out_stream);
    std::thread::sleep(Duration::from_millis(500));
    drop(in_stream);

    let captured = rx_buf.lock().unwrap().clone();
    let max_amp = captured.iter().map(|&s| s.abs()).fold(0.0f32, f32::max);
    let rms = (captured.iter().map(|&s| s * s).sum::<f32>() / captured.len() as f32).sqrt();
    eprintln!("Captured: {:.1}s ({} samples), max={:.4} RMS={:.6}",
        captured.len() as f64 / sample_rate as f64, captured.len(), max_amp, rms);

    // ota_validate-style chunked demodulation (with reset between frames)
    let lead_in = (sample_rate as f64 * 0.5) as usize;
    let chunk_size = cfg.frame_samples() + guard * 6;
    let mut search_pos = lead_in;
    let mut total_valid = 0usize;
    let mut total_corrupt = 0usize;
    let mut frame_count = 0usize;

    while search_pos + cfg.preamble_samples() < captured.len() && frame_count < args.frames {
        let chunk_end = std::cmp::min(search_pos + chunk_size, captured.len());
        let mut padded: Vec<f32> = captured[search_pos..chunk_end].to_vec();
        padded.extend(std::iter::repeat(0.0f32).take(cfg.symbol_duration_samples() * 4));

        let mut demod = OfdmDemodulator::new(&cfg);
        if let Some(result) = demod.process_samples(&padded) {
            let mut parser = FrameParser::new(&cfg);
            let frames = parser.feed_bytes(&result.bytes);
            if let Some(rx_frame) = frames.into_iter().find(|f| f.valid) {
                let expected = &test_payloads[frame_count];
                let min_len = std::cmp::min(rx_frame.payload.len(), expected.len());
                let errors: usize = rx_frame.payload[..min_len].iter()
                    .zip(expected[..min_len].iter())
                    .filter(|(a, b)| a != b)
                    .count();
                if errors == 0 && rx_frame.payload.len() == expected.len() {
                    total_valid += 1;
                    eprintln!("Frame {}: OK ({} bytes, peak={:.3})", frame_count, rx_frame.payload.len(), result.preamble_peak);
                } else {
                    total_corrupt += 1;
                    eprintln!("Frame {}: CORRUPT ({} errors, {} vs {} bytes)", frame_count, errors,
                        rx_frame.payload.len(), expected.len());
                }
                frame_count += 1;
                search_pos += result.consumed_samples;
            } else {
                search_pos += cfg.symbol_duration_samples();
            }
        } else {
            search_pos += cfg.symbol_duration_samples();
        }
    }

    let total = total_valid + total_corrupt;
    eprintln!("=== Results: {}/{} valid ({:.1}%) ===",
        total_valid, total, total_valid as f64 / total.max(1) as f64 * 100.0);
    if total_corrupt > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn find_device(host: &cpal::Host, name: Option<&str>, is_output: bool) -> Result<cpal::Device> {
    match name {
        Some(n) => {
            let lower = n.to_lowercase();
            let dev = if is_output { host.output_devices() } else { host.input_devices() }?
                .find(|d| d.id().map(|id| format!("{}", id).to_lowercase().contains(&lower)).unwrap_or(false))
                .ok_or_else(|| anyhow::anyhow!("device matching '{}' not found", n))?;
            Ok(dev)
        }
        None => {
            if is_output { host.default_output_device().ok_or_else(|| anyhow::anyhow!("no default output device")) }
            else { host.default_input_device().ok_or_else(|| anyhow::anyhow!("no default input device")) }
        }
    }
}
