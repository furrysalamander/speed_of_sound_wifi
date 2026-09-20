//! Acoustic file transfer: stop-and-wait ARQ over FSK, with RS FEC + CRC.
//!
//!   sosw-ftp --mode sim  [--bytes N] [--snr-db D] [--drop P]
//!   sosw-ftp --mode recv --out FILE [--rx-device ..] [--timeout-ms ..]
//!   sosw-ftp --mode send --file FILE [--tx-device ..] [--rx-device ..]
//!
//! The `sim` mode runs both endpoints over an in-process lossy channel and is
//! the deterministic test of the protocol. The acoustic modes are meant to run
//! on two machines (recv first on the peer, then send here); the acoustic
//! latency is multi-second, so the timeout is generous.

use anyhow::Result;
use clap::Parser;
use sosw_core::physical::fsk::FskConfig;
use sosw_core::physical::fsk_bank::FskBankConfig;
use sosw_tap::xfer::{self, run_receiver, run_sender, AudioTransport, BankTransport, SimTransport};
use std::time::Instant;


const SR: u32 = 48_000;

#[derive(Parser)]
#[command(name = "sosw-ftp", about = "Stop-and-wait acoustic file transfer")]
struct Args {
    #[arg(long, default_value = "sim")]
    mode: String,
    #[arg(long)]
    file: Option<String>,
    #[arg(long)]
    out: Option<String>,
    /// Bytes to transfer in sim mode.
    #[arg(long, default_value_t = 512)]
    bytes: usize,
    #[arg(long)]
    snr_db: Option<f32>,
    #[arg(long, default_value_t = 0.0)]
    drop: f32,
    #[arg(long, default_value_t = 4)]
    m: usize,
    #[arg(long, default_value_t = 3)]
    symbol_ms: u64,
    #[arg(long, default_value_t = 1000.0)]
    base_freq: f32,
    #[arg(long, default_value_t = 0.5)]
    amplitude: f32,
    #[arg(long, default_value_t = 16)]
    rs_nsym: usize,
    #[arg(long, default_value_t = 64)]
    fec_block: usize,
    #[arg(long, default_value_t = 24)]
    preamble: usize,
    #[arg(long, default_value_t = 120)]
    guard_ms: u64,
    #[arg(long)]
    tx_device: Option<String>,
    #[arg(long)]
    rx_device: Option<String>,
    /// Receiver's listen window (must exceed one DATA frame).
    #[arg(long, default_value_t = 14000)]
    timeout_ms: u64,
    /// Sender's wait for one ACK. Short, because latency is now ~120 ms.
    #[arg(long, default_value_t = 3000)]
    ack_timeout_ms: u64,
    #[arg(long, default_value_t = 6)]
    max_retries: usize,
    /// Receiver idle rounds before giving up.
    #[arg(long, default_value_t = 4)]
    max_rounds: usize,
    /// Dump all received audio to this raw f32 file (for offline diagnosis).
    #[arg(long)]
    dump_rx: Option<String>,
    /// Use the parallel FSK bank on these carrier base frequencies (Hz),
    /// comma-separated. Empty = single-stream FSK.
    #[arg(long, value_delimiter = ',')]
    carriers: Vec<f32>,
    /// Bank channels are 2-FSK (default).
    #[arg(long, default_value_t = 2)]
    tones: usize,
    /// Bank channel count when no explicit --carriers list is given.
    #[arg(long, default_value_t = 0)]
    channels: usize,
    /// Payload bytes per ARQ chunk.
    #[arg(long, default_value_t = xfer::CHUNK)]
    chunk: usize,
}

fn make_cfg(a: &Args) -> FskConfig {
    FskConfig {
        m: a.m,
        symbol_samples: (a.symbol_ms as usize * SR as usize) / 1000,
        base_freq: a.base_freq,
        amplitude: a.amplitude,
        preamble_symbols: a.preamble,
        guard_samples: (a.guard_ms as usize * SR as usize) / 1000,
        rs_nsym: a.rs_nsym,
        fec_data_block: a.fec_block,
        // Transport frames are 6 + CHUNK bytes; keep the padding tight.
        payload_size: xfer::CHUNK + 8,
        ..FskConfig::default()
    }
}

fn make_ack_cfg(a: &Args) -> FskConfig {
    FskConfig {
        // ACKs are short: a small preamble and CRC-only keep their air time
        // (and therefore the half-duplex turnaround) small.
        preamble_symbols: 16,
        rs_nsym: 4,
        fec_data_block: 16,
        payload_size: 16,
        ..make_cfg(a)
    }
}

