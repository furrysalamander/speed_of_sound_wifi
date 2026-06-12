//! OTA Validation: end-to-end audio loopback test.
//!
//! Generates test payload, modulates to audio, plays via speaker,
//! captures via microphone, demodulates, and compares.
//!
//! Usage:
//!   cargo run --bin ota-validate -- [--tx-device NAME] [--rx-device NAME] [--frames N] [--snr-db N]

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

    /// SNR in dB for optional software noise injection (negative = no noise)
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

    // 1. Generate test payload
    let payload: Vec<u8> = (0..n_frames * payload_size)
        .map(|i| (i % 256) as u8)
        .collect();

    // 2. Modulate all frames
    let mut modulator = OfdmModulator::new(&config);
    let mut audio = Vec::new();
    for chunk in payload.chunks(payload_size) {
        let frame_audio = modulator.modulate_with_preamble(chunk);
        audio.extend_from_slice(&frame_audio);
    }
    eprintln!("  Audio: {:.1}s ({} samples)", audio.len() as f64 / config.sample_rate as f64, audio.len());

    // 3. Play and capture (if no software-only flag)
    let received = if args.snr_db < 0.0 {
        // Hardware OTA loopback
        ota_loopback(&config, &audio, &args.tx_device, &args.rx_device, n_frames)?
    } else {
        // Software loopback with noise
        software_loopback(&config, &audio, args.snr_db)?
    };

    // 4. Compare
    eprintln!("\n=== Analysis ===");
    let mut matching_frames = 0usize;
    let fps = payload_size;
    for i in 0..n_frames {
        let start = i * fps;
        let sent = &payload[start..start + fps];
        let recv = if i * fps + fps <= received.len() {
            &received[i * fps..i * fps + fps]
        } else {
            &[]
        };
        if recv == sent {
            matching_frames += 1;
        } else if i < 3 {
            let xor: Vec<u8> = sent.iter().zip(recv.iter()).map(|(a, b)| a ^ b).collect();
            let diff_count = sent.iter().zip(recv.iter()).filter(|(a, b)| a != b).count();
            // Find bit error rate within the first 40 bytes
            let bit_xor: u32 = sent.iter().zip(recv.iter()).take(40)
                .map(|(a, b)| (a ^ b).count_ones()).sum();
            eprintln!("  Frame {}: {} / {} bytes differ ({} bit errors in first 40 bytes, xor first 8: {:02x?})",
                      i, diff_count, fps, bit_xor, &xor[..8.min(xor.len())]);
            if i == 0 {
                eprintln!("    Expected first 40: {:02x?}", &sent[..40]);
                eprintln!("    Received first 40: {:02x?}", &recv[..40.min(recv.len())]);
            }
        }
    }

    let pct = matching_frames as f64 / n_frames as f64 * 100.0;
    eprintln!("  Frames matching: {}/{} = {:.1}%", matching_frames, n_frames, pct);

    if pct >= 90.0 {
        eprintln!("  Result: PASS (threshold >= 90%)");
        Ok(())
    } else {
        eprintln!("  Result: FAIL (threshold >= 90%)");
        std::process::exit(1)
    }
}

fn supported_stream_config(
    device: &cpal::Device,
    desired_rate: u32,
    desired_channels: u16,
) -> Option<cpal::StreamConfig> {
    // Check all supported configs and find the closest match
    if let Ok(configs) = device.supported_input_configs() {
        for cfg in configs {
            let rate_range = cfg.min_sample_rate().0..=cfg.max_sample_rate().0;
            let channels = cfg.channels();
            if rate_range.contains(&desired_rate) && channels >= desired_channels {
                return Some(cpal::StreamConfig {
                    channels: desired_channels,
                    sample_rate: cpal::SampleRate(desired_rate),
                    buffer_size: cpal::BufferSize::Default,
                });
            }
        }
    }
    if let Ok(configs) = device.supported_output_configs() {
        for cfg in configs {
            let rate_range = cfg.min_sample_rate().0..=cfg.max_sample_rate().0;
            let channels = cfg.channels();
            if rate_range.contains(&desired_rate) && channels >= desired_channels {
                return Some(cpal::StreamConfig {
                    channels: desired_channels,
                    sample_rate: cpal::SampleRate(desired_rate),
                    buffer_size: cpal::BufferSize::Default,
                });
            }
        }
    }
    // Fallback: use the device's default config
    device.default_output_config().ok().map(|c| c.config()).or_else(|| {
        device.default_input_config().ok().map(|c| c.config())
    })
}

