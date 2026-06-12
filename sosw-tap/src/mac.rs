use crate::fragment::{FragmentHeader, ReassemblyBuffer};
use crate::phy::Phy;
use crate::tap::TapInterface;
use anyhow::Result;
use rand::Rng as _;
use std::time::{Duration, Instant};

const FRAME_TYPE_DATA: u8 = 0;
const FRAME_TYPE_ACK: u8 = 1;

#[derive(Clone)]
pub struct MacConfig {
    pub node_id: u8,
    pub difs_ms: u64,
    pub slot_ms: u64,
    pub cw_min: u8,
    pub cw_max: u8,
    pub max_retries: u8,
    pub ack_timeout_ms: u64,
}

impl Default for MacConfig {
    fn default() -> Self {
        Self {
            node_id: 1,
            difs_ms: 300,
            slot_ms: 60,
            cw_min: 4,
            cw_max: 64,
            max_retries: 5,
            ack_timeout_ms: 700,
        }
    }
}

impl MacConfig {
    pub fn difs(&self) -> Duration {
        Duration::from_millis(self.difs_ms)
    }
    pub fn slot(&self) -> Duration {
        Duration::from_millis(self.slot_ms)
    }
    pub fn ack_timeout(&self) -> Duration {
        Duration::from_millis(self.ack_timeout_ms)
    }
}

#[derive(PartialEq)]
enum Phase {
    Idle,
    Sensing,
    Backoff,
    WaitingAck,
}

pub struct Mac {
    config: MacConfig,
    phy: Phy,
    tap: TapInterface,
    reassembly: ReassemblyBuffer,

    // TX state
    phase: Phase,
    phase_start: Instant,
    eth_frame: Vec<u8>,
    fragments: Vec<Vec<u8>>,
    frag_idx: usize,
    retries: u8,
    cw: u8,
    frame_id_counter: u8,
    tx_frame_id: u8,
}

impl Mac {
    pub fn new(config: MacConfig, phy: Phy, tap: TapInterface) -> Self {
        Self {
            config,
            phy,
            tap,
            reassembly: ReassemblyBuffer::default(),
            phase: Phase::Idle,
            phase_start: Instant::now(),
            eth_frame: Vec::new(),
            fragments: Vec::new(),
            frag_idx: 0,
            retries: 0,
            cw: 4,
            frame_id_counter: 0,
            tx_frame_id: 0,
        }
    }

