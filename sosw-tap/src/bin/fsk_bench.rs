//! Stage 1/2 FSK link benchmark.
//!
//! Modes:
//!   software  generate frames, add AWGN, demodulate in-process (baseline)
//!   self      play frames through the speaker and decode the acoustic echo
//!   tx        play frames and exit (for cross-machine RX on the peer)
//!   rx        record for --rx-ms and decode (for cross-machine TX on the peer)
//!   sweep     software rate ladder: M x symbol rate -> PER and headroom
//!
//! Reports frame error rate, CRC-valid payloads, goodput, and the worst-case
//! per-symbol tone SNR (the headroom metric).

use anyhow::Result;
use clap::Parser;
use sosw_core::physical::fsk::{self, FskConfig, FskDecoded, FskDemodulator};
use sosw_tap::audio::{self, DuplexAudio};
use std::time::Duration;

const SR: u32 = 48_000;

#[derive(Parser)]
#[command(name = "fsk-bench", about = "FSK link benchmark (Stages 1-2)")]
struct Args {
    #[arg(long, default_value = "software")]
    mode: String,
    #[arg(long, default_value_t = 4)]
    m: usize,
    /// Symbol duration in ms.
    #[arg(long, default_value_t = 20)]
    symbol_ms: u64,
    #[arg(long, default_value_t = 1000.0)]
    base_freq: f32,
    #[arg(long, default_value_t = 0.25)]
    amplitude: f32,
    #[arg(long, default_value_t = false)]
    differential: bool,
    #[arg(long, default_value_t = 5)]
    frames: usize,
    #[arg(long, default_value_t = 16)]
    payload: usize,
    #[arg(long, default_value_t = 24)]
    preamble: usize,
    /// Guard silence after each frame, in ms.
    #[arg(long, default_value_t = 120)]
    guard_ms: u64,
    /// AWGN SNR in dB for software mode (omit for clean).
    #[arg(long)]
    snr_db: Option<f32>,
    #[arg(long)]
    tx_device: Option<String>,
    #[arg(long)]
    rx_device: Option<String>,
    /// Record duration for rx mode.
    #[arg(long, default_value_t = 20000)]
    rx_ms: u64,
    /// Extra capture time to cover acoustic latency (self/rx).
    #[arg(long, default_value_t = 6000)]
    latency_ms: u64,
    #[arg(long)]
    dump: Option<String>,
    #[arg(long)]
    load: Option<String>,
}

fn make_cfg(a: &Args) -> FskConfig {
    FskConfig {
        m: a.m,
        symbol_samples: (a.symbol_ms as usize * SR as usize) / 1000,
        base_freq: a.base_freq,
        amplitude: a.amplitude,
        preamble_symbols: a.preamble,
        differential: a.differential,
        guard_samples: (a.guard_ms as usize * SR as usize) / 1000,
        ..FskConfig::default()
    }
}

fn payloads(a: &Args) -> Vec<Vec<u8>> {
    (0..a.frames)
        .map(|i| {
            (0..a.payload)
                .map(|j| ((i * 131 + j * 17 + 7) % 256) as u8)
                .collect()
        })
        .collect()
}

fn build_script(cfg: &FskConfig, payloads: &[Vec<u8>]) -> Vec<f32> {
    let mut out = Vec::new();
    for p in payloads {
        out.extend(cfg.encode_frame(&fsk::wrap_frame(p)));
    }
    out
}

#[derive(Default, Clone)]
struct Metrics {
    sent: usize,
    decoded: usize,
    valid: usize,
    min_snr_db: f32,
    mean_snr_db: f32,
    min_conf: f32,
    decoded_frames: Vec<FskDecoded>,
}

impl Metrics {
    fn per(&self) -> f32 {
        if self.sent == 0 {
            0.0
        } else {
            (self.sent - self.valid) as f32 / self.sent as f32
        }
    }
}

fn evaluate(a: &Args, cfg: &FskConfig, audio: &[f32], payloads: &[Vec<u8>]) -> Metrics {
    let dem = FskDemodulator::new(cfg.clone());
    let decoded = dem.decode_capture(audio);
    let mut m = Metrics {
        sent: payloads.len(),
        min_snr_db: f32::INFINITY,
        min_conf: f32::INFINITY,
        ..Default::default()
    };
    let mut snr_sum = 0.0;
    for d in &decoded {
        m.decoded += 1;
        m.min_snr_db = m.min_snr_db.min(d.min_snr_db);
        m.min_conf = m.min_conf.min(d.mean_confidence);
        snr_sum += d.mean_snr_db;
        if let Some(p) = fsk::unwrap_frame(&d.bytes) {
            if payloads.iter().any(|x| x == &p) {
                m.valid += 1;
            }
        }
    }
    m.mean_snr_db = if m.decoded > 0 {
        snr_sum / m.decoded as f32
    } else {
        0.0
    };
    if !m.min_snr_db.is_finite() {
        m.min_snr_db = 0.0;
    }
    if !m.min_conf.is_finite() {
        m.min_conf = 0.0;
    }
    m.decoded_frames = decoded;
    m
}

fn add_noise(audio: &mut [f32], snr_db: f32) {
    let sig = audio.iter().map(|s| s * s).sum::<f32>() / audio.len().max(1) as f32;
    let noise = (sig / 10f32.powf(snr_db / 10.0)).sqrt();
    let mut rng = 0x9E37_79B9u32;
    for s in audio.iter_mut() {
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        let n = ((rng >> 8) as f32 / 16_777_216.0) * 2.0 - 1.0;
        *s += n * noise;
    }
}