fn ota_loopback(
    config: &Config,
    audio: &[f32],
    tx_device_name: &str,
    rx_device_name: &str,
    n_frames: usize,
) -> anyhow::Result<Vec<u8>> {
    let host = cpal::default_host();

    let tx_device = find_device(&host, tx_device_name, false)
        .unwrap_or_else(|_| host.default_output_device().expect("no output device"));
    let rx_device = find_device(&host, rx_device_name, true)
        .unwrap_or_else(|_| host.default_input_device().expect("no input device"));

    eprintln!("  TX device: {}", tx_device.name().unwrap_or("?".into()));
    eprintln!("  RX device: {}", rx_device.name().unwrap_or("?".into()));

    let sample_rate = config.sample_rate;

    let tx_config = supported_stream_config(&tx_device, sample_rate, 1)
        .unwrap_or(cpal::StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        });
    let rx_config = supported_stream_config(&rx_device, sample_rate, 1)
        .unwrap_or(cpal::StreamConfig {
            channels: 1,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        });

    eprintln!("  TX config: {} Hz, {} ch", tx_config.sample_rate.0, tx_config.channels);
    eprintln!("  RX config: {} Hz, {} ch", rx_config.sample_rate.0, rx_config.channels);

    // Quick check: if TX and RX are the SAME device (loopback), warn about cable
    let tx_name = tx_device.name().unwrap_or_default();
    let rx_name = rx_device.name().unwrap_or_default();
    if tx_name == rx_name {
        eprintln!("  WARNING: TX and RX on same device — needs loopback cable");
    } else {
        eprintln!("  TX device: {} (detected)", tx_name);
    eprintln!("  RX device: {} (detected)", rx_name);
    eprintln!("  Ensure speaker is audible to the microphone!");
    }

    // RX buffer shared between callback and main thread
    let rx_buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let rx_buf_tx = rx_buf.clone();
    let rx_buf_rx = rx_buf.clone();

    // TX: play audio samples — try f32 first, fall back to i16
    // Scale audio to avoid clipping on PulseAudio paths
    let max_tx = audio.iter().map(|&s| s.abs()).fold(0.0f32, f32::max);
    let tx_scale = if max_tx > 0.0 { (0.02 / max_tx).min(1.0) } else { 1.0 };
    let scaled_audio: Vec<f32> = audio.iter().map(|&s| s * tx_scale).collect();
    let audio_arc = Arc::new(scaled_audio);
    let audio_tx_f32 = audio_arc.clone();
    let audio_tx_i16 = audio_arc.clone();
    let audio_keep = audio_arc.clone();
    if tx_scale < 0.99 {
        eprintln!("  TX scale: {:.4} (prevent clipping)", tx_scale);
    }
    let tx_offset = Arc::new(AtomicUsize::new(0));
    let tx_done = Arc::new(AtomicBool::new(false));

    let tx_stream: cpal::Stream = {
        let tx_off_f32 = tx_offset.clone();
        let tx_dn_f32 = tx_done.clone();
        let audio_f32 = audio_tx_f32.clone();
        let result_f32 = tx_device.build_output_stream::<f32, _, _>(
            &tx_config,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                let off = tx_off_f32.load(Ordering::SeqCst);
                let remain = audio_f32.len().saturating_sub(off);
                let n = data.len().min(remain);
                for (i, s) in data.iter_mut().enumerate() {
                    *s = audio_f32.get(off + i).copied().unwrap_or(0.0);
                }
                tx_off_f32.store(off + n, Ordering::SeqCst);
                if off + n >= audio_f32.len() { tx_dn_f32.store(true, Ordering::SeqCst); }
            },
            |err| eprintln!("TX error: {}", err),
            None,
        );
        match result_f32 {
            Ok(s) => s,
            Err(_) => {
                eprintln!("  F32 TX failed, trying I16...");
                let tx_off_i16 = tx_offset.clone();
                let tx_dn_i16 = tx_done.clone();
                let audio_i16 = audio_tx_i16.clone();
                tx_device.build_output_stream::<i16, _, _>(
                    &tx_config,
                    move |data: &mut [i16], _info: &cpal::OutputCallbackInfo| {
                        let off = tx_off_i16.load(Ordering::SeqCst);
                        let remain = audio_i16.len().saturating_sub(off);
                        let n = data.len().min(remain);
                        for (i, s) in data.iter_mut().enumerate() {
                            *s = (audio_i16.get(off + i).copied().unwrap_or(0.0) * 32767.0) as i16;
                        }
                        tx_off_i16.store(off + n, Ordering::SeqCst);
                        if off + n >= audio_i16.len() { tx_dn_i16.store(true, Ordering::SeqCst); }
                    },
                    |err| eprintln!("TX error: {}", err),
                    None,
                )?
            }
        }
    };

    // RX: capture samples — try f32 first, fall back to i16 with conversion
    let rx_stream: cpal::Stream = {
        let rx_buf_tx_i16 = rx_buf_tx.clone();
        let result_f32 = rx_device.build_input_stream(
            &rx_config,
            move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                if let Ok(mut buf) = rx_buf_tx.lock() {
                    buf.extend_from_slice(data);
                }
            },
            |err| eprintln!("RX error: {}", err),
            None,
        );
        match result_f32 {
            Ok(s) => s,
            Err(err_f32) => {
                eprintln!("  F32 RX failed ({:?}), trying I16...", err_f32);
                let rx_buf_tx_i16 = rx_buf_rx.clone();
                rx_device.build_input_stream::<i16, _, _>(
                    &rx_config,
                    move |data: &[i16], _info: &cpal::InputCallbackInfo| {
                        if let Ok(mut buf) = rx_buf_tx_i16.lock() {
                            // Convert i16 to f32
                            for &s in data {
                                buf.push(s as f32 / 32768.0);
                            }
                        }
                    },
                    |err| eprintln!("RX error: {}", err),
                    None,
                )?
            }
        }
    };

    // Start RX first, then TX (with small gap for sync)
    rx_stream.play()?;
    std::thread::sleep(Duration::from_millis(500));
    tx_stream.play()?;

    // Wait for TX to finish
    let audio_dur = Duration::from_secs_f64(audio.len() as f64 / sample_rate as f64);
    std::thread::sleep(audio_dur + Duration::from_secs(1));

    // Wait until TX finishes or timeout
    for _ in 0..100 {
        if tx_done.load(Ordering::SeqCst) { break; }
        std::thread::sleep(Duration::from_millis(50));
    }

    drop(tx_stream);
    // Keep RX alive briefly to capture any trailing audio
    std::thread::sleep(Duration::from_millis(500));
    drop(rx_stream);

    let captured = rx_buf_rx.lock().unwrap().clone();
    eprintln!("  Captured: {:.1}s ({} samples)", captured.len() as f64 / sample_rate as f64, captured.len());

    // Check signal amplitude and approximate SNR
    let max_amp = captured.iter().map(|&s| s.abs()).fold(0.0f32, f32::max);
    let rms = (captured.iter().map(|&s| s * s).sum::<f32>() / captured.len() as f32).sqrt();
    eprintln!("  Captured: max={:.4} RMS={:.6}", max_amp, rms);

    if captured.len() < config.preamble_samples() {
        anyhow::bail!("Too little audio captured ({} samples, need >{})", captured.len(), config.preamble_samples());
    }

    // Demodulate — search the full buffer, not chunked
    let mut demod = OfdmDemodulator::new(config);
    let mut received_frames = Vec::new();
    let mut search_pos = 0usize;

    let bits_per_sym = config.active_subcarriers() * 2;
    let data_syms = (config.payload_size * 8 + bits_per_sym - 1) / bits_per_sym;
    let frame_len = (config.preamble_symbols + data_syms) * config.symbol_duration_samples();

    // First, try to find preamble in the raw audio using cross-correlation
    // to identify where the signal actually starts
    use sosw_core::physical::preamble;
    let preamble_raw = preamble::generate_preamble_audio(config);
    let corr = preamble::compute_cross_correlation(&captured, &preamble_raw);
    let max_corr = corr.iter().cloned().fold(0.0f32, f32::max);
    let min_corr = corr.iter().cloned().fold(f32::MAX, f32::min);
    let mean_val = if corr.is_empty() { 0.0 } else { corr.iter().sum::<f32>() / corr.len() as f32 };
    let preamble_energy: f32 = preamble_raw.iter().map(|&s| s * s).sum::<f32>().max(1e-12);
    eprintln!("  Raw correlation: max={:.4} min={:.4} mean={:.6} (preamble_energy={:.1})", max_corr, min_corr, mean_val, preamble_energy);

    if !corr.is_empty() {
        let peak_idx = corr.iter().enumerate()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i).unwrap_or(0);
        let signal_energy: f32 = captured.iter().map(|&s| s * s).sum::<f32>().max(1e-12);
        let norm_peak = corr[peak_idx] / (preamble_energy * signal_energy).sqrt();
        eprintln!("  Peak at offset {} samples ({:.2}s): norm={:.6}", 
                  peak_idx, peak_idx as f64 / config.sample_rate as f64, norm_peak);

        // Also check energy of the signal around the peak
        let win_start = peak_idx;
        let win_end = std::cmp::min(win_start + preamble_raw.len(), captured.len());
        let win_energy: f32 = captured[win_start..win_end].iter().map(|&s| s * s).sum::<f32>().max(1e-12);
        let local_norm = corr[peak_idx] / (preamble_energy * win_energy).sqrt();
        eprintln!("  Local energy at peak: {:.6}, normalized: {:.6}", win_energy, local_norm);
    }

    // Use a sliding window approach: process the full audio in large overlapping chunks
    let chunk_stride = frame_len; // advance by one frame at a time
    let chunk_size = frame_len + config.preamble_samples() * 2; // room for preamble anywhere

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