fn make_bank_cfg(a: &Args) -> FskBankConfig {
    FskBankConfig {
        n_channels: if a.carriers.is_empty() { a.channels } else { a.carriers.len() },
        tones_per_channel: a.tones,
        symbol_samples: (a.symbol_ms as usize * SR as usize) / 1000,
        carrier_freqs: a.carriers.clone(),
        amplitude: a.amplitude,
        preamble_symbols: a.preamble,
        guard_samples: (a.guard_ms as usize * SR as usize) / 1000,
        rs_nsym: a.rs_nsym,
        fec_data_block: a.fec_block,
        payload_size: xfer::CHUNK + 8,
        ..FskBankConfig::default()
    }
}

fn main() -> Result<()> {
    let a = Args::parse();
    let cfg = make_cfg(&a);
    let ack_cfg = make_ack_cfg(&a);
    let use_bank = !a.carriers.is_empty() || a.channels > 0;
    match a.mode.as_str() {
        "sim" => {
            let data: Vec<u8> = (0..a.bytes).map(|i| (i as u8).wrapping_mul(37).wrapping_add(5)).collect();
            let mut ch = SimTransport::new(cfg.clone(), a.snr_db, a.drop, 0xC0FFEE);
            println!(
                "sim: {} bytes, {} chunks, snr={:?} dB, drop={:.2}",
                data.len(),
                xfer::chunk_count(data.len(), a.chunk),
                a.snr_db,
                a.drop
            );
            let stats = run_sender(&data, &mut ch, 10, a.max_retries.max(1), &cfg, &cfg, a.chunk);
            let ok = ch.received == data;
            println!(
                "result: received {} bytes, {} chunks, {} retransmits, {} timeouts, {} channel drops, matches={}",
                ch.received.len(),
                stats.chunks,
                stats.retransmits,
                stats.timeouts,
                ch.dropped,
                ok
            );
            if !ok {
                anyhow::bail!("sim transfer mismatch");
            }
        }
        "send" => {
            let path = a.file.as_deref().ok_or_else(|| anyhow::anyhow!("--file required"))?;
            let data = std::fs::read(path)?;
            let start = Instant::now();
            if use_bank {
                let bcfg = make_bank_cfg(&a);
                let mut t = BankTransport::new(a.tx_device.as_deref(), a.rx_device.as_deref(), bcfg, a.dump_rx.as_deref())?;
                println!("sending {} bytes ({} chunks, bank {} carriers) ...", data.len(), xfer::chunk_count(data.len(), a.chunk), a.carriers.len());
                let stats = run_sender(&data, &mut t, a.ack_timeout_ms, a.max_retries, &cfg, &ack_cfg, a.chunk);
                let dur = start.elapsed().as_secs_f32();
                println!("done: {} chunks, {} retransmits, {} timeouts in {:.1}s ({:.1} B/s)", stats.chunks, stats.retransmits, stats.timeouts, dur, data.len() as f32 / dur.max(0.01));
                return Ok(());
            }
            let mut t = AudioTransport::with_dump(a.tx_device.as_deref(), a.rx_device.as_deref(), cfg.clone(), a.dump_rx.as_deref())?;
            println!(
                "sending {} bytes ({} chunks, {} bps raw) ...",
                data.len(),
                xfer::chunk_count(data.len(), a.chunk),
                cfg.raw_bps()
            );
            let stats = run_sender(&data, &mut t, a.ack_timeout_ms, a.max_retries, &cfg, &ack_cfg, a.chunk);
            let dur = start.elapsed().as_secs_f32();
            println!(
                "done: {} chunks, {} retransmits, {} timeouts in {:.1}s ({:.1} B/s)",
                stats.chunks,
                stats.retransmits,
                stats.timeouts,
                dur,
                data.len() as f32 / dur.max(0.01)
            );
        }
        "recv" => {
            let path = a.out.as_deref().unwrap_or("received.bin");
            if use_bank {
                let bcfg = make_bank_cfg(&a);
                let mut t = BankTransport::new(a.tx_device.as_deref(), a.rx_device.as_deref(), bcfg, a.dump_rx.as_deref())?;
                println!("listening for bank transfer (timeout {} ms, {} carriers) ...", a.timeout_ms, a.carriers.len());
                let (data, stats) = run_receiver(&mut t, a.timeout_ms, a.max_rounds, &cfg, &ack_cfg);
                std::fs::write(path, &data)?;
                println!("received {} bytes ({} chunks) -> {}", data.len(), stats.chunks, path);
                return Ok(());
            }
            let mut t = AudioTransport::with_dump(a.tx_device.as_deref(), a.rx_device.as_deref(), cfg.clone(), a.dump_rx.as_deref())?;
            println!("listening for transfer (timeout {} ms) ...", a.timeout_ms);
            let (data, stats) = run_receiver(&mut t, a.timeout_ms, a.max_rounds, &cfg, &ack_cfg);
            std::fs::write(path, &data)?;
            println!(
                "received {} bytes ({} chunks) -> {}",
                data.len(),
                stats.chunks,
                path
            );
        }
        other => anyhow::bail!("unknown mode '{}'", other),
    }
    Ok(())
}
