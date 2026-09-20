//! Acoustic link layer: stop-and-wait ARQ over the FSK PHY, with CRC/FEC
//! already provided by [`FskConfig`]. Written against a small `Transport`
//! trait so the exact same sender/receiver code runs over a simulated lossy
//! channel (unit-tested) and over real audio (the `sosw_ftp` binary).

use sosw_core::physical::fsk::{self, FskConfig, FskDemodulator};
use sosw_core::physical::fsk_bank::{FskBankConfig, FskBankDemodulator};

pub const CHUNK: usize = 128;
const KIND_DATA: u8 = 0;
const KIND_ACK: u8 = 1;
const KIND_BYE: u8 = 2;

#[derive(Clone, Debug, Default)]
pub struct XferStats {
    pub chunks: usize,
    pub retransmits: usize,
    pub timeouts: usize,
    pub bytes: usize,
}

/// A frame-oriented link endpoint. `send` transmits one transport payload;
/// `recv` returns the next transport payload from the peer, or None on timeout.
pub trait Transport {
    fn send(&mut self, payload: &[u8]);
    fn recv(&mut self, timeout_ms: u64) -> Option<Vec<u8>>;
    /// Switch the modem configuration (used by link training mode negotiation).
    fn set_config(&mut self, _cfg: FskConfig) {}
    /// Send using a specific config (DATA and ACK can differ).
    fn send_cfg(&mut self, payload: &[u8], cfg: &FskConfig) {
        self.set_config(cfg.clone());
        self.send(payload);
    }
    /// Receive using a specific config.
    fn recv_cfg(&mut self, timeout_ms: u64, cfg: &FskConfig) -> Option<Vec<u8>> {
        self.set_config(cfg.clone());
        self.recv(timeout_ms)
    }
    /// Quality of the most recently received frame: (min tone SNR dB, fec_ok).
    fn last_quality(&self) -> (f32, bool) {
        (0.0, true)
    }
}

/// Transport payload: `[kind, seq_hi, seq_lo, total_hi, total_lo, len, data..]`.
pub fn encode_transport(kind: u8, seq: u16, total: u16, data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(6 + data.len());
    v.push(kind);
    v.push((seq >> 8) as u8);
    v.push(seq as u8);
    v.push((total >> 8) as u8);
    v.push(total as u8);
    v.push(data.len() as u8);
    v.extend_from_slice(data);
    v
}

pub fn decode_transport(bytes: &[u8]) -> Option<(u8, u16, u16, Vec<u8>)> {
    if bytes.len() < 6 {
        return None;
    }
    let kind = bytes[0];
    let seq = ((bytes[1] as u16) << 8) | bytes[2] as u16;
    let total = ((bytes[3] as u16) << 8) | bytes[4] as u16;
    let len = bytes[5] as usize;
    if 6 + len > bytes.len() {
        return None;
    }
    Some((kind, seq, total, bytes[6..6 + len].to_vec()))
}

pub fn chunk_count(data_len: usize, chunk: usize) -> u16 {
    data_len.div_ceil(chunk.max(1)).max(1) as u16
}

