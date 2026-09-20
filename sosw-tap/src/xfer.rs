//! Acoustic link layer: stop-and-wait ARQ over the FSK PHY, with CRC/FEC
//! already provided by [`FskConfig`]. Written against a small `Transport`
//! trait so the exact same sender/receiver code runs over a simulated lossy
//! channel (unit-tested) and over real audio (the `sosw_ftp` binary).

use sosw_core::physical::fsk::{self, FskConfig, FskDemodulator};

pub const CHUNK: usize = 32;
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

pub fn chunk_count(data_len: usize) -> u16 {
    data_len.div_ceil(CHUNK).max(1) as u16
}

/// Stop-and-wait sender. Returns after every chunk is acknowledged and a BYE
/// is acknowledged (or after exhausting retries).
pub fn run_sender<T: Transport>(
    data: &[u8],
    t: &mut T,
    timeout_ms: u64,
    max_retries: usize,
) -> XferStats {
    let total = chunk_count(data.len());
    let mut stats = XferStats { bytes: data.len(), ..Default::default() };
    for seq in 0..total {
        let start = seq as usize * CHUNK;
        let end = (start + CHUNK).min(data.len());
        let frame = encode_transport(KIND_DATA, seq, total, &data[start..end]);
        let mut acked = false;
        for attempt in 0..=max_retries {
            if attempt > 0 {
                stats.retransmits += 1;
            }
            t.send(&frame);
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
                match t.recv(remaining.as_millis() as u64) {
                    Some(resp) => {
                        if let Some((kind, rseq, _, _)) = decode_transport(&resp) {
                            if kind == KIND_ACK && rseq == seq {
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
            if acked {
                break;
            }
        }
        if acked {
            stats.chunks += 1;
        }
    }
    // Best-effort BYE.
    let bye = encode_transport(KIND_BYE, total, total, &[]);
    for _ in 0..max_retries {
        t.send(&bye);
        if t.recv(timeout_ms).is_some() {
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
) -> (Vec<u8>, XferStats) {
    let mut out = Vec::new();
    let mut expected: u16 = 0;
    let mut stats = XferStats::default();
    let mut idle = 0usize;
    loop {
        let frame = match t.recv(timeout_ms) {
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
                // ACK current expected-1 (dup) or the seq just accepted.
                let ack = encode_transport(KIND_ACK, seq, 0, &[]);
                t.send(&ack);
            }
            KIND_BYE => {
                let ack = encode_transport(KIND_ACK, seq, 0, &[]);
                t.send(&ack);
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
}

impl AudioTransport {
    pub fn new(tx: Option<&str>, rx: Option<&str>, cfg: FskConfig) -> anyhow::Result<Self> {
        Ok(Self {
            audio: crate::audio::DuplexAudio::new(tx, rx)?,
            cfg,
            last_quality: (0.0, true),
        })
    }
}

impl Transport for AudioTransport {
    fn send(&mut self, payload: &[u8]) {
        let audio = self.cfg.encode_payload(payload);
        eprintln!(
            "  TX {} samples ({:.1} s)",
            audio.len(),
            audio.len() as f32 / 48_000.0
        );
        self.audio.play_blocking(&audio, std::time::Duration::from_millis(50));
        // Drop our own delayed echo so it does not crowd the listen window.
        self.audio.clear_rx();
    }

    fn recv(&mut self, timeout_ms: u64) -> Option<Vec<u8>> {
        let mut dem = FskDemodulator::new(self.cfg.clone());
        self.audio.clear_rx();
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(timeout_ms);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let chunk = self.audio.take_rx();
            if !chunk.is_empty() {
                if let Some(fr) = dem.process_samples(&chunk) {
                    if let Some(p) = fsk::unwrap_frame(&fr.bytes) {
                        eprintln!(
                            "  RX frame (kind={}) minSNR={:.1}dB",
                            p.first().copied().unwrap_or(255),
                            fr.min_snr_db
                        );
                        self.last_quality = (fr.min_snr_db, fr.fec_ok);
                        return Some(p);
                    }
                }
            }
            if start.elapsed() > timeout {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sim_cfg() -> FskConfig {
        FskConfig {
            m: 4,
            symbol_samples: 480,
            rs_nsym: 16,
            fec_data_block: 32,
            payload_size: 64,
            guard_samples: 1_200,
            ..FskConfig::default()
        }
    }

    #[test]
    fn clean_transfer_roundtrip() {
        let cfg = sim_cfg();
        let data: Vec<u8> = (0..200u8).map(|i| i.wrapping_mul(3)).collect();
        let mut ch = SimTransport::new(cfg, None, 0.0, 1);
        let stats = run_sender(&data, &mut ch, 10, 5);
        assert_eq!(ch.received, data, "clean transfer mismatch");
        assert_eq!(stats.retransmits, 0);
    }

    #[test]
    fn lossy_transfer_uses_arq() {
        let cfg = sim_cfg();
        let data: Vec<u8> = (0..400usize).map(|i| (i as u8).wrapping_add(11)).collect();
        let mut ch = SimTransport::new(cfg, Some(20.0), 0.25, 7);
        let stats = run_sender(&data, &mut ch, 10, 8);
        assert_eq!(ch.received, data, "lossy transfer mismatch");
        assert!(stats.retransmits > 0, "ARQ never triggered");
    }

    #[test]
    fn receiver_only_collects_in_order() {
        let cfg = sim_cfg();
        let data: Vec<u8> = (0..100u8).collect();
        let mut ch = SimTransport::new(cfg, None, 0.0, 2);
        run_sender(&data, &mut ch, 10, 3);
        assert_eq!(ch.received.len(), data.len());
        assert_eq!(ch.received, data);
    }
}