fn find_device(host: &cpal::Host, name: &str, input: bool) -> anyhow::Result<cpal::Device> {
    let devices: Vec<cpal::Device> = if input {
        host.input_devices()?.collect()
    } else {
        host.output_devices()?.collect()
    };

    let lower = name.to_lowercase();


    // Phase 1: exact match
    for device in &devices {
        if let Ok(dn) = device.name() {
            if dn.to_lowercase() == lower {
                return Ok(device.clone());
            }
        }
    }

    // Phase 2: device name STARTS WITH the query (e.g. "plughw:CARD=..." starts with "plughw:")
    for device in &devices {
        if let Ok(dn) = device.name() {
            if dn.to_lowercase().starts_with(&lower) {
                return Ok(device.clone());
            }
        }
    }

    // Phase 3: query is a substring of the device name (prefer longer/more specific matches)
    let mut candidates: Vec<(usize, cpal::Device)> = Vec::new();
    for device in &devices {
        if let Ok(dn) = device.name() {
            let dn_lower = dn.to_lowercase();
            if dn_lower.contains(&lower) {
                // Prefer longer device names (more specific match)
                candidates.push((dn_lower.len(), device.clone()));
            }
        }
    }
    candidates.sort_by(|a, b| b.0.cmp(&a.0)); // longest match first
    if let Some((_, dev)) = candidates.into_iter().next() {
        return Ok(dev);
    }

    // Fall back to default
    if input {
        host.default_input_device().ok_or_else(|| anyhow::anyhow!("no input device matching '{}'", name))
    } else {
        host.default_output_device().ok_or_else(|| anyhow::anyhow!("no output device matching '{}'", name))
    }
}