/// Stop-and-wait sender. Returns after every chunk is acknowledged and a BYE
/// is acknowledged (or after exhausting retries).
pub fn run_sender<T: Transport>(
    data: &[u8],
    t: &mut T,
    timeout_ms: u64,
    max_retries: usize,
    data_cfg: &FskConfig,
    ack_cfg: &FskConfig,
    chunk: usize,
) -> XferStats {
    let chunk = chunk.max(1);
    let total = chunk_count(data.len(), chunk);
    let mut stats = XferStats { bytes: data.len(), ..Default::default() };
    for seq in 0..total {
        let start = seq as usize * chunk;
        let end = (start + chunk).min(data.len());
        let frame = encode_transport(KIND_DATA, seq, total, &data[start..end]);
        let mut acked = false;
        let mut chunk_air_ms = 0.0f64;
        let mut chunk_wait_ms = 0.0f64;
        let mut attempts = 0usize;
        for attempt in 0..=max_retries {
            if attempt > 0 {
                stats.retransmits += 1;
            }
            attempts += 1;
            let t_air = std::time::Instant::now();
            t.send_cfg(&frame, data_cfg);
            chunk_air_ms += t_air.elapsed().as_secs_f64() * 1000.0;
            let t_wait = std::time::Instant::now();
            // Listen for the matching ACK until the attempt deadline. Frames
            // that are not our ACK (e.g. our own delayed echo) are ignored
            // rather than treated as a response, so we do not retransmit early.
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    stats.timeouts += 1;
                    break;
                }
                match t.recv_cfg(remaining.as_millis() as u64, ack_cfg) {
                    Some(resp) => {
                        if let Some((kind, rseq, _, data)) = decode_transport(&resp) {
                            if kind == KIND_ACK && rseq == seq {
                                let (my_snr, _) = t.last_quality();
                                let peer_snr = data.first().copied().unwrap_or(0) as f32;
                                eprintln!(
                                    "  [arq] seq {} acked: peer_rx_snr={:.0} dB, my_rx_snr={:.1} dB",
                                    seq, peer_snr, my_snr
                                );
                                acked = true;
                                break;
                            }
                        }
                    }
                    None => {
                        stats.timeouts += 1;
                        break;
                    }
                }
            }
            chunk_wait_ms += t_wait.elapsed().as_secs_f64() * 1000.0;
            if acked {
                break;
            }
        }
        eprintln!(
            "[perf] chunk {}: air={:.0}ms wait={:.0}ms attempts={} bytes={}",
            seq, chunk_air_ms, chunk_wait_ms, attempts, end - start
        );
        if acked {
            stats.chunks += 1;
            // Wait out the peer's ACK air time and echo tail before the next
            // DATA, so the peer is listening when it arrives.
            std::thread::sleep(std::time::Duration::from_millis(SEND_TURNAROUND_MS));
        }
    }
    // Best-effort BYE: the receiver also exits on idle, so do not spend many
    // multi-second timeouts chasing this ACK.
    let bye = encode_transport(KIND_BYE, total, total, &[]);
    for _ in 0..2 {
        // BYE is a DATA-class frame, so the receiver (listening with data_cfg)
        // can decode it; its ACK comes back on ack_cfg.
        t.send_cfg(&bye, data_cfg);
        if t.recv_cfg((timeout_ms / 2).max(2000), ack_cfg).is_some() {
            break;
        }
    }
    stats
}

/// Stop-and-wait receiver. Returns the reassembled bytes once BYE is seen (or
/// after `max_rounds` with no new data).
pub fn run_receiver<T: Transport>(
    t: &mut T,
    timeout_ms: u64,
    max_rounds: usize,
    data_cfg: &FskConfig,
    ack_cfg: &FskConfig,
) -> (Vec<u8>, XferStats) {
    let mut out = Vec::new();
    let mut expected: u16 = 0;
    let mut stats = XferStats::default();
    let mut idle = 0usize;
    loop {
        let frame = match t.recv_cfg(timeout_ms, data_cfg) {
            Some(f) => f,
            None => {
                idle += 1;
                if idle > max_rounds {
                    break;
                }
                continue;
            }
        };
        idle = 0;
        let (kind, seq, _total, data) = match decode_transport(&frame) {
            Some(x) => x,
            None => continue,
        };
        match kind {
            KIND_DATA => {
                if seq == expected {
                    out.extend_from_slice(&data);
                    stats.chunks += 1;
                    stats.bytes += data.len();
                    expected = expected.wrapping_add(1);
                }
                // Turn gap: let the sender's own speaker echo decay and its
                // mute release before we transmit, so it hears only our ACK.
                std::thread::sleep(std::time::Duration::from_millis(RECV_TURN_GAP_MS));
                // ACK current seq and report the measured DATA quality so the
                // sender can log/adapt on evidence.
                let (snr, fec_ok) = t.last_quality();
                let ack = encode_transport(
                    KIND_ACK,
                    seq,
                    0,
                    &[snr.clamp(0.0, 255.0) as u8, fec_ok as u8],
                );
                t.send_cfg(&ack, ack_cfg);
            }
            KIND_BYE => {
                let ack = encode_transport(KIND_ACK, seq, 0, &[]);
                t.send_cfg(&ack, ack_cfg);
                break;
            }
            _ => {}
        }
    }
    (out, stats)
}

// ---------------------------------------------------------------------------
// Simulated channel: validates the ARQ/framing/FEC logic without audio.
// ---------------------------------------------------------------------------

pub struct SimTransport {
    cfg: FskConfig,
    noise_db: Option<f32>,
    drop_prob: f32,
    rng: u32,
    /// Audio queued from the peer (the next thing recv() will hear).
    pending: Option<Vec<f32>>,
    /// Receiver state.
    expected: u16,
    pub received: Vec<u8>,
    pub sent_frames: usize,
    pub dropped: usize,
    last_quality: (f32, bool),
}

