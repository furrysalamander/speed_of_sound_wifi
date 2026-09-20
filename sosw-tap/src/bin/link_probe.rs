//! Over-air probe for tuning the DTMF control channel.
//!
//! TX sends `count` frames, each a TRAIN message whose `param` is the frame
//! index. RX records for `duration` seconds and reports how many distinct
//! frame indices it recovered, which is the frame success rate for the chosen
//! symbol/gap/repeat settings.

use anyhow::Result;
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::physical::dtmf::{self, DtmfConfig};
use sosw_tap::link::{self, Message};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Wall-clock `HH:MM:SS.mmm`, for cross-machine timing.
fn wall() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();
    format!("{:02}:{:02}:{:02}.{:03}", (secs / 3600) % 24, (secs / 60) % 60, secs % 60, now.subsec_millis())
}

#[derive(Parser)]
#[command(name = "link-probe", about = "Tune the DTMF control channel over the air")]
struct Args {
    /// 'tx' or 'rx'
    mode: String,
    #[arg(long, default_value_t = 40)]
    symbol_ms: u32,
    #[arg(long, default_value_t = 20)]
    gap_ms: u32,
    #[arg(long, default_value_t = 3)]
    repeat: usize,
    /// Number of frames to transmit (TX) / expected (RX)
    #[arg(long, default_value_t = 20)]
    count: usize,
    /// Seconds to record (RX)
    #[arg(long, default_value_t = 10)]
    duration: u64,
    #[arg(long)]
    device: Option<String>,
    #[arg(long, default_value_t = 1)]
    node_id: u8,
    #[arg(long)]
    dump: Option<String>,
}

fn cfg_from(args: &Args) -> DtmfConfig {
    DtmfConfig {
        sample_rate: 48000,
        symbol_samples: 48 * args.symbol_ms as usize,
        gap_samples: 48 * args.gap_ms as usize,
        amplitude: 0.4,
    }
}

fn pick(host: &cpal::Host, name: Option<&str>, output: bool) -> Result<cpal::Device> {
    match name {
        Some(n) => {
            let lower = n.to_lowercase();
            let dev = if output { host.output_devices()? } else { host.input_devices()? }
                .find(|d| d.id().map(|id| format!("{}", id).to_lowercase().contains(&lower)).unwrap_or(false))
                .ok_or_else(|| anyhow::anyhow!("no device matching '{}'", n))?;
            Ok(dev)
        }
        None => if output {
            host.default_output_device().ok_or_else(|| anyhow::anyhow!("no default output"))
        } else {
            host.default_input_device().ok_or_else(|| anyhow::anyhow!("no default input"))
        },
    }
}

