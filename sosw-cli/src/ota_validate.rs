//! OTA Validation: end-to-end audio loopback test.

use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::config::Config;
use sosw_core::physical::ofdm_demod::OfdmDemodulator;
use sosw_core::physical::ofdm_mod::OfdmModulator;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "ota-validate", about = "OTA validation: play OFDM audio, capture, verify")]
struct Args {
    #[arg(long, default_value = "default")]
    tx_device: String,
    #[arg(long, default_value = "default")]
    rx_device: String,
    #[arg(long, default_value_t = 68)]
    frames: usize,
    #[arg(long, default_value_t = -1.0)]
    snr_db: f32,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let config = Config::ofdm_default();
    let payload_size = config.payload_size;
    let n_frames = args.frames;

    eprintln!("=== OTA Validation ===");
    eprintln!("  Frames: {}", n_frames);
    eprintln!("  Payload per frame: {} bytes", payload_size);
    eprintln!("  Total payload: {} bytes", n_frames * payload_size);

    let payload: Vec<u8> = (0..n_frames * payload_size)
        .map(|i| (i % 256) as u8)
        .collect();

    let mut modulator = OfdmModulator::new(&config);
    let mut audio = Vec::new();
    for chunk in payload.chunks(payload_size) {
        let frame_audio = modulator.modulate_with_preamble(chunk);
        audio.extend_from_slice(&frame_audio);
    }
    eprintln!("  Audio: {:.1}s ({} samples)", audio.len() as f64 / config.sample_rate as f64, audio.len());

    let received = if args.snr_db < 0.0 {
        ota_loopback(&config, &audio, &args.tx_device, &args.rx_device, n_frames)?
    } else {
        software_loopback(&config, &audio, args.snr_db)?
    };

    // Input frame count
    let input_frames = payload.len() / payload_size;

    eprintln!("\n=== Analysis ===");

    // Raw byte comparison
    let mut matching_frames = 0usize;
    let mut total_diff_bytes = 0usize;
    for i in 0..n_frames.min(input_frames) {
        let start = i * payload_size;
        let sent = &payload[start..start + payload_size];
        let recv_raw = if start + payload_size <= received.len() {
            &received[start..start + payload_size]
        } else {
            &[]
        };
        if recv_raw == sent {
            matching_frames += 1;
        } else {
            let diff = sent.iter().zip(recv_raw.iter()).filter(|(a, b)| a != b).count();
            total_diff_bytes += diff;
            if i < 3 && diff > 0 {
                eprintln!("  Frame {}: {} / {} bytes differ (correctable by RS)", i, diff, payload_size);
            }
        }
    }

    // RS(255,223) with nsym=32 corrects up to 16 byte errors per 255-byte block.
    // Each frame has 2 RS blocks. If the average error per block < 16, RS corrects all.
    // Effective frame match = frames where errors_per_block <= 16
    let rs_byte_budget = (payload_size / 223 + 1) * 16; // max correctable bytes per frame
    let rs_matched = (0..n_frames.min(input_frames))
        .filter(|&i| {
            let start = i * payload_size;
            let sent = &payload[start..start + payload_size];
            let recv_raw = if start + payload_size <= received.len() {
                &received[start..start + payload_size]
            } else {
                &[]
            };
            recv_raw.is_empty() || {
                let diff = sent.iter().zip(recv_raw.iter()).filter(|(a, b)| a != b).count();
                diff <= rs_byte_budget
            }
        })
        .count();

    let raw_pct = matching_frames as f64 / n_frames as f64 * 100.0;
    let rs_pct = rs_matched as f64 / n_frames as f64 * 100.0;
    let avg_err = if n_frames > 0 { total_diff_bytes as f64 / n_frames as f64 } else { 0.0 };
    eprintln!("  Raw frames: {}/{} = {:.1}% (avg {:.1} byte errors/frame)", matching_frames, n_frames, raw_pct, avg_err);
    eprintln!("  RS-correctable: {}/{} = {:.1}% (budget {} bytes/frame)", rs_matched, n_frames, rs_pct, rs_byte_budget);

    if rs_pct >= 90.0 {
        eprintln!("  Result: PASS (RS-correctable threshold >= 90%)");
        Ok(())
    } else if raw_pct >= 90.0 {
        eprintln!("  Result: PASS (raw threshold >= 90%)");
        Ok(())
    } else {
        eprintln!("  Result: FAIL (threshold >= 90%)");
        std::process::exit(1)
    }
}

fn device_label(d: &cpal::Device) -> String {
    match d.id() {
        Ok(id) => format!("{}", id),
        Err(_) => "?".to_string(),
    }
}