impl SimTransport {
    pub fn new(cfg: FskConfig, noise_db: Option<f32>, drop_prob: f32, seed: u32) -> Self {
        Self {
            cfg,
            noise_db,
            drop_prob,
            rng: seed | 1,
            pending: None,
            expected: 0,
            received: Vec::new(),
            sent_frames: 0,
            dropped: 0,
            last_quality: (0.0, true),
        }
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng & 0xFFFFFF) as f32 / 0xFFFFFF as f32
    }

    fn channel(&mut self, audio: &[f32]) -> Option<Vec<f32>> {
        if self.rand() < self.drop_prob {
            self.dropped += 1;
            return None;
        }
        let mut a = audio.to_vec();
        if let Some(snr) = self.noise_db {
            let sig = a.iter().map(|s| s * s).sum::<f32>() / a.len().max(1) as f32;
            let noise = (sig / 10f32.powf(snr / 10.0)).sqrt();
            for s in a.iter_mut() {
                *s += (self.rand() * 2.0 - 1.0) * noise;
            }
        }
        Some(a)
    }

    fn demod(&self, audio: &[f32]) -> (Option<Vec<u8>>, f32, bool) {
        let dem = FskDemodulator::new(self.cfg.clone());
        let frames = dem.decode_capture(audio);
        let mut best_snr = 0.0f32;
        for fr in frames {
            best_snr = best_snr.max(fr.min_snr_db);
            if let Some(p) = fsk::unwrap_frame(&fr.bytes) {
                return (Some(p), fr.min_snr_db, fr.fec_ok);
            }
        }
        (None, best_snr, false)
    }
}

impl Transport for SimTransport {
    fn send(&mut self, payload: &[u8]) {
        self.sent_frames += 1;
        let audio = self.cfg.encode_payload(payload);
        let heard = match self.channel(&audio) {
            Some(a) => a,
            None => {
                self.pending = None;
                return;
            }
        };
        // Embedded receiver logic.
        let (decoded, snr, fec_ok) = self.demod(&heard);
        self.last_quality = (snr, fec_ok);
        let response = decoded.and_then(|bytes| {
            let (kind, seq, _t, data) = decode_transport(&bytes)?;
            match kind {
                KIND_DATA => {
                    if seq == self.expected {
                        self.received.extend_from_slice(&data);
                        self.expected = self.expected.wrapping_add(1);
                    }
                    Some(encode_transport(KIND_ACK, seq, 0, &[]))
                }
                KIND_BYE => Some(encode_transport(KIND_ACK, seq, 0, &[])),
                _ => None,
            }
        });
        // The response travels back over the same noisy channel.
        self.pending = match response {
            Some(p) => {
                let a = self.cfg.encode_payload(&p);
                self.channel(&a)
            }
            None => None,
        };
    }

    fn recv(&mut self, _timeout_ms: u64) -> Option<Vec<u8>> {
        let audio = self.pending.take()?;
        let (out, snr, ok) = self.demod(&audio);
        self.last_quality = (snr, ok);
        out
    }

    fn set_config(&mut self, cfg: FskConfig) {
        self.cfg = cfg;
    }

    fn last_quality(&self) -> (f32, bool) {
        self.last_quality
    }
}

// ---------------------------------------------------------------------------
// Real acoustic endpoint.
// ---------------------------------------------------------------------------

pub struct AudioTransport {
    audio: crate::audio::DuplexAudio,
    cfg: FskConfig,
    last_quality: (f32, bool),
    /// Decoded frames not yet returned. Our own echo is decoded here too and
    /// ignored by the caller, instead of being discarded with the buffer.
    pending: std::collections::VecDeque<Vec<u8>>,
    /// Optional raw capture of everything the local mic hears, for offline
    /// diagnosis (little-endian f32).
    dump: Option<std::fs::File>,
    /// Capture accumulated across recv calls. A frame can be longer than the
    /// recv timeout, so this must survive a timeout rather than being dropped.
    collected: Vec<f32>,
}

impl AudioTransport {
    pub fn new(tx: Option<&str>, rx: Option<&str>, cfg: FskConfig) -> anyhow::Result<Self> {
        Self::with_dump(tx, rx, cfg, None)
    }

    pub fn with_dump(
        tx: Option<&str>,
        rx: Option<&str>,
        cfg: FskConfig,
        dump_path: Option<&str>,
    ) -> anyhow::Result<Self> {
        let dump = match dump_path {
            Some(p) => Some(std::fs::File::create(p)?),
            None => None,
        };
        Ok(Self {
            audio: crate::audio::DuplexAudio::new(tx, rx)?,
            cfg,
            last_quality: (0.0, true),
            pending: std::collections::VecDeque::new(),
            dump,
            collected: Vec::new(),
        })
    }
}