fn report(a: &Args, cfg: &FskConfig, m: &Metrics, _rec_len: usize) {
    // Per-frame goodput: payload bits / (air time of one frame incl. guard).
    let payload_syms = (a.payload + 6) * 8 / cfg.bits_per_symbol().max(1) + 1;
    let frame_samples =
        (cfg.preamble_symbols + payload_syms) * cfg.symbol_samples + cfg.guard_samples;
    let frame_s = frame_samples as f32 / SR as f32;
    let goodput = if frame_s > 0.0 {
        (a.payload * 8) as f32 / frame_s
    } else {
        0.0
    };
    let eff = goodput * (1.0 - m.per());
    println!(
        "M={} sym={}ms raw={:.0}bps frames={} decoded={} valid={} PER={:.1}% minSNR={:.1}dB meanSNR={:.1}dB minConf={:.2} goodput={:.0}bps eff={:.0}bps",
        a.m,
        a.symbol_ms,
        cfg.raw_bps(),
        m.sent,
        m.decoded,
        m.valid,
        m.per() * 100.0,
        m.min_snr_db,
        m.mean_snr_db,
        m.min_conf,
        goodput,
        eff,
    );
}

fn main() -> Result<()> {
    let a = Args::parse();
    let cfg = make_cfg(&a);

    if a.mode == "sweep" {
        return sweep(&a);
    }

    let ps = payloads(&a);
    let script = build_script(&cfg, &ps);
    let mut audio = script.clone();

    match a.mode.as_str() {
        "software" => {
            if let Some(snr) = a.snr_db {
                add_noise(&mut audio, snr);
            }
            let m = evaluate(&a, &cfg, &audio, &ps);
            report(&a, &cfg, &m, audio.len());
        }
        "tx" => {
            let dev = DuplexAudio::new(a.tx_device.as_deref(), a.rx_device.as_deref())?;
            eprintln!("playing {} frames ({:.1} s)", ps.len(), script.len() as f32 / SR as f32);
            dev.play_blocking(&script, Duration::from_millis(200));
        }
        "rx" | "self" => {
            let rec = if let Some(path) = &a.load {
                eprintln!("loading {}", path);
                audio::load_capture(path)?
            } else {
                let dev = DuplexAudio::new(a.tx_device.as_deref(), a.rx_device.as_deref())?;
                dev.clear_rx();
                if a.mode == "self" {
                    dev.play_now(&script);
                }
                let total = if a.mode == "self" {
                    script.len() as f32 / SR as f32 * 1000.0 + a.latency_ms as f32 + 1000.0
                } else {
                    a.rx_ms as f32
                };
                eprintln!("recording {:.0} ms", total);
                std::thread::sleep(Duration::from_millis(total as u64));
                let rec = dev.take_rx();
                if let Some(path) = &a.dump {
                    audio::save_f32(path, &rec)?;
                    eprintln!("saved {} samples to {}", rec.len(), path);
                }
                rec
            };
            let m = evaluate(&a, &cfg, &rec, &ps);
            report(&a, &cfg, &m, rec.len());
        }
        other => anyhow::bail!("unknown mode '{}'", other),
    }
    Ok(())
}

fn sweep(_a: &Args) -> Result<()> {
    println!(
        "{:>3} {:>6} {:>8} {:>8} {:>8} {:>8} {:>10} {:>8}",
        "M", "sym_ms", "rawbps", "SNRdB", "sent", "valid", "PER%", "minSNR"
    );
    let payload = 16usize;
    let frames = 5usize;
    for &snr in &[30.0f32, 20.0, 15.0, 12.0, 10.0] {
        for &(m, sym_ms) in &[
            (2usize, 40u64),
            (4, 40),
            (8, 40),
            (16, 40),
            (4, 20),
            (8, 20),
            (16, 20),
            (4, 10),
            (8, 10),
            (16, 10),
            (4, 5),
            (8, 5),
        ] {
            let cfg = FskConfig {
                m,
                symbol_samples: (sym_ms as usize * SR as usize) / 1000,
                base_freq: 1000.0,
                amplitude: 0.25,
                preamble_symbols: 24,
                guard_samples: SR as usize / 8,
                ..FskConfig::default()
            };
            let ps: Vec<Vec<u8>> = (0..frames)
                .map(|i| (0..payload).map(|j| ((i * 131 + j * 17 + 7) % 256) as u8).collect())
                .collect();
            let mut audio = build_script(&cfg, &ps);
            add_noise(&mut audio, snr);
            let sub = Args {
                mode: "software".into(),
                m,
                symbol_ms: sym_ms,
                base_freq: 1000.0,
                amplitude: 0.25,
                differential: false,
                frames,
                payload,
                preamble: 24,
                guard_ms: 125,
                snr_db: Some(snr),
                tx_device: None,
                rx_device: None,
                rx_ms: 20000,
                latency_ms: 6000,
                dump: None,
                load: None,
            };
            let mt = evaluate(&sub, &cfg, &audio, &ps);
            println!(
                "{:>3} {:>6} {:>8.0} {:>8.0} {:>8} {:>8} {:>8.1} {:>8.1}",
                m,
                sym_ms,
                cfg.raw_bps(),
                snr,
                mt.sent,
                mt.valid,
                mt.per() * 100.0,
                mt.min_snr_db
            );
        }
    }
    Ok(())
}
