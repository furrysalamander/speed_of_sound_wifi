//! Stage 3 link training: discover the peer, measure control-channel headroom,
//! then negotiate the fastest data mode that still decodes with margin.
//!
//!   sosw-link-train --mode a --node-id 1 [devices]
//!   sosw-link-train --mode b --node-id 2 [devices]
//!   sosw-link-train --mode sim [--snr-db N] [--drop P]
//!
//! Protocol (stop-and-wait at the transport layer):
//!   A -> HELLO                 B -> HELLO_ACK
//!   A -> MODE(id)              B -> MODE_ACK, then B listens in mode id
//!   A -> PROBE (in mode id)    B -> REPORT(id, minSNR, ok)
//!   ... repeat from fastest to most robust until a mode reports ok with margin
//!   A -> DONE                  B -> DONE_ACK
//!
//! The `sim` mode runs A and B in threads over a noisy in-memory channel, so a
//! fixed noise floor makes fast modes fail and robust modes pass, exercising
//! the exact negotiation logic that runs over the air.

use anyhow::Result;
use clap::Parser;
use sosw_core::physical::fsk::{self, FskConfig, FskDemodulator};
use sosw_tap::xfer::{AudioTransport, Transport};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

const SR: u32 = 48_000;
const KIND_HELLO: u8 = 1;
const KIND_HELLO_ACK: u8 = 2;
const KIND_MODE: u8 = 3;
const KIND_MODE_ACK: u8 = 4;
const KIND_PROBE: u8 = 5;
const KIND_REPORT: u8 = 6;
const KIND_DONE: u8 = 7;
const KIND_DONE_ACK: u8 = 8;

/// Minimum per-symbol tone SNR to accept a mode (headroom).
const MIN_HEADROOM_DB: f32 = 6.0;

fn base_cfg() -> FskConfig {
    FskConfig {
        m: 2,
        symbol_samples: SR as usize * 20 / 1000,
        base_freq: 1000.0,
        amplitude: 0.8,
        preamble_symbols: 24,
        guard_samples: SR as usize * 40 / 1000,
        rs_nsym: 8,
        fec_data_block: 16,
        payload_size: 16,
        ..FskConfig::default()
    }
}

/// Candidate data modes, fastest first; the last is the robust control mode.
fn mode_table() -> Vec<(&'static str, FskConfig)> {
    let mk = |m: usize, sym_ms: u64| FskConfig {
        m,
        symbol_samples: SR as usize * sym_ms as usize / 1000,
        ..base_cfg()
    };
    vec![
        ("M8/5ms", mk(8, 5)),
        ("M4/5ms", mk(4, 5)),
        ("M2/5ms", mk(2, 5)),
        ("M8/10ms", mk(8, 10)),
        ("M4/10ms", mk(4, 10)),
        ("M2/10ms", mk(2, 10)),
        ("M4/20ms", mk(4, 20)),
        ("M2/20ms", mk(2, 20)),
    ]
}

fn control_index(modes: &[(&str, FskConfig)]) -> usize {
    modes.len() - 1
}

#[derive(Parser)]
#[command(name = "sosw-link-train", about = "Acoustic link training / rate negotiation")]
struct Args {
    /// 'a' initiates, 'b' responds, 'sim' runs both over a simulated channel.
    #[arg(long, default_value = "sim")]
    mode: String,
    #[arg(long, default_value_t = 1)]
    node_id: u8,
    #[arg(long)]
    tx_device: Option<String>,
    #[arg(long)]
    rx_device: Option<String>,
    #[arg(long, default_value_t = 30000)]
    timeout_ms: u64,
    #[arg(long, default_value_t = 4)]
    max_retries: usize,
    /// Simulated channel SNR in dB.
    #[arg(long, default_value_t = 12.0)]
    snr_db: f32,
    /// Simulated per-frame channel drop probability.
    #[arg(long, default_value_t = 0.0)]
    drop: f32,
}

fn snr_byte(snr: f32) -> u8 {
    snr.clamp(0.0, 255.0) as u8
}