/// Turn gap before a receiver replies, so the sender's echo has ended.
const RECV_TURN_GAP_MS: u64 = 400;
/// After an ACK, the sender waits out the peer's ACK air time plus echo tail
/// before sending the next DATA, so the peer is listening when it arrives.
const SEND_TURNAROUND_MS: u64 = 600;

fn now_ms() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let s = now.as_secs();
    format!("{:02}:{:02}:{:02}.{:03}", (s / 3600) % 24, (s / 60) % 60, s % 60, now.subsec_millis())
}

/// Mute tail after playback, to ride out the local speaker echo (as in the
/// proven `sosw_link` handshake).
const ECHO_TAIL_MS: u64 = 250;

impl Transport for AudioTransport {
    fn send(&mut self, payload: &[u8]) {
        // Transmit with the local mic muted (plus an echo tail) so we never
        // record our own, much louder, speaker echo. The peer waits a turn gap
        // before replying, so its response arrives after we unmute.
        self.audio.clear_rx();
        self.pending.clear();
        self.collected.clear();
        let audio = self.cfg.encode_payload(payload);
        eprintln!(
            "[{}] TX {} samples ({:.1} s)",
            now_ms(),
            audio.len(),
            audio.len() as f32 / 48_000.0
        );
        self.audio.play_muted(&audio, std::time::Duration::from_millis(ECHO_TAIL_MS));
    }

    fn recv(&mut self, timeout_ms: u64) -> Option<Vec<u8>> {
        // Return any already-decoded frame first (e.g. our own echo, which the
        // caller ignores), then collect the listen window and decode the whole
        // capture once. Decoding a growing buffer in a sliding window only ever
        // sees prefixes and never assembles a frame (the `sosw_link` lesson).
        if let Some(p) = self.pending.pop_front() {
            return Some(p);
        }
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(timeout_ms);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let chunk = self.audio.take_rx();
            if let Some(f) = &mut self.dump {
                use std::io::Write;
                let mut bytes = Vec::with_capacity(chunk.len() * 4);
                for s in &chunk {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                let _ = f.write_all(&bytes);
            }
            self.collected.extend(chunk);
            if self.collected.len() >= self.cfg.preamble_samples() {
                let dem = FskDemodulator::new(self.cfg.clone());
                let frames = dem.decode_capture(&self.collected);
                for fr in &frames {
                    if let Some(p) = fsk::unwrap_frame(&fr.bytes) {
                        eprintln!(
                            "[{}] RX frame (kind={}) minSNR={:.1}dB meanSNR={:.1}dB",
                            now_ms(),
                            p.first().copied().unwrap_or(255),
                            fr.min_snr_db,
                            fr.mean_snr_db
                        );
                        self.last_quality = (fr.min_snr_db, fr.fec_ok);
                        self.pending.push_back(p);
                    }
                }
                // Only drop the capture once a *valid* frame is in hand. A
                // partial frame can still produce a (CRC-failing) region, and
                // clearing then would throw away the rest of the real frame.
                if !self.pending.is_empty() {
                    self.collected.clear();
                    return self.pending.pop_front();
                }
            }
            if start.elapsed() > timeout {
                // Keep self.collected so a frame spanning this timeout is not
                // lost; the next recv continues from here.
                return None;
            }
        }
    }

    fn set_config(&mut self, cfg: FskConfig) {
        self.cfg = cfg;
    }

    fn last_quality(&self) -> (f32, bool) {
        self.last_quality
    }
}

// ---------------------------------------------------------------------------
// Parallel-FSK-bank endpoint (carrier-selective, higher rate).
// ---------------------------------------------------------------------------

pub struct BankTransport {
    audio: crate::audio::DuplexAudio,
    cfg: FskBankConfig,
    last_quality: (f32, bool),
    pending: std::collections::VecDeque<Vec<u8>>,
    collected: Vec<f32>,
    dump: Option<std::fs::File>,
}

impl BankTransport {
    pub fn new(
        tx: Option<&str>,
        rx: Option<&str>,
        cfg: FskBankConfig,
        dump_path: Option<&str>,
    ) -> anyhow::Result<Self> {
        let dump = match dump_path {
            Some(p) => Some(std::fs::File::create(p)?),
            None => None,
        };
        Ok(Self {
            audio: crate::audio::DuplexAudio::new(tx, rx)?,
            cfg,
            last_quality: (0.0, true),
            pending: std::collections::VecDeque::new(),
            collected: Vec::new(),
            dump,
        })
    }
}

