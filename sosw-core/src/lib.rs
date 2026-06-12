pub mod config;
pub mod link;
pub mod physical;

pub use config::Config;
pub use physical::ofdm_demod::{DemodResult, OfdmDemodulator};
pub use physical::ofdm_mod::OfdmModulator;
pub use link::frame::{FrameAssembler, FrameParser, ParsedFrame};
pub use link::fec::ReedSolomonFec;
pub use link::crc;

/// Trait for raw frame modulation (bytes -> audio samples).
pub trait Modulator {
    fn modulate_frame(&mut self, data: &[u8]) -> Vec<f32>;
}

/// Trait for raw frame demodulation (audio samples -> bytes + quality metrics).
pub trait Demodulator {
    fn process_samples(&mut self, samples: &[f32]) -> Option<DemodResult>;
    fn reset(&mut self);
}

impl Modulator for OfdmModulator {
    fn modulate_frame(&mut self, data: &[u8]) -> Vec<f32> {
        self.modulate_with_preamble(data)
    }
}

impl Demodulator for OfdmDemodulator {
    fn process_samples(&mut self, samples: &[f32]) -> Option<DemodResult> {
        self.process_samples(samples)
    }

    fn reset(&mut self) {
        self.reset();
    }
}

/// Bit manipulation helpers
pub fn bytes_to_bits(data: &[u8]) -> Vec<u8> {
    let mut bits = Vec::with_capacity(data.len() * 8);
    for &byte in data {
        for b in (0..8).rev() {
            bits.push((byte >> b) & 1);
        }
    }
    bits
}

pub fn bits_to_bytes(bits: &[u8]) -> Vec<u8> {
    let n_bytes = (bits.len() + 7) / 8;
    let mut bytes = Vec::with_capacity(n_bytes);
    for byte_idx in 0..n_bytes {
        let mut byte = 0u8;
        for bit_idx in 0..8 {
            let bit = bits.get(byte_idx * 8 + bit_idx).copied().unwrap_or(0);
            byte = (byte << 1) | bit;
        }
        bytes.push(byte);
    }
    bytes
}
