//! Interactive link-training handshake over the DTMF control channel.
//!
//! ```text
//!   sosw-link --role a --node-id 1   (beacon/initiator)
//!   sosw-link --role b --node-id 2   (responder)
//! ```
//!
//! A sends HELLO; B replies HELLO_ACK; A sends TRAIN(n); B replies TRAIN_ACK.
//! Each side mutes its own receive path while transmitting so the local
//! speaker does not echo into the local microphone.

use anyhow::Result;
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::physical::dtmf::{self, DtmfConfig};
use sosw_tap::link::{self, Message};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long after our own playback ends we keep the local mic muted, to ride
/// out the acoustic echo of our own speaker through the room.
const ECHO_TAIL_MS: u64 = 1200;

/// After a peer transmits it waits this long before replying, so we have time
/// to finish our echo tail and start listening.
const TURN_GAP: Duration = Duration::from_millis(1500);

#[derive(Parser)]
#[command(name = "sosw-link", about = "DTMF link-training handshake")]
struct Args {
    /// 'a' initiates (HELLO), 'b' responds (HELLO_ACK)
    #[arg(long, default_value = "a")]
    role: String,
    #[arg(long, default_value_t = 1)]
    node_id: u8,
    #[arg(long)]
    tx_device: Option<String>,
    #[arg(long)]
    rx_device: Option<String>,
    /// Number of training frames to request
    #[arg(long, default_value_t = 8)]
    train: u8,
    /// Overall handshake timeout in seconds
    #[arg(long, default_value_t = 60)]
    timeout: u64,
    /// Dump everything this node receives to a raw f32 file for diagnosis
    #[arg(long)]
    dump_rx: Option<String>,
}

struct Audio {
    tx: Arc<Mutex<Vec<f32>>>,
    rx: Arc<Mutex<Vec<f32>>>,
    tx_pos: Arc<AtomicUsize>,
    muting: Arc<AtomicBool>,
    node_id: u8,
    dump_rx: Option<String>,
    t0: Instant,
    symbols: Arc<Mutex<Vec<u8>>>,
    _out: cpal::Stream,
    _in: cpal::Stream,
}

/// Wall-clock time as `HH:MM:SS.mmm`, for cross-machine log alignment.
fn now_ms(_t0: Instant) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let ms = now.subsec_millis();
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}.{:03}", h, m, s, ms)
}

impl Audio {
    fn new(tx_dev: Option<&str>, rx_dev: Option<&str>, node_id: u8, dump_rx: Option<String>) -> Result<Self> {
        let host = cpal::default_host();
        let out_dev = pick(&host, tx_dev, true)?;
        let in_dev = pick(&host, rx_dev, false)?;
        let out_cfg = out_dev.default_output_config()?.config();
        let in_cfg = in_dev.default_input_config()?.config();
        let out_ch = out_cfg.channels as usize;
        let in_ch = in_cfg.channels as usize;
        eprintln!(
            "TX {} ({} Hz, {} ch) | RX {} ({} Hz, {} ch)",
            out_dev.id().map(|i| format!("{}", i)).unwrap_or_default(), out_cfg.sample_rate, out_ch,
            in_dev.id().map(|i| format!("{}", i)).unwrap_or_default(), in_cfg.sample_rate, in_ch,
        );

        let tx = Arc::new(Mutex::new(Vec::<f32>::new()));
        let rx = Arc::new(Mutex::new(Vec::<f32>::new()));
        let tx_pos = Arc::new(AtomicUsize::new(0));
        let muting = Arc::new(AtomicBool::new(false));

        let t = tx.clone();
        let p = tx_pos.clone();
        let out = out_dev.build_output_stream::<f32, _, _>(
            out_cfg,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let buf = t.lock().unwrap();
                let base = p.load(Ordering::Relaxed);
                let frames = data.len() / out_ch;
                for i in 0..frames {
                    let s = buf.get(base + i).copied().unwrap_or(0.0);
                    for c in 0..out_ch {
                        data[i * out_ch + c] = s;
                    }
                }
                p.fetch_add(frames, Ordering::Relaxed);
            },
            |e| eprintln!("out error: {}", e),
            None,
        )?;