impl Transport for BankTransport {
    fn send(&mut self, payload: &[u8]) {
        self.audio.clear_rx();
        self.pending.clear();
        self.collected.clear();
        let audio = self.cfg.encode_payload(payload);
        eprintln!(
            "[{}] TX(bank) {} samples ({:.1} s)",
            now_ms(),
            audio.len(),
            audio.len() as f32 / 48_000.0
        );
        self.audio.play_muted(&audio, std::time::Duration::from_millis(ECHO_TAIL_MS));
    }

    fn recv(&mut self, timeout_ms: u64) -> Option<Vec<u8>> {
        if let Some(p) = self.pending.pop_front() {
            return Some(p);
        }
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(timeout_ms);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let chunk = self.audio.take_rx();
            if let Some(f) = &mut self.dump {
                use std::io::Write;
                let mut bytes = Vec::with_capacity(chunk.len() * 4);
                for s in &chunk {
                    bytes.extend_from_slice(&s.to_le_bytes());
                }
                let _ = f.write_all(&bytes);
            }
            self.collected.extend(chunk);
            if self.collected.len() >= self.cfg.preamble_samples() {
                let dem = FskBankDemodulator::new(self.cfg.clone());
                for fr in dem.decode_capture(&self.collected) {
                    if let Some(p) = fsk::unwrap_frame(&fr.bytes) {
                        eprintln!(
                            "[{}] RX(bank) frame (kind={}) minSNR={:.1}dB meanSNR={:.1}dB",
                            now_ms(),
                            p.first().copied().unwrap_or(255),
                            fr.min_snr_db,
                            fr.mean_snr_db
                        );
                        self.last_quality = (fr.min_snr_db, fr.fec_ok);
                        self.pending.push_back(p);
                    }
                }
                if !self.pending.is_empty() {
                    self.collected.clear();
                    return self.pending.pop_front();
                }
            }
            if start.elapsed() > timeout {
                return None;
            }
        }
    }

    /// Bank transport ignores the per-call FSK config: it always uses its bank
    /// configuration, which carries the selected carriers.
    fn send_cfg(&mut self, payload: &[u8], _cfg: &FskConfig) {
        self.send(payload);
    }

    fn recv_cfg(&mut self, timeout_ms: u64, _cfg: &FskConfig) -> Option<Vec<u8>> {
        self.recv(timeout_ms)
    }

    fn last_quality(&self) -> (f32, bool) {
        self.last_quality
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sim_cfg() -> FskConfig {
        FskConfig {
            m: 4,
            symbol_samples: 480,
            rs_nsym: 16,
            fec_data_block: 32,
            payload_size: CHUNK + 8,
            guard_samples: 1_200,
            ..FskConfig::default()
        }
    }

    #[test]
    fn clean_transfer_roundtrip() {
        let cfg = sim_cfg();
        let data: Vec<u8> = (0..200u8).map(|i| i.wrapping_mul(3)).collect();
        let mut ch = SimTransport::new(cfg.clone(), None, 0.0, 1);
        let stats = run_sender(&data, &mut ch, 10, 5, &cfg, &cfg, CHUNK);
        assert_eq!(ch.received, data, "clean transfer mismatch");
        assert_eq!(stats.retransmits, 0);
    }

    #[test]
    fn lossy_transfer_uses_arq() {
        let cfg = sim_cfg();
        let data: Vec<u8> = (0..400usize).map(|i| (i as u8).wrapping_add(11)).collect();
        let mut ch = SimTransport::new(cfg.clone(), Some(20.0), 0.25, 7);
        let stats = run_sender(&data, &mut ch, 10, 24, &cfg, &cfg, CHUNK);
        assert_eq!(ch.received, data, "lossy transfer mismatch");
        assert!(stats.retransmits > 0, "ARQ never triggered");
    }

    #[test]
    fn receiver_only_collects_in_order() {
        let cfg = sim_cfg();
        let data: Vec<u8> = (0..100u8).collect();
        let mut ch = SimTransport::new(cfg.clone(), None, 0.0, 2);
        run_sender(&data, &mut ch, 10, 3, &cfg, &cfg, CHUNK);
        assert_eq!(ch.received.len(), data.len());
        assert_eq!(ch.received, data);
    }
}