fn role_a(t: &mut dyn Transport, modes: &[(&str, FskConfig)], timeout_ms: u64, retries: usize) -> Result<usize> {
    let ctrl = control_index(modes);
    t.set_config(modes[ctrl].1.clone());

    // 1. Discover the peer.
    let mut hello_ack = false;
    for _ in 0..=retries {
        t.send(&[KIND_HELLO]);
        if let Some(r) = t.recv(timeout_ms) {
            if r.first() == Some(&KIND_HELLO_ACK) {
                hello_ack = true;
                break;
            }
        }
    }
    if !hello_ack {
        anyhow::bail!("no HELLO_ACK from peer (control channel failed)");
    }
    let (ctrl_snr, _) = t.last_quality();
    println!("peer found; control-channel headroom {:.1} dB", ctrl_snr);

    // 2. Try modes fastest-first.
    let pattern: [u8; 8] = [0xA5, 0x5A, 0xC3, 0x3C, 0x0F, 0xF0, 0x11, 0x22];
    let mut chosen: Option<usize> = None;
    for id in 0..ctrl {
        t.set_config(modes[ctrl].1.clone());
        let mut mode_acked = false;
        for _ in 0..=retries {
            t.send(&[KIND_MODE, id as u8]);
            if let Some(r) = t.recv(timeout_ms) {
                if r.first() == Some(&KIND_MODE_ACK) {
                    mode_acked = true;
                    break;
                }
            }
        }
        if !mode_acked {
            println!("  {} : no MODE_ACK, skipping", modes[id].0);
            continue;
        }
        // Probe in the candidate mode.
        t.set_config(modes[id].1.clone());
        let mut probe = vec![KIND_PROBE];
        probe.extend_from_slice(&pattern);
        let mut report: Option<(f32, bool)> = None;
        for _ in 0..=retries {
            t.send(&probe);
            if let Some(r) = t.recv(timeout_ms) {
                if r.first() == Some(&KIND_REPORT) && r.len() >= 4 {
                    let snr = r[2] as f32;
                    let ok = r[3] == 1;
                    report = Some((snr, ok));
                    break;
                }
            }
        }
        match report {
            Some((snr, true)) if snr >= MIN_HEADROOM_DB => {
                println!("  {} : OK, headroom {:.0} dB  <-- selected", modes[id].0, snr);
                chosen = Some(id);
                break;
            }
            Some((snr, ok)) => {
                println!("  {} : ok={} headroom {:.0} dB, stepping down", modes[id].0, ok, snr);
            }
            None => println!("  {} : no report, stepping down", modes[id].0),
        }
        // Return to control for the next negotiation round.
        t.set_config(modes[ctrl].1.clone());
    }

    // 3. Finish.
    t.set_config(modes[ctrl].1.clone());
    for _ in 0..=retries {
        t.send(&[KIND_DONE]);
        if let Some(r) = t.recv(timeout_ms) {
            if r.first() == Some(&KIND_DONE_ACK) {
                break;
            }
        }
    }
    match chosen {
        Some(id) => {
            println!(
                "link trained: mode {} (control {:.1} dB headroom)",
                modes[id].0, ctrl_snr
            );
            Ok(id)
        }
        None => {
            println!("link trained: control mode only ({} at {:.1} dB)", modes[ctrl].0, ctrl_snr);
            Ok(ctrl)
        }
    }
}

fn role_b(t: &mut dyn Transport, modes: &[(&str, FskConfig)], timeout_ms: u64, retries: usize) -> Result<()> {
    let ctrl = control_index(modes);
    t.set_config(modes[ctrl].1.clone());
    let mut current_probe_mode = ctrl;
    let mut idle = 0usize;
    loop {
        let frame = match t.recv(timeout_ms) {
            Some(f) => f,
            None => {
                idle += 1;
                if idle > retries + 2 {
                    anyhow::bail!("training timed out waiting for peer");
                }
                continue;
            }
        };
        idle = 0;
        match frame.first().copied().unwrap_or(0) {
            KIND_HELLO => {
                t.send(&[KIND_HELLO_ACK]);
            }
            KIND_MODE if frame.len() >= 2 => {
                let id = frame[1] as usize;
                t.send(&[KIND_MODE_ACK]);
                if id < modes.len() {
                    current_probe_mode = id;
                    t.set_config(modes[id].1.clone());
                    println!("  switching to mode {}", modes[id].0);
                }
            }
            KIND_PROBE => {
                let (snr, fec_ok) = t.last_quality();
                t.send(&[KIND_REPORT, current_probe_mode as u8, snr_byte(snr), fec_ok as u8]);
                // Back to control for the next negotiation round.
                t.set_config(modes[ctrl].1.clone());
                current_probe_mode = ctrl;
            }
            KIND_DONE => {
                t.send(&[KIND_DONE_ACK]);
                println!("training complete");
                return Ok(());
            }
            _ => {}
        }
    }
}