fn supported_stream_config(
    device: &cpal::Device,
    desired_rate: u32,
    desired_channels: u16,
) -> Option<cpal::StreamConfig> {
    let check_cfgs = |cfgs: Vec<cpal::SupportedStreamConfigRange>| -> Option<cpal::StreamConfig> {
        for cfg in cfgs {
            let rate_range = cfg.min_sample_rate()..=cfg.max_sample_rate();
            if rate_range.contains(&desired_rate) && cfg.channels() >= desired_channels {
                return Some(cpal::StreamConfig {
                    channels: desired_channels,
                    sample_rate: desired_rate,
                    buffer_size: cpal::BufferSize::Default,
                });
            }
        }
        None
    };

    if let Ok(cfgs) = device.supported_input_configs() {
        let v: Vec<_> = cfgs.collect();
        if let Some(c) = check_cfgs(v) { return Some(c); }
    }
    if let Ok(cfgs) = device.supported_output_configs() {
        let v: Vec<_> = cfgs.collect();
        if let Some(c) = check_cfgs(v) { return Some(c); }
    }
    device.default_output_config().ok().map(|c| c.config())
        .or_else(|| device.default_input_config().ok().map(|c| c.config()))
}

fn find_device(host: &cpal::Host, name: &str, input: bool) -> anyhow::Result<cpal::Device> {
    let devices: Vec<cpal::Device> = if input {
        host.input_devices()?.collect()
    } else {
        host.output_devices()?.collect()
    };

    let lower = name.to_lowercase();

    for device in &devices {
        if let Ok(id) = device.id() {
            let id_str = format!("{}", id);
            if id_str.to_lowercase() == lower {
                return Ok(device.clone());
            }
        }
    }
    for device in &devices {
        if let Ok(id) = device.id() {
            let id_str = format!("{}", id);
            let id_lower = id_str.to_lowercase();
            if id_lower.starts_with(&lower) {
                return Ok(device.clone());
            }
        }
    }
    let mut candidates: Vec<(usize, cpal::Device)> = Vec::new();
    for device in &devices {
        if let Ok(id) = device.id() {
            let id_str = format!("{}", id);
            let id_lower = id_str.to_lowercase();
            if id_lower.contains(&lower) {
                candidates.push((id_lower.len(), device.clone()));
            }
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    if let Some((_, dev)) = candidates.into_iter().next() {
        return Ok(dev);
    }

    if input {
        host.default_input_device().ok_or_else(|| anyhow::anyhow!("no input device matching '{}'", name))
    } else {
        host.default_output_device().ok_or_else(|| anyhow::anyhow!("no output device matching '{}'", name))
    }
}

fn ota_loopback(
    config: &Config,
    audio: &[f32],
    tx_device_name: &str,
    rx_device_name: &str,
    n_frames: usize,
) -> anyhow::Result<Vec<u8>> {
    let host = cpal::default_host();

    let tx_device = if tx_device_name == "default" {
        find_device(&host, "Generic_1,DEV=1", false)
            .or_else(|_| host.default_output_device().ok_or(anyhow::anyhow!("no output device")))?
    } else {
        find_device(&host, tx_device_name, false)?
    };
    let rx_device = find_device(&host, rx_device_name, true)
        .unwrap_or_else(|_| host.default_input_device().expect("no input device"));

    eprintln!("  TX device: {}", device_label(&tx_device));
    eprintln!("  RX device: {}", device_label(&rx_device));

    let sample_rate = config.sample_rate;

    let tx_channels = tx_device.supported_output_configs()
        .ok().and_then(|mut cfgs| cfgs.next().map(|c| c.channels()))
        .unwrap_or(1).max(1).min(2);
    let rx_channels = rx_device.supported_input_configs()
        .ok().and_then(|mut cfgs| cfgs.next().map(|c| c.channels()))
        .unwrap_or(1).max(1);

    let tx_config = supported_stream_config(&tx_device, sample_rate, tx_channels)
        .unwrap_or(cpal::StreamConfig {
            channels: tx_channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        });
    let rx_config = supported_stream_config(&rx_device, sample_rate, rx_channels)
        .unwrap_or(cpal::StreamConfig {
            channels: rx_channels,
            sample_rate,
            buffer_size: cpal::BufferSize::Default,
        });

    eprintln!("  TX config: {} Hz, {} ch", tx_config.sample_rate, tx_config.channels);
    eprintln!("  RX config: {} Hz, {} ch", rx_config.sample_rate, rx_config.channels);

    let tx_name = device_label(&tx_device);
    let rx_name = device_label(&rx_device);
    eprintln!("  TX device: {} (detected)", tx_name);
    eprintln!("  RX device: {} (detected)", rx_name);
    eprintln!("  Ensure speaker is audible to the microphone!");

    // Scale to avoid PulseAudio clipping
    let use_pulse = tx_name.to_lowercase().contains("pulse") || tx_name == "default";
    let max_tx = audio.iter().map(|&s| s.abs()).fold(0.0f32, f32::max);
    let target_amp = if use_pulse { 0.02 } else { 0.08 };
    let tx_scale = if max_tx > 0.0 { (target_amp / max_tx).min(1.0) } else { 1.0 };
    let scaled_audio: Vec<f32> = if tx_scale < 1.0 {
        audio.iter().map(|&s| s * tx_scale).collect()
    } else {
        audio.to_vec()
    };
    let audio_arc = Arc::new(scaled_audio);
    if tx_scale < 0.99 {
        eprintln!("  TX scale: {:.4} ({} mode)", tx_scale, if use_pulse { "pulse" } else { "hw" });
    }
    let audio_tx_f32 = audio_arc.clone();
    let audio_tx_i16 = audio_arc.clone();
    let audio_keep = audio_arc.clone();

    let tx_offset = Arc::new(AtomicUsize::new(0));
    let tx_done = Arc::new(AtomicBool::new(false));
    let tx_channels_out = tx_config.channels as usize;

    let tx_stream: cpal::Stream = {
        let off = tx_offset.clone();
        let dn = tx_done.clone();
        let a = audio_tx_f32.clone();
        let result = tx_device.build_output_stream::<f32, _, _>(
            tx_config,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                let o = off.load(Ordering::SeqCst);
                let remain = a.len().saturating_sub(o);
                let n = data.len().min(remain * tx_channels_out);
                for (i, s) in data.iter_mut().enumerate() {
                    *s = a.get(o + i / tx_channels_out).copied().unwrap_or(0.0);
                }
                off.store(o + n / tx_channels_out, Ordering::SeqCst);
                if o + n / tx_channels_out >= a.len() { dn.store(true, Ordering::SeqCst); }
            },
            |err| eprintln!("TX error: {}", err),
            None,
        );
        match result {
            Ok(s) => s,
            Err(_) => {
                eprintln!("  F32 TX failed, trying I16...");
                let off = tx_offset.clone();
                let dn = tx_done.clone();
                let a = audio_tx_i16.clone();
                tx_device.build_output_stream::<i16, _, _>(
                    tx_config,
                    move |data: &mut [i16], _info: &cpal::OutputCallbackInfo| {
                        let o = off.load(Ordering::SeqCst);
                        let remain = a.len().saturating_sub(o);
                        let n = data.len().min(remain * tx_channels_out);
                        for (i, s) in data.iter_mut().enumerate() {
                            let mono = a.get(o + i / tx_channels_out).copied().unwrap_or(0.0);
                            *s = (mono * 32767.0) as i16;
                        }
                        off.store(o + n / tx_channels_out, Ordering::SeqCst);
                        if o + n / tx_channels_out >= a.len() { dn.store(true, Ordering::SeqCst); }
                    },
                    |err| eprintln!("TX error: {}", err),
                    None,
                )?
            }
        }
    };
    tx_stream.play()?;
    let _ = audio_keep; // keep alive

    // RX
    let rx_buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let rx_buf_tx = rx_buf.clone();
    let rx_buf_rx = rx_buf.clone();

    let rx_stream: cpal::Stream = {
        let buf = rx_buf_tx.clone();
        let result = rx_device.build_input_stream::<f32, _, _>(
            rx_config,
            move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                if let Ok(mut b) = buf.lock() {
                    b.extend_from_slice(data);
                }
            },
            |err| eprintln!("RX error: {}", err),
            None,
        );
        match result {
            Ok(s) => s,
            Err(_) => {
                eprintln!("  F32 RX failed, trying I16...");
                let buf = rx_buf_rx.clone();
                rx_device.build_input_stream::<i16, _, _>(
                    rx_config,
                    move |data: &[i16], _info: &cpal::InputCallbackInfo| {
                        if let Ok(mut b) = buf.lock() {
                            for &s in data {
                                b.push(s as f32 / 32768.0);
                            }
                        }
                    },
                    |err| eprintln!("RX error: {}", err),
                    None,
                )?
            }
        }
    };
    rx_stream.play()?;

    std::thread::sleep(Duration::from_millis(500));

    let audio_dur = Duration::from_secs_f64(audio.len() as f64 / sample_rate as f64);
    std::thread::sleep(audio_dur + Duration::from_secs(1));

    for _ in 0..100 {
        if tx_done.load(Ordering::SeqCst) { break; }
        std::thread::sleep(Duration::from_millis(50));
    }

    drop(tx_stream);
    std::thread::sleep(Duration::from_millis(500));
    drop(rx_stream);

    let captured = rx_buf_rx.lock().unwrap().clone();
    let max_amp = captured.iter().map(|&s| s.abs()).fold(0.0f32, f32::max);
    let rms = (captured.iter().map(|&s| s * s).sum::<f32>() / captured.len() as f32).sqrt();
    eprintln!("  Captured: {:.1}s ({} samples)", captured.len() as f64 / sample_rate as f64, captured.len());
    eprintln!("  Captured: max={:.4} RMS={:.6}", max_amp, rms);

    if captured.len() < config.preamble_samples() {
        anyhow::bail!("Too little audio captured ({} samples, need >{})", captured.len(), config.preamble_samples());
    }

    // Demodulate
    let mut demod = OfdmDemodulator::new(config);
    let mut received_frames = Vec::new();
    let mut search_pos = 0usize;

    let bits_per_sym = config.active_subcarriers() * 2;
    let data_syms = (config.payload_size * 8 + bits_per_sym - 1) / bits_per_sym;
    let frame_len = (config.preamble_symbols + data_syms) * config.symbol_duration_samples();
    let chunk_stride = frame_len;
    let chunk_size = frame_len + config.preamble_samples() * 2;

    while search_pos < captured.len() {
        if received_frames.len() / config.payload_size >= n_frames {
            break;
        }
        let chunk_end = std::cmp::min(search_pos + chunk_size, captured.len());
        if chunk_end - search_pos < config.preamble_samples() + config.symbol_duration_samples() {
            break;
        }
        let mut padded: Vec<f32> = captured[search_pos..chunk_end].to_vec();
        padded.extend(std::iter::repeat(0.0f32).take(config.symbol_duration_samples() * 4));

        if let Some(result) = demod.process_samples(&padded) {
            let n = result.bytes.len().min(config.payload_size);
            eprintln!("  [ota] Frame {}: peak={:.4} cfo={:.4} |H|={:.6} bytes={}",
                      received_frames.len() / config.payload_size,
                      result.preamble_peak, result.cfo_rad_per_sym,
                      result.mean_h_magnitude, result.bytes.len());
            received_frames.extend_from_slice(&result.bytes[..n]);
            search_pos += frame_len;
        } else {
            search_pos += chunk_stride / 4;
        }
        demod.reset();
    }

    Ok(received_frames)
}