        let r = rx.clone();
        let m = muting.clone();
        let in_stream = in_dev.build_input_stream::<f32, _, _>(
            in_cfg,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if m.load(Ordering::Relaxed) {
                    return;
                }
                if let Ok(mut v) = r.lock() {
                    if in_ch > 1 {
                        for f in data.chunks(in_ch) {
                            v.push(f[0]);
                        }
                    } else {
                        v.extend_from_slice(data);
                    }
                }
            },
            |e| eprintln!("in error: {}", e),
            None,
        )?;
        out.play()?;
        in_stream.play()?;
        Ok(Self { tx, rx, tx_pos, muting, node_id, dump_rx, t0: Instant::now(), symbols: Arc::new(Mutex::new(Vec::new())), _out: out, _in: in_stream })
    }

    fn transmit(&self, samples: &[f32]) {
        self.muting.store(true, Ordering::Relaxed);
        {
            let mut t = self.tx.lock().unwrap();
            t.clear();
            t.extend_from_slice(samples);
        }
        self.tx_pos.store(0, Ordering::Relaxed);
        let dur_ms = (samples.len() as f64 / 48_000.0 * 1000.0) as u64;
        eprintln!("[{}] TX start: {} samples, {:.0} ms audio", now_ms(self.t0), samples.len(), dur_ms as f64);
        // Keep the local mic muted through playback AND the acoustic echo tail
        // (speaker -> room -> local mic, plus input latency). While muted the
        // RX callback drops samples, so the buffer is clean the moment we
        // unmute; do NOT clear it afterwards or we may erase the peer's reply.
        std::thread::sleep(Duration::from_millis(dur_ms + ECHO_TAIL_MS));
        self.muting.store(false, Ordering::Relaxed);
        eprintln!("[{}] TX end (mic live)", now_ms(self.t0));
    }

    /// Wait for a message of the given `kind`. Collects all RX audio for the
    /// whole listen window, then decodes the complete capture once — the same
    /// approach as the proven `rx_listen` tool. Decoding a growing buffer in a
    /// sliding window only ever sees prefixes and never assembles a frame.
    fn wait_for(&self, kind: u8, dur: Duration, cfg: &DtmfConfig) -> Option<Message> {
        let start = Instant::now();
        let mut collected: Vec<f32> = Vec::new();
        let mut peak = 0.0f32;
        // Drain whatever was buffered before this call (mostly our own tail).
        self.rx.lock().unwrap().clear();
        loop {
            {
                let mut r = self.rx.lock().unwrap();
                collected.extend(r.drain(..));
            }
            if !collected.is_empty() {
                let p = collected.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
                peak = peak.max(p);
            }
            // Try to decode the full capture so far; a complete message will
            // appear once enough audio has accumulated.
            if collected.len() >= cfg.symbol_samples {
                let symbols = dtmf::decode(&collected, cfg);
                for (msg, _) in link::decode_all(&symbols) {
                    if msg.kind == kind && msg.node_id != self.node_id {
                        eprintln!(
                            "[{}] RX kind={} node={} param={} (peak {:.3})",
                            now_ms(self.t0), msg.kind, msg.node_id, msg.param, peak
                        );
                        return Some(msg);
                    }
                }
            }
            if start.elapsed() > dur {
                eprintln!(
                    "[{}] RX timeout after {:.0}ms ({} samples, peak {:.3})",
                    now_ms(self.t0), dur.as_millis(), collected.len(), peak
                );
                if let Some(path) = &self.dump_rx {
                    let mut bytes = Vec::with_capacity(collected.len() * 4);
                    for s in &collected {
                        bytes.extend_from_slice(&s.to_le_bytes());
                    }
                    let _ = std::fs::write(path, &bytes);
                }
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
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

fn main() -> Result<()> {
    let args = Args::parse();
    let cfg = DtmfConfig::default();
    let role = args.role.to_lowercase();
    let deadline = Instant::now() + Duration::from_secs(args.timeout);

    let audio = Audio::new(args.tx_device.as_deref(), args.rx_device.as_deref(), args.node_id, args.dump_rx.clone())?;
    eprintln!("Node {} starting as role {} (DTMF {:?})", args.node_id, role, cfg);

    let mut peer: Option<Message> = None;

    if role == "a" {
        let hello = Message::hello(args.node_id, args.train, "sosw");
        let mut hello_audio = link::encode_message(&hello, &cfg);
        hello_audio.extend(std::iter::repeat(0.0f32).take(cfg.symbol_samples));

        let mut hello_ack: Option<Message> = None;
        let mut attempt = 0;
        while hello_ack.is_none() && Instant::now() < deadline {
            attempt += 1;
            eprintln!("[{}] [A] TX HELLO (attempt {})", now_ms(audio.t0), attempt);
            audio.transmit(&hello_audio);
            std::thread::sleep(TURN_GAP);
            match audio.wait_for(link::MSG_HELLO_ACK, Duration::from_secs(10), &cfg) {
                Some(msg) => {
                    eprintln!("[{}] [A] RX HELLO_ACK from node {} (trains={})", now_ms(audio.t0), msg.node_id, msg.param);
                    hello_ack = Some(msg);
                }
                None => eprintln!("[{}] [A] no HELLO_ACK", now_ms(audio.t0)),
            }
        }
        let peer = hello_ack.ok_or_else(|| anyhow::anyhow!("handshake timeout waiting for HELLO_ACK"))?;

        let train = Message::train(args.node_id, args.train);
        let mut train_audio = link::encode_message(&train, &cfg);
        train_audio.extend(std::iter::repeat(0.0f32).take(cfg.symbol_samples));
        let mut train_ack: Option<Message> = None;
        attempt = 0;
        while train_ack.is_none() && Instant::now() < deadline {
            attempt += 1;
            eprintln!("[{}] [A] TX TRAIN({}) (attempt {})", now_ms(audio.t0), args.train, attempt);
            audio.transmit(&train_audio);
            std::thread::sleep(TURN_GAP);
            match audio.wait_for(link::MSG_TRAIN_ACK, Duration::from_secs(10), &cfg) {
                Some(m) => {
                    eprintln!("[{}] [A] RX TRAIN_ACK from node {} (received {})", now_ms(audio.t0), m.node_id, m.param);
                    train_ack = Some(m);
                }
                None => eprintln!("[{}] [A] no TRAIN_ACK", now_ms(audio.t0)),
            }
        }
        if train_ack.is_some() {
            eprintln!("[{}] [A] LINK ESTABLISHED with node {}", now_ms(audio.t0), peer.node_id);
        } else {
            eprintln!("[{}] [A] HELLO ok but TRAIN unacknowledged", now_ms(audio.t0));
        }
    } else {
        eprintln!("[{}] [B] waiting for HELLO", now_ms(audio.t0));
        // Strict alternation: B only transmits in direct response to a decoded
        // HELLO, and only after A is done talking. Between responses B listens.
        let mut linked = false;
        while Instant::now() < deadline {
            let hello = match audio.wait_for(link::MSG_HELLO, Duration::from_secs(8), &cfg) {
                Some(h) => h,
                None => continue,
            };
            eprintln!("[{}] [B] RX HELLO from node {} (trains={})", now_ms(audio.t0), hello.node_id, hello.param);
            std::thread::sleep(TURN_GAP);
            let ack = Message::hello_ack(args.node_id, hello.param, "sosw");
            let mut ack_audio = link::encode_message(&ack, &cfg);
            ack_audio.extend(std::iter::repeat(0.0f32).take(cfg.symbol_samples));
            eprintln!("[{}] [B] TX HELLO_ACK", now_ms(audio.t0));
            audio.transmit(&ack_audio);

            // Now wait for TRAIN.
            let train = match audio.wait_for(link::MSG_TRAIN, Duration::from_secs(10), &cfg) {
                Some(t) => t,
                None => {
                    eprintln!("[{}] [B] no TRAIN; will re-ACK if HELLO repeats", now_ms(audio.t0));
                    continue;
                }
            };
            eprintln!("[{}] [B] RX TRAIN({}) from node {}", now_ms(audio.t0), train.param, train.node_id);
            std::thread::sleep(TURN_GAP);
            let ta = Message::train_ack(args.node_id, train.param);
            let mut ta_audio = link::encode_message(&ta, &cfg);
            ta_audio.extend(std::iter::repeat(0.0f32).take(cfg.symbol_samples));
            eprintln!("[{}] [B] TX TRAIN_ACK", now_ms(audio.t0));
            audio.transmit(&ta_audio);
            eprintln!("[{}] [B] LINK ESTABLISHED with node {}", now_ms(audio.t0), hello.node_id);
            linked = true;
            break;
        }
        if !linked {
            eprintln!("[{}] [B] handshake timeout", now_ms(audio.t0));
        }
    }
    Ok(())
}