fn transmit(samples: &[f32], device: Option<&str>) -> Result<()> {
    let host = cpal::default_host();
    let dev = pick(&host, device, true)?;
    let out = dev.default_output_config()?.config();
    let ch = out.channels as usize;
    let audio = Arc::new(samples.to_vec());
    let mut off = 0usize;
    let a = audio.clone();
    let stream = dev.build_output_stream::<f32, _, _>(out, move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        let frames = data.len() / ch;
        for i in 0..frames {
            let s = a.get(off + i).copied().unwrap_or(0.0);
            for c in 0..ch { data[i * ch + c] = s; }
        }
        off += frames;
    }, |e| eprintln!("out error: {}", e), None)?;
    stream.play()?;
    let ms = (samples.len() as f64 / 48_000.0 * 1000.0) as u64;
    std::thread::sleep(Duration::from_millis(ms + 300));
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let cfg = cfg_from(&args);
    if args.mode == "tx" {
        let mut audio = Vec::new();
        // Two symbols of silence between frames so each frame is a distinct
        // active region (also prevents one frame's tail bleeding into the next).
        let sep = 2 * (cfg.symbol_samples + cfg.gap_samples);
        for i in 0..args.count {
            let msg = Message::train(args.node_id, i as u8);
            audio.extend_from_slice(&link::encode_message_repeat(&msg, &cfg, args.repeat));
            audio.extend(std::iter::repeat(0.0f32).take(sep));
        }
        let ms = audio.len() as f64 / 48.0;
        eprintln!(
            "TX {} frames, {}/{} ms, repeat={}, total {:.2}s, start {}\n",
            args.count, args.symbol_ms, args.gap_ms, args.repeat, ms / 1000.0, wall()
        );
        transmit(&audio, args.device.as_deref())?;
        return Ok(());
    }

    // RX
    let host = cpal::default_host();
    let dev = pick(&host, args.device.as_deref(), false)?;
    let in_cfg = dev.default_input_config()?.config();
    let ch = in_cfg.channels as usize;
    let collected = Arc::new(Mutex::new(Vec::<f32>::new()));
    let c = collected.clone();
    let stream = dev.build_input_stream::<f32, _, _>(in_cfg, move |data: &[f32], _: &cpal::InputCallbackInfo| {
        if let Ok(mut v) = c.lock() {
            if ch > 1 {
                for f in data.chunks(ch) { v.push(f[0]); }
            } else {
                v.extend_from_slice(data);
            }
        }
    }, |e| eprintln!("in error: {}", e), None)?;
    stream.play()?;
    let t_start = wall();
    let t0 = Instant::now();
    std::thread::sleep(Duration::from_secs(args.duration));
    let audio = collected.lock().unwrap().clone();
    let elapsed = t0.elapsed().as_secs_f64();

    if let Some(path) = &args.dump {
        let mut bytes = Vec::with_capacity(audio.len() * 4);
        for s in &audio { bytes.extend_from_slice(&s.to_le_bytes()); }
        let _ = std::fs::write(path, &bytes);
    }

    let t_dec = Instant::now();
    let symbols = dtmf::decode(&audio, &cfg);
    let dec_ms = t_dec.elapsed().as_millis();

    // First onset wall time, so the peer can measure true acoustic latency
    // against its own TX wall time (clocks are NTP-synced).
    let w = (cfg.sample_rate as usize / 100).max(1);
    let env: Vec<f32> = audio.windows(w).step_by(w).map(|x| x.iter().map(|v| v * v).sum()).collect();
    let mx = env.iter().cloned().fold(0.0f32, f32::max).max(1e-12);
    let onset_off = env.iter().position(|&e| e > mx * 0.05).map(|i| i as f64 / cfg.sample_rate as f64);
    let onset_wall = onset_off.map(|off| {
        let h = t_start[..2].parse::<u64>().unwrap();
        let m = t_start[3..5].parse::<u64>().unwrap();
        let s = t_start[6..8].parse::<u64>().unwrap();
        let ms = t_start[9..12].parse::<u64>().unwrap();
        let mut tot = ((h * 60 + m) * 60 + s) * 1000 + ms + (off * 1000.0) as u64;
        tot %= 86_400_000;
        format!("{:02}:{:02}:{:02}.{:03}", tot / 3_600_000, (tot / 60_000) % 60, (tot / 1000) % 60, tot % 1000)
    });

    let msgs = link::decode_all(&symbols);
    let mut params: BTreeSet<u8> = BTreeSet::new();
    for (m, _) in &msgs {
        if m.kind == link::MSG_TRAIN && m.node_id == args.node_id {
            params.insert(m.param);
        }
    }
    let bad = msgs.len() - params.len();
    let frame_ms = 7.0 * args.repeat as f64 * (args.symbol_ms + args.gap_ms) as f64;
    eprintln!(
        "RX {}/{} frames (unique) in {:.1}s | {} symbols ({}ms decode) | frame={:.0}ms rate={:.1} fps | spurious={}",
        params.len(), args.count, elapsed, symbols.len(), dec_ms, frame_ms,
        1000.0 / frame_ms, bad,
    );
    eprintln!("params: {:?}", params);
    eprintln!("RX stream started {} | first onset {}", t_start, onset_wall.as_deref().unwrap_or("none"));
    Ok(())
}
