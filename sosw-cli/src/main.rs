use clap::{Parser, Subcommand};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::Config;
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "sosw-cli", about = "Speed of Sound WiFi CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    ListDevices,
    Tx {
        file: PathBuf,
        #[arg(short = 'd', long)]
        device: Option<String>,
    },
    Rx {
        #[arg(short = 'n', long, default_value = "100")]
        count: usize,
        #[arg(short = 'd', long)]
        device: Option<String>,
        #[arg(short = 'o', long)]
        output: Option<PathBuf>,
    },
    Test {
        #[arg(short = 't', long, default_value = "10")]
        duration: f64,
        #[arg(short = 'd', long)]
        device: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::ofdm_default();

    match cli.command {
        Commands::ListDevices => list_devices(),
        Commands::Tx { file, device } => tx_mode(&config, &file, device.as_deref()),
        Commands::Rx { count, device, output } => rx_mode(&config, count, device.as_deref(), output.as_ref()),
        Commands::Test { duration, device } => test_mode(&config, duration, device.as_deref()),
    }
}

fn device_label(d: &cpal::Device) -> String {
    match d.id() {
        Ok(id) => format!("{}", id),
        Err(_) => "?".to_string(),
    }
}

fn list_devices() -> anyhow::Result<()> {
    let host = cpal::default_host();
    println!("Host: {}", host.id().name());

    println!("\nInput devices:");
    for device in host.input_devices()? {
        let config = device.default_input_config()?;
        println!("  {} ({} Hz, {} channels, {:?})", device_label(&device),
                 config.sample_rate(), config.channels(), config.sample_format());
    }

    println!("\nOutput devices:");
    for device in host.output_devices()? {
        let config = device.default_output_config()?;
        println!("  {} ({} Hz, {} channels, {:?})", device_label(&device),
                 config.sample_rate(), config.channels(), config.sample_format());
    }
    Ok(())
}

fn find_device(name_substring: &str, kind: &str) -> anyhow::Result<cpal::Device> {
    let host = cpal::default_host();
    let devices: Box<dyn Iterator<Item = cpal::Device>> = match kind {
        "input" => Box::new(host.input_devices()?),
        "output" => Box::new(host.output_devices()?),
        _ => anyhow::bail!("unknown device kind: {}", kind),
    };

    let lower = name_substring.to_lowercase();
    for device in devices {
        if let Ok(id) = device.id() {
            let id_str = format!("{}", id);
            if id_str.to_lowercase().contains(&lower) {
                return Ok(device);
            }
        }
    }
    anyhow::bail!("no {} device matching '{}' found", kind, name_substring)
}

fn default_output_device() -> anyhow::Result<cpal::Device> {
    let host = cpal::default_host();
    host.default_output_device().ok_or_else(|| anyhow::anyhow!("no default output device"))
}

fn default_input_device() -> anyhow::Result<cpal::Device> {
    let host = cpal::default_host();
    host.default_input_device().ok_or_else(|| anyhow::anyhow!("no default input device"))
}

fn tx_mode(config: &Config, file: &PathBuf, device_name: Option<&str>) -> anyhow::Result<()> {
    let data = std::fs::read(file)?;
    let device = match device_name {
        Some(name) => find_device(name, "output")?,
        None => default_output_device()?,
    };
    eprintln!("TX: {} bytes from {}, device: {}", data.len(), file.display(), device_label(&device));

    let mut modulator = sosw_core::OfdmModulator::new(config);
    let audio = modulator.modulate_with_preamble(&data);
    let audio_len = audio.len();

    let config_out = device.default_output_config()?.config();

    let stream = device.build_output_stream(
        config_out,
        move |data_out: &mut [f32], _info: &cpal::OutputCallbackInfo| {
            for (i, sample) in data_out.iter_mut().enumerate() {
                *sample = *audio.get(i).unwrap_or(&0.0);
            }
        },
        |err| eprintln!("Audio error: {}", err),
        None,
    )?;

    stream.play()?;
    let duration_ms = (audio_len as f64 / config.sample_rate as f64) * 1000.0;
    std::thread::sleep(std::time::Duration::from_millis(duration_ms as u64 + 500));
    eprintln!("TX done ({} ms)", duration_ms as u64);
    Ok(())
}

