//! OTA Validation: end-to-end audio loopback test.

use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::config::Config;
use sosw_core::physical::ofdm_demod::OfdmDemodulator;
use sosw_core::physical::ofdm_mod::OfdmModulator;
use sosw_core::physical::preamble;
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
    #[arg(short = 'p', long, default_value = "default")]
    preset: String,
    #[arg(long, default_value_t = 1.0)]
    gain: f32,
    /// Dump captured mic audio as raw little-endian f32 for offline analysis
    #[arg(long)]
    dump_rx: Option<std::path::PathBuf>,
    /// Dump generated TX audio as raw little-endian f32 for offline analysis
    #[arg(long)]
    dump_tx: Option<std::path::PathBuf>,
    /// OFDM layout overrides (any of these triggers a refit of the frame)
    #[arg(long)]
    fft: Option<usize>,
    #[arg(long)]
    cp: Option<usize>,
    #[arg(long)]
    sc_min: Option<usize>,
    #[arg(long)]
    sc_max: Option<usize>,
    #[arg(long)]
    rs: Option<usize>,
    #[arg(long)]
    payload: Option<usize>,
    /// Use differential subcarrier modulation (no absolute channel estimate)
    #[arg(long)]
    differential: bool,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let mut config = Config::from_preset_name(&args.preset);
    config.differential = args.differential;
    if args.differential || args.fft.is_some() || args.cp.is_some() || args.sc_min.is_some() || args.sc_max.is_some() || args.rs.is_some() || args.payload.is_some() {
        let fft = args.fft.unwrap_or(config.fft_size);
        let cp = args.cp.unwrap_or(config.cp_length);
        let sc_min = args.sc_min.unwrap_or(config.sc_min);
        let sc_max = args.sc_max.unwrap_or(config.sc_max);
        let rs = args.rs.unwrap_or(config.rs_nsym);
        let payload = args.payload.unwrap_or(config.payload_size);
        config = config.with_layout(fft, cp, sc_min, sc_max, rs, payload);
    }
    let payload_size = config.payload_size;
    let tx_gain = args.gain;
    let n_frames = args.frames;

    eprintln!("=== OTA Validation ===");
    eprintln!("  Frames: {}", n_frames);
    eprintln!("  Payload per frame: {} bytes", payload_size);
    eprintln!("  Total payload: {} bytes", n_frames * payload_size);

    let payload: Vec<u8> = (0..n_frames * payload_size)
        .map(|i| (i % 256) as u8)
        .collect();

    let mut modulator = OfdmModulator::new(&config);
    let guard = config.symbol_duration_samples();
    let conv_frames = 3; // training frames for consumed_samples to converge
    let mut total_frames = n_frames + conv_frames;
    let mut audio = Vec::new();
    // 0.5s silence before first frame for capture alignment
    audio.extend(std::iter::repeat(0.0f32).take((config.sample_rate as f64 * 0.5) as usize));
    // training frames for convergence (consumed_samples stride needs 2-3 frames)
    for i in 0..conv_frames {
        let train = vec![(i % 256) as u8; payload_size];
        audio.extend_from_slice(&modulator.modulate_with_preamble(&train));
        audio.extend(std::iter::repeat(0.0f32).take(guard));
    }
    for chunk in payload.chunks(payload_size) {
        let frame_audio = modulator.modulate_with_preamble(chunk);
        audio.extend_from_slice(&frame_audio);
        audio.extend(std::iter::repeat(0.0f32).take(guard));
    }
    if tx_gain != 1.0 {
        for s in audio.iter_mut() {
            *s *= tx_gain;
        }
    }
    let tx_rms = (audio.iter().map(|&s| s * s).sum::<f32>() / audio.len() as f32).sqrt();
    let tx_max = audio.iter().map(|&s| s.abs()).fold(0.0f32, f32::max);
    eprintln!("  Audio: {:.1}s ({} samples, {} data+{}train frames, max={:.4} RMS={:.6}, gain={}x)",
              audio.len() as f64 / config.sample_rate as f64, audio.len(), n_frames, conv_frames, tx_max, tx_rms, tx_gain);

    if let Some(path) = &args.dump_tx {
        let mut bytes = Vec::with_capacity(audio.len() * 4);
        for s in &audio {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, &bytes)?;
        eprintln!("  Dumped raw TX ({} f32) to {}", audio.len(), path.display());
    }

    let received = if args.snr_db < 0.0 {
        ota_loopback(&config, &audio, &args.tx_device, &args.rx_device, n_frames + conv_frames, args.dump_rx.as_deref())?
    } else {
        software_loopback(&config, &audio, args.snr_db)?
    };

    // Input frame count
    let input_frames = payload.len() / payload_size;

    eprintln!("\n=== Analysis ===");

    // Raw byte comparison (skip conv_frames training frames in received buffer)
    let mut matching_frames = 0usize;
    let mut total_diff_bytes = 0usize;
    for i in 0..n_frames.min(input_frames) {
        let start = i * payload_size;
        let rx_start = (conv_frames + i) * payload_size;
        let sent = &payload[start..start + payload_size];
        let recv_raw = if rx_start + payload_size <= received.len() {
            &received[rx_start..rx_start + payload_size]
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

    // RS(255,223) corrects up to nsym/2 byte errors per 255-byte block.
    // Effective frame match = frames where total byte errors <= budget.
    let rs_byte_budget = (payload_size / 223 + 1) * (config.rs_nsym / 2); // max correctable bytes per frame
    let rs_matched = (0..n_frames.min(input_frames))
        .filter(|&i| {
            let start = i * payload_size;
            let rx_start = (conv_frames + i) * payload_size;
            let sent = &payload[start..start + payload_size];
            let recv_raw = if rx_start + payload_size <= received.len() {
                &received[rx_start..rx_start + payload_size]
            } else {
                &[]
            };
            // A frame only counts as RS-correctable if the full payload was
            // actually recovered; a missing/short frame is a failure, not a pass.
            recv_raw.len() == payload_size && {
                let diff = sent.iter().zip(recv_raw.iter()).filter(|(a, b)| a != b).count();
                diff <= rs_byte_budget
            }
        })
        .count();

        // Debug: print per-frame error counts for all frames
        for i in 0..n_frames.min(input_frames) {
            let start = i * payload_size;
            let rx_start = (conv_frames + i) * payload_size;
            let sent = &payload[start..start + payload_size];
            let recv_raw = if rx_start + payload_size <= received.len() {
                &received[rx_start..rx_start + payload_size]
            } else {
                &[]
            };
            if recv_raw != sent {
                let diff = sent.iter().zip(recv_raw.iter()).filter(|(a, b)| a != b).count();
                eprintln!("  Frame {} errors: {} / {}", i, diff, payload_size);
            }
        }

        let raw_pct = matching_frames as f64 / n_frames as f64 * 100.0;
    let rs_pct = rs_matched as f64 / n_frames as f64 * 100.0;
    let avg_err = if n_frames > 0 { total_diff_bytes as f64 / n_frames as f64 } else { 0.0 };
    eprintln!("  Raw frames: {}/{} = {:.1}% (avg {:.1} byte errors/frame)", matching_frames, n_frames, raw_pct, avg_err);
    eprintln!("  RS-correctable: {}/{} = {:.1}% (budget {} bytes/frame)", rs_matched, n_frames, rs_pct, rs_byte_budget);

    let audio_dur = audio.len() as f64 / config.sample_rate as f64;
    let goodput = rs_matched as f64 * config.payload_size as f64 * 8.0 / audio_dur;
    eprintln!(
        "  Layout: fft={} cp={} sc={}-{} ({:.0}-{:.0} Hz, {:.0} Hz BW) rs={} payload={} syms/frame={} theory={:.0} bps",
        config.fft_size, config.cp_length, config.sc_min, config.sc_max,
        config.frequency_min(), config.frequency_max(), config.occupied_bandwidth(),
        config.rs_nsym, config.payload_size, config.data_symbols_per_frame, config.theoretical_bps(),
    );
    eprintln!("  Goodput: {:.0} bps ({} payload bytes RS-correct / {:.2}s)", goodput, rs_matched * config.payload_size, audio_dur);

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
    dump_rx: Option<&std::path::Path>,
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
    let tx_config = tx_device.default_output_config()?.config();
    let rx_config = rx_device.default_input_config()?.config();

    eprintln!("  TX config: {} Hz, {} ch", tx_config.sample_rate, tx_config.channels);
    eprintln!("  RX config: {} Hz, {} ch", rx_config.sample_rate, rx_config.channels);

    let tx_name = device_label(&tx_device);
    let rx_name = device_label(&rx_device);
    eprintln!("  TX device: {} (detected)", tx_name);
    eprintln!("  RX device: {} (detected)", rx_name);
    eprintln!("  Ensure speaker is audible to the microphone!");

    let audio_arc = Arc::new(audio.to_vec());

    let tx_offset = Arc::new(AtomicUsize::new(0));
    let tx_done = Arc::new(AtomicBool::new(false));

    let tx_is_stereo = tx_config.channels >= 2;
    let tx_stream: cpal::Stream = {
        let off = tx_offset.clone();
        let dn = tx_done.clone();
        let a = audio_arc;
        tx_device.build_output_stream::<f32, _, _>(
            tx_config,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                let o = off.load(Ordering::SeqCst);
                let consumed = if tx_is_stereo {
                    let frames = data.len() / 2;
                    for i in 0..frames {
                        let s = a.get(o + i).copied().unwrap_or(0.0);
                        data[i * 2] = s;
                        data[i * 2 + 1] = s;
                    }
                    frames
                } else {
                    let n = data.len().min(a.len().saturating_sub(o));
                    for (i, s) in data[..n].iter_mut().enumerate() {
                        *s = a.get(o + i).copied().unwrap_or(0.0);
                    }
                    n
                };
                let new_off = o + consumed;
                off.store(new_off, Ordering::SeqCst);
                if new_off >= a.len() { dn.store(true, Ordering::SeqCst); }
            },
            |err| eprintln!("TX error: {}", err),
            None,
        )?
    };
    tx_stream.play()?;

    let rx_buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let rx_buf_rx = rx_buf.clone();

    let rx_stream: cpal::Stream = {
        let buf = rx_buf_rx.clone();
        let rx_channels = rx_config.channels as usize;
        rx_device.build_input_stream::<f32, _, _>(
            rx_config,
            move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                if let Ok(mut b) = buf.lock() {
                    if rx_channels > 1 {
                        // Take the left channel only.
                        for frame in data.chunks(rx_channels) {
                            b.push(frame[0]);
                        }
                    } else {
                        b.extend_from_slice(data);
                    }
                }
            },
            |err| eprintln!("RX error: {}", err),
            None,
        )?
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

    if let Some(path) = dump_rx {
        let mut bytes = Vec::with_capacity(captured.len() * 4);
        for s in &captured {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(path, &bytes)?;
        eprintln!("  Dumped raw RX ({} f32) to {}", captured.len(), path.display());
    }

    if captured.len() < config.preamble_samples() {
        anyhow::bail!("Too little audio captured ({} samples, need >{})", captured.len(), config.preamble_samples());
    }

    // Demodulate (skip ~0.5s lead-in silence)
    let mut demod = OfdmDemodulator::new(config);
    let mut received_frames = Vec::new();
    let lead_in = (config.sample_rate as f64 * 0.5) as usize;
    let guard = config.symbol_duration_samples();

    // No preamble scan — start at lead_in. consumed_samples stride converges
    // the preamble offset to ~guard after the first frame.

    // Use 6× guard for chunk margin so the initial preamble offset (~576 samples
    // of audio latency) doesn't truncate data symbols. consumed_samples stride
    // converges the offset to ~guard after 1-2 frames.
    let chunk_size = config.frame_samples() + guard * 6;
    let mut search_pos = lead_in;

    while search_pos + config.preamble_samples() < captured.len() && received_frames.len() / config.payload_size < n_frames {
        let chunk_end = std::cmp::min(search_pos + chunk_size, captured.len());
        let mut padded: Vec<f32> = captured[search_pos..chunk_end].to_vec();
        padded.extend(std::iter::repeat(0.0f32).take(config.symbol_duration_samples() * 4));

        if let Some(result) = demod.process_samples(&padded) {
            let n = result.bytes.len().min(config.payload_size);
            let idx = received_frames.len() / config.payload_size;
            eprintln!("  [ota] Frame {}: peak={:.4} cfo={:.4} |H|={:.6} bytes={}",
                      idx, result.preamble_peak, result.cfo_rad_per_sym,
                      result.mean_h_magnitude, result.bytes.len());
            received_frames.extend_from_slice(&result.bytes[..n]);
            search_pos += result.consumed_samples;
        } else {
            search_pos += config.symbol_duration_samples();
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

    // skip initial silence, stride by full frame period. The transmitter emits
    // `frame_samples` of audio followed by one `guard` symbol of silence, so the
    // period is frame_samples + guard; using frame_samples alone drifts by one
    // symbol per frame and progressively misaligns the demodulator.
    let lead_in = (config.sample_rate as f64 * 0.5) as usize;
    let guard = config.symbol_duration_samples();
    let frame_period = config.frame_samples() + guard;

    for chunk_start in (lead_in..noisy.len()).step_by(frame_period) {
        if chunk_start + config.preamble_samples() > noisy.len() {
            break;
        }
        let chunk_end = std::cmp::min(chunk_start + frame_period, noisy.len());
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