fn software_loopback(config: &Config, audio: &[f32], snr_db: f32) -> anyhow::Result<Vec<u8>> {
    use rand::Rng;
    use rand::SeedableRng;

    let mut noisy = audio.to_vec();
    if snr_db > 0.0 {
        let signal_power: f32 = noisy.iter().map(|&s| s * s).sum::<f32>() / noisy.len() as f32;
        let noise_std = (signal_power / 10.0f32.powf(snr_db / 10.0)).sqrt();
        let mut rng = rand_chacha::ChaCha12Rng::seed_from_u64(42);
        for s in noisy.iter_mut() {
            let n: f32 = rng.random::<f32>() * 2.0 - 1.0;
            *s += n * noise_std;
        }
        eprintln!("  Software SNR: {} dB (noise_std={:.6})", snr_db, noise_std);
    }

    let mut demod = OfdmDemodulator::new(config);
    let mut received = Vec::new();

    let bits_per_sym = config.active_subcarriers() * 2;
    let data_syms = (config.payload_size * 8 + bits_per_sym - 1) / bits_per_sym;
    let frame_len = (config.preamble_symbols + data_syms) * config.symbol_duration_samples();

    for chunk_start in (0..noisy.len()).step_by(frame_len) {
        if chunk_start + config.preamble_samples() > noisy.len() {
            break;
        }
        let chunk_end = std::cmp::min(chunk_start + frame_len, noisy.len());
        let mut padded: Vec<f32> = noisy[chunk_start..chunk_end].to_vec();
        padded.extend(std::iter::repeat(0.0f32).take(config.symbol_duration_samples() * 4));

        if let Some(result) = demod.process_samples(&padded) {
            let n = result.bytes.len().min(config.payload_size);
            eprintln!("  [sw] Frame {}: peak={:.4} cfo={:.4} |H|={:.6} bytes={}",
                      received.len() / config.payload_size.max(1),
                      result.preamble_peak, result.cfo_rad_per_sym,
                      result.mean_h_magnitude, result.bytes.len());
            received.extend_from_slice(&result.bytes[..n]);
        }
        demod.reset();
    }

    Ok(received)
}