fn rx_mode(config: &Config, count: usize, device_name: Option<&str>, output: Option<&PathBuf>) -> anyhow::Result<()> {
    let device = match device_name {
        Some(name) => find_device(name, "input")?,
        None => default_input_device()?,
    };
    eprintln!("RX device: {}", device_label(&device));

    let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
    let buf_clone = buf.clone();

    let config_in = device.default_input_config()?.config();

    let stream = device.build_input_stream(
        config_in,
        move |data: &[f32], _info: &cpal::InputCallbackInfo| {
            let mut b = buf_clone.lock().unwrap();
            b.extend_from_slice(data);
        },
        |err| eprintln!("Audio error: {}", err),
        None,
    )?;

    stream.play()?;

    let mut demodulator = sosw_core::OfdmDemodulator::new(config);
    let mut received = Vec::new();
    let mut frames_decoded = 0usize;

    eprint!("Receiving");
    while frames_decoded < count {
        let chunk = {
            let mut b = buf.lock().unwrap();
            let len = b.len();
            if len < config.preamble_samples() {
                std::mem::drop(b);
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            let chunk: Vec<f32> = b.drain(..len / 2).collect();
            chunk
        };

        if let Some(result) = demodulator.process_samples(&chunk) {
            let n = result.bytes.len().min(config.payload_size);
            received.extend_from_slice(&result.bytes[..n]);
            frames_decoded += 1;
            eprint!("\rReceived {} frames (peak={:.3}, cfo={:.4})", frames_decoded, result.preamble_peak, result.cfo_rad_per_sym);
        }
    }
    eprintln!();

    drop(stream);

    match output {
        Some(path) => std::fs::write(path, &received)?,
        None => std::io::stdout().write_all(&received)?,
    }

    eprintln!("Wrote {} bytes", received.len());
    Ok(())
}

fn test_mode(config: &Config, duration_secs: f64, device_name: Option<&str>) -> anyhow::Result<()> {
    let device = match device_name {
        Some(name) => find_device(name, "input")?,
        None => default_input_device()?,
    };
    eprintln!("Test mode on device: {} for {:.1}s", device_label(&device), duration_secs);

    let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
    let buf_clone = buf.clone();

    let config_in = device.default_input_config()?.config();

    let stream = device.build_input_stream(
        config_in,
        move |data: &[f32], _info: &cpal::InputCallbackInfo| {
            let mut b = buf_clone.lock().unwrap();
            b.extend_from_slice(data);
        },
        |err| eprintln!("Audio error: {}", err),
        None,
    )?;

    stream.play()?;

    let mut demodulator = sosw_core::OfdmDemodulator::new(config);
    let mut total_frames = 0usize;
    let mut valid_frames = 0usize;
    let mut last_peak = 0.0f32;
    let mut last_cfo = 0.0f32;
    let mut last_h = 0.0f32;
    let start = std::time::Instant::now();

    eprintln!("{:-<70}", "");
    eprintln!("{:<6} {:<10} {:<10} {:<12} {:<12} {:<10}", "Frame", "Peak", "CFO", "|H| mean", "Dropped", "Rate");
    eprintln!("{:-<70}", "");

    while start.elapsed().as_secs_f64() < duration_secs {
        let chunk = {
            let mut b = buf.lock().unwrap();
            let len = b.len();
            if len < config.preamble_samples() {
                std::mem::drop(b);
                std::thread::sleep(std::time::Duration::from_millis(20));
                continue;
            }
            let chunk: Vec<f32> = b.drain(..len / 2).collect();
            chunk
        };

        total_frames += 1;
        if let Some(result) = demodulator.process_samples(&chunk) {
            valid_frames += 1;
            last_peak = result.preamble_peak;
            last_cfo = result.cfo_rad_per_sym;
            last_h = result.mean_h_magnitude;
            demodulator.reset();
        }

        if total_frames % 5 == 0 {
            let rate = valid_frames as f64 / total_frames as f64 * 100.0;
            let dropped = total_frames - valid_frames;
            eprint!("\r{:<6} {:<10.4} {:<10.4} {:<12.6} {:<12} {:<10.1}%",
                    valid_frames, last_peak, last_cfo, last_h, dropped, rate,);
        }
    }

    drop(stream);
    let elapsed = start.elapsed().as_secs_f64();
    let rate = valid_frames as f64 / total_frames as f64 * 100.0;

    eprintln!("\n{:-<70}", "");
    eprintln!("Test complete ({:.1}s)", elapsed);
    eprintln!("  Total detections: {}", total_frames);
    eprintln!("  Valid frames:     {}", valid_frames);
    eprintln!("  Dropped:          {} ({:.1}%)", total_frames - valid_frames, 100.0 - rate);
    eprintln!("  Avg peak:         {:.4}", last_peak);
    eprintln!("  Avg CFO:          {:.4} rad/sym", last_cfo);
    Ok(())
}
