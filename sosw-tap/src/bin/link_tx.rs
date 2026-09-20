use anyhow::Result;
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::physical::dtmf::DtmfConfig;
use sosw_tap::link::{self, Message};

#[derive(Parser)]
#[command(name = "link-tx", about = "Transmit a single sosw link message")]
struct Args {
    kind: u8,
    #[arg(long, default_value_t = 1)]
    node_id: u8,
    #[arg(long, default_value_t = 8)]
    param: u8,
    #[arg(long)]
    device: Option<String>,
    #[arg(short, long, default_value_t = 1)]
    repeat: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let cfg = DtmfConfig::default();
    let msg = match args.kind {
        0 => Message::hello(args.node_id, args.param, "sosw"),
        1 => Message::hello_ack(args.node_id, args.param, "sosw"),
        2 => Message::train(args.node_id, args.param),
        3 => Message::train_ack(args.node_id, args.param),
        _ => anyhow::bail!("kind must be 0-3"),
    };
    let mut audio = Vec::new();
    for _ in 0..args.repeat.max(1) {
        audio.extend_from_slice(&link::encode_message(&msg, &cfg));
        audio.extend(std::iter::repeat(0.0f32).take(cfg.symbol_samples));
    }
    eprintln!("TX kind={} node={} ({} samples)", args.kind, args.node_id, audio.len());

    let host = cpal::default_host();
    let device = match &args.device {
        Some(n) => {
            let lower = n.to_lowercase();
            host.output_devices()?
                .find(|d| d.id().map(|id| format!("{}", id).to_lowercase().contains(&lower)).unwrap_or(false))
                .ok_or_else(|| anyhow::anyhow!("no output '{}'", n))?
        }
        None => host.default_output_device().ok_or_else(|| anyhow::anyhow!("no default output"))?,
    };
    let out = device.default_output_config()?.config();
    let ch = out.channels as usize;
    let audio = std::sync::Arc::new(audio);
    let mut off = 0usize;
    let a = audio.clone();
    let stream = device.build_output_stream::<f32, _, _>(out, move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        let frames = data.len() / ch;
        for i in 0..frames {
            let s = a.get(off + i).copied().unwrap_or(0.0);
            for c in 0..ch { data[i * ch + c] = s; }
        }
        off += frames;
    }, |e| eprintln!("err {}", e), None)?;
    stream.play()?;
    let ms = (audio.len() as f64 / 48000.0 * 1000.0) as u64;
    std::thread::sleep(std::time::Duration::from_millis(ms + 300));
    eprintln!("done");
    Ok(())
}