    pub fn run(&mut self) -> Result<()> {
        let mut tap_buf = vec![0u8; 65536];
        let mut ack_queue: Vec<(u8, u8)> = Vec::new();

        loop {
            // 1. Drain Ethernet frames from TAP into TX pipeline
            if self.phase == Phase::Idle {
                while let Some(n) = self.tap.recv(&mut tap_buf)? {
                    self.start_tx(tap_buf[..n].to_vec());
                }
            }

            // 2. Process received OFDM frames
            while let Some(frame) = self.phy.receive_frame() {
                if frame.frame_type == FRAME_TYPE_DATA {
                    if let Some(hdr) = FragmentHeader::decode(&frame.payload) {
                        let is_for_us =
                            hdr.dst_id == self.config.node_id || hdr.dst_id == 0xFF;
                        if is_for_us {
                            let src_id = hdr.src_id;
                            let fid = hdr.frame_id;
                            let fidx = hdr.frag_index;
                            self.send_ack(src_id, fid, fidx);
                        }
                        if let Some(eth_frame) =
                            self.reassembly.add_fragment(&frame.payload)
                        {
                            log::info!(
                                "reassembled Ethernet frame ({} B)",
                                eth_frame.len()
                            );
                            if let Err(e) = self.tap.send(&eth_frame) {
                                log::error!("tap send: {}", e);
                            }
                        }
                    }
                } else if frame.frame_type == FRAME_TYPE_ACK {
                    if frame.payload.len() >= 4 {
                        let ack_dst = frame.payload[0];
                        let ack_fid = frame.payload[2];
                        let ack_fidx = frame.payload[3];
                        if ack_dst == self.config.node_id {
                            ack_queue.push((ack_fid, ack_fidx));
                        }
                    }
                }
            }

            // 3. Process ACKs
            for (fid, _fidx) in ack_queue.drain(..) {
                if self.phase == Phase::WaitingAck && fid == self.tx_frame_id {
                    self.frag_idx += 1;
                    self.retries = 0;
                    self.cw = self.config.cw_min;
                    if self.frag_idx >= self.fragments.len() {
                        log::info!("frame TX complete ({} fragments)", self.frag_idx);
                        self.phase = Phase::Idle;
                        self.eth_frame.clear();
                        self.fragments.clear();
                    } else {
                        self.phase = Phase::Idle;
                    }
                }
            }

            // 4. Run TX state machine
            match self.phase {
                Phase::Idle => {}
                Phase::Sensing => {
                    if self.phase_start.elapsed() >= self.config.difs() {
                        if !self.phy.carrier_sense() {
                            let backoff_ms =
                                rand::rng().random_range(0..=self.cw as u64)
                                    * self.config.slot_ms;
                            self.phase = Phase::Backoff;
                            self.phase_start =
                                Instant::now() + Duration::from_millis(backoff_ms);
                        } else {
                            self.phase_start = Instant::now();
                        }
                    }
                }
                Phase::Backoff => {
                    if Instant::now() >= self.phase_start {
                        let frag = &self.fragments[self.frag_idx];
                        let audio = self.phy.transmit_frame(frag, FRAME_TYPE_DATA);
                        let sample_count = audio.len();
                        self.phy.begin_tx_mute();
                        self.phy.play_samples(&audio);
                        self.phy.wait_tx_done(sample_count);
                        self.phy.end_tx_mute();

                        self.tx_frame_id = FragmentHeader::decode(frag)
                            .map(|h| h.frame_id)
                            .unwrap_or(0);

                        self.phase = Phase::WaitingAck;
                        self.phase_start = Instant::now();
                    }
                }
                Phase::WaitingAck => {
                    if self.phase_start.elapsed() >= self.config.ack_timeout() {
                        self.retries += 1;
                        if self.retries > self.config.max_retries {
                            log::warn!(
                                "max retries reached for frame, dropping"
                            );
                            self.phase = Phase::Idle;
                            self.eth_frame.clear();
                            self.fragments.clear();
                        } else {
                            self.cw = (self.cw * 2).min(self.config.cw_max);
                            log::info!(
                                "retry {}/{} for frag {}",
                                self.retries,
                                self.config.max_retries,
                                self.frag_idx
                            );
                            self.phase = Phase::Idle;
                        }
                    }
                }
            }

            // 5. Progress
            if self.phase == Phase::Idle && !self.fragments.is_empty() {
                self.phase = Phase::Sensing;
                self.phase_start = Instant::now();
            }

            self.reassembly.cleanup();
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn start_tx(&mut self, eth_frame: Vec<u8>) {
        if eth_frame.len() > 1500 {
            log::warn!("oversized Ethernet frame ({} B), dropping", eth_frame.len());
            return;
        }
        self.frame_id_counter = self.frame_id_counter.wrapping_add(1);
        self.eth_frame = eth_frame;
        self.fragments = crate::fragment::fragment_eth_frame(
            &self.eth_frame,
            0xFF, // broadcast dest for now
            self.config.node_id,
            self.frame_id_counter,
        );
        self.frag_idx = 0;
        self.retries = 0;
        self.cw = self.config.cw_min;
        log::info!(
            "TX start: {} B eth -> {} fragments",
            self.eth_frame.len(),
            self.fragments.len()
        );
    }

    fn send_ack(&mut self, dst: u8, frame_id: u8, frag_index: u8) {
        let payload = [dst, self.config.node_id, frame_id, frag_index];
        let audio = self.phy.transmit_frame(&payload, FRAME_TYPE_ACK);
        let sc = audio.len();
        self.phy.begin_tx_mute();
        self.phy.play_samples(&audio);
        self.phy.wait_tx_done(sc);
        self.phy.end_tx_mute();
    }
}
