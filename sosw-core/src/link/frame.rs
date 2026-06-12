use crate::config::Config;
use crate::link::crc;
use crate::link::fec::ReedSolomonFec;

const SYNC_PATTERN: [u8; 8] = [0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55, 0xAA, 0x55];

#[derive(Clone, Debug)]
pub struct ParsedFrame {
    pub payload: Vec<u8>,
    pub frame_type: u8,
    pub sequence_number: u16,
    pub valid: bool,
}

pub struct FrameAssembler {
    fec: ReedSolomonFec,
    sequence_number: u16,
}

impl FrameAssembler {
    pub fn new(config: &Config) -> Self {
        let fec = ReedSolomonFec::new(config.rs_nsym);
        Self {
            fec,
            sequence_number: 0,
        }
    }

    pub fn assemble_frame(&mut self, payload: &[u8]) -> Vec<u8> {
        self.assemble_frame_with_type(payload, 0)
    }

    pub fn assemble_frame_with_type(&mut self, payload: &[u8], frame_type: u8) -> Vec<u8> {
        let payload_len = payload.len() as u16;
        let seq = self.sequence_number;
        self.sequence_number = self.sequence_number.wrapping_add(1);

        let mut header = vec![0u8; 4];
        header[0] = (frame_type << 4) | ((payload_len >> 8) as u8);
        header[1] = (payload_len & 0xFF) as u8;
        header[2] = ((seq >> 8) & 0xFF) as u8;
        header[3] = (seq & 0xFF) as u8;

        let mut to_encode = Vec::with_capacity(header.len() + payload.len());
        to_encode.extend_from_slice(&header);
        to_encode.extend_from_slice(payload);

        let encoded = self.fec.encode(&to_encode);

        let mut frame = Vec::with_capacity(SYNC_PATTERN.len() + encoded.len() + 4);
        frame.extend_from_slice(&SYNC_PATTERN);
        frame.extend_from_slice(&encoded);
        crc::compute_and_append_crc(&mut frame);

        frame
    }
}

pub struct FrameParser {
    fec: ReedSolomonFec,
    buffer: Vec<u8>,
    pub frames_received: usize,
    pub frames_valid: usize,
    pub crc_fail: usize,
    pub fec_fail: usize,
    max_blocks: usize,
}

impl FrameParser {
    pub fn new(config: &Config) -> Self {
        let fec = ReedSolomonFec::new(config.rs_nsym);
        let msg_len = fec.max_data_bytes();
        let payload_max = config.payload_size;
        let max_blocks = (4 + payload_max + msg_len - 1) / msg_len;
        Self {
            fec,
            buffer: Vec::new(),
            frames_received: 0,
            frames_valid: 0,
            crc_fail: 0,
            fec_fail: 0,
            max_blocks,
        }
    }

    pub fn feed_bytes(&mut self, data: &[u8]) -> Vec<ParsedFrame> {
        self.buffer.extend_from_slice(data);
        let mut frames = Vec::new();

        loop {
            let sync_idx = self.find_sync();
            match sync_idx {
                Some(idx) => {
                    if idx > 0 {
                        self.buffer.drain(..idx);
                    }
                    if let Some(frame) = self.try_extract_frame() {
                        self.frames_received += 1;
                        if frame.valid {
                            self.frames_valid += 1;
                        }
                        frames.push(frame);
                    } else {
                        if self.buffer.len() > SYNC_PATTERN.len() + 4 {
                            self.buffer.drain(0..1);
                        } else {
                            break;
                        }
                    }
                }
                None => {
                    if self.buffer.len() > SYNC_PATTERN.len() + 4 {
                        let drain_to = self.buffer.len() - SYNC_PATTERN.len() - 4;
                        self.buffer.drain(..drain_to);
                    }
                    break;
                }
            }
        }

        frames
    }

    fn find_sync(&self) -> Option<usize> {
        if self.buffer.len() < SYNC_PATTERN.len() {
            return None;
        }
        for i in 0..=self.buffer.len().saturating_sub(SYNC_PATTERN.len()) {
            let mut matches = 0;
            for j in 0..SYNC_PATTERN.len() {
                if self.buffer[i + j] == SYNC_PATTERN[j] {
                    matches += 1;
                }
            }
            if matches >= 4 {
                return Some(i);
            }
        }
        None
    }

    fn try_extract_frame(&mut self) -> Option<ParsedFrame> {
        let sync_offset = 0usize;
        if self.buffer.len() < sync_offset + SYNC_PATTERN.len() + 4 {
            return None;
        }

        let sync_end = sync_offset + SYNC_PATTERN.len();
        let block_len = 255usize;

        for n_blocks in 1..=self.max_blocks {
            let total_fec = n_blocks * block_len;
            let data_end = sync_end + total_fec;
            let full_frame_end = data_end + 4;
            if full_frame_end > self.buffer.len() {
                break;
            }

            let crc_region = &self.buffer[..data_end];
            let crc_bytes = &self.buffer[data_end..full_frame_end];
            let fec_data = &self.buffer[sync_end..data_end];

            if crc::verify_crc32(crc_region, crc_bytes) {
                let (decoded, _success) = self.fec.decode(fec_data);
                if decoded.len() >= 4 {
                    let frame_type = (decoded[0] >> 4) & 0x0F;
                    let payload_len =
                        ((decoded[0] as u16 & 0x0F) << 8) | decoded[1] as u16;
                    let seq_num = ((decoded[2] as u16) << 8) | decoded[3] as u16;
                    let payload_end = 4 + payload_len as usize;
                    let actual_payload = if payload_end <= decoded.len() {
                        decoded[4..payload_end].to_vec()
                    } else {
                        Vec::new()
                    };

                    self.buffer.drain(..full_frame_end);
                    return Some(ParsedFrame {
                        payload: actual_payload,
                        frame_type,
                        sequence_number: seq_num,
                        valid: true,
                    });
                }
            } else {
                let (decoded, success) = self.fec.decode(fec_data);
                if decoded.len() >= 4 {
                    let reencoded = self.fec.encode(&decoded);
                    let mut crc_check = Vec::from(&self.buffer[..sync_end]);
                    crc_check.extend_from_slice(&reencoded);
                    if crc::verify_crc32(&crc_check, crc_bytes) {
                        let frame_type = (decoded[0] >> 4) & 0x0F;
                        let payload_len =
                            ((decoded[0] as u16 & 0x0F) << 8) | decoded[1] as u16;
                        let seq_num = ((decoded[2] as u16) << 8) | decoded[3] as u16;
                        let payload_end = 4 + payload_len as usize;
                        let actual_payload = if payload_end <= decoded.len() {
                            decoded[4..payload_end].to_vec()
                        } else {
                            Vec::new()
                        };

                        if !success {
                            self.fec_fail += 1;
                        }

                        self.buffer.drain(..full_frame_end);
                        return Some(ParsedFrame {
                            payload: actual_payload,
                            frame_type,
                            sequence_number: seq_num,
                            valid: success,
                        });
                    }
                }
            }
        }

        None
    }

    pub fn stats_summary(&self) -> String {
        format!(
            "Frames: {}/{} valid, {} crc fail, {} fec fail",
            self.frames_valid, self.frames_received, self.crc_fail, self.fec_fail
        )
    }
}