// --- simulated channel (two threads) ---------------------------------------

struct ThreadTransport {
    tx: Sender<Vec<f32>>,
    rx: Receiver<Vec<f32>>,
    cfg: FskConfig,
    noise_db: f32,
    drop: f32,
    rng: u32,
    last: (f32, bool),
}

impl ThreadTransport {
    fn new(tx: Sender<Vec<f32>>, rx: Receiver<Vec<f32>>, cfg: FskConfig, noise_db: f32, drop: f32, seed: u32) -> Self {
        Self { tx, rx, cfg, noise_db, drop, rng: seed | 1, last: (0.0, true) }
    }
    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng & 0xFFFFFF) as f32 / 0xFFFFFF as f32
    }
}

impl Transport for ThreadTransport {
    fn send(&mut self, payload: &[u8]) {
        if self.rand() < self.drop {
            return;
        }
        let mut audio = self.cfg.encode_payload(payload);
        let sig = audio.iter().map(|s| s * s).sum::<f32>() / audio.len().max(1) as f32;
        let noise = (sig / 10f32.powf(self.noise_db / 10.0)).sqrt();
        for s in audio.iter_mut() {
            *s += (self.rand() * 2.0 - 1.0) * noise;
        }
        let _ = self.tx.send(audio);
    }

    fn recv(&mut self, timeout_ms: u64) -> Option<Vec<u8>> {
        let audio = self.rx.recv_timeout(Duration::from_millis(timeout_ms)).ok()?;
        let dem = FskDemodulator::new(self.cfg.clone());
        for fr in dem.decode_capture(&audio) {
            self.last = (fr.min_snr_db, fr.fec_ok);
            if let Some(p) = fsk::unwrap_frame(&fr.bytes) {
                return Some(p);
            }
        }
        None
    }

    fn set_config(&mut self, cfg: FskConfig) {
        self.cfg = cfg;
    }

    fn last_quality(&self) -> (f32, bool) {
        self.last
    }
}

fn run_sim(a: &Args) -> Result<()> {
    let modes = mode_table();
    let ctrl = control_index(&modes);
    let (a_tx, b_rx) = channel::<Vec<f32>>();
    let (b_tx, a_rx) = channel::<Vec<f32>>();
    let mut ta = ThreadTransport::new(a_tx, a_rx, modes[ctrl].1.clone(), a.snr_db, a.drop, 0xA1);
    let mut tb = ThreadTransport::new(b_tx, b_rx, modes[ctrl].1.clone(), a.snr_db, a.drop, 0xB2);
    let modes_for_b = mode_table();
    let timeout = a.timeout_ms.min(3000); // sim is fast
    let retries = a.max_retries;
    let b = std::thread::spawn(move || {
        let m: Vec<(&str, FskConfig)> = modes_for_b;
        role_b(&mut tb, &m, timeout, retries)
    });
    let chosen = role_a(&mut ta, &modes, timeout, a.max_retries)?;
    b.join().unwrap()?;
    println!("sim negotiated mode id {} ({}) at snr {} dB, drop {}", chosen, modes[chosen].0, a.snr_db, a.drop);
    Ok(())
}

fn run_acoustic(a: &Args) -> Result<()> {
    let modes = mode_table();
    let ctrl = control_index(&modes);
    let mut t = AudioTransport::new(a.tx_device.as_deref(), a.rx_device.as_deref(), modes[ctrl].1.clone())?;
    let id = match a.mode.as_str() {
        "a" => role_a(&mut t, &modes, a.timeout_ms, a.max_retries)?,
        "b" => {
            role_b(&mut t, &modes, a.timeout_ms, a.max_retries)?;
            ctrl
        }
        other => anyhow::bail!("unknown mode '{}'", other),
    };
    let _ = id;
    Ok(())
}

fn main() -> Result<()> {
    let a = Args::parse();
    match a.mode.as_str() {
        "sim" => run_sim(&a),
        "a" | "b" => run_acoustic(&a),
        other => anyhow::bail!("unknown mode '{}'", other),
    }
}

// Keep Instant referenced for future timing diagnostics.
#[allow(dead_code)]
fn _unused() -> Instant {
    Instant::now()
}
