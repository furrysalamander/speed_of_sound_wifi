//! Link-training handshake over the DTMF control channel.
//!
//! Wideband OFDM is used for data, but it needs the microphone AGC to be
//! settled and both ends to agree on parameters before it is usable. This
//! module provides a slow, robust DTMF-based handshake:
//!
//! ```text
//!   A                                      B
//!   |-- HELLO(node_id, param) ------------->|
//!   |<-- HELLO_ACK(node_id, param) ---------|
//!   |-- TRAIN(n) -------------------------->|
//!   |<-- TRAIN_ACK(received n) -------------|
//! ```
//!
//! Each message is 3 nibbles (`kind, node_id, param`) plus a sync and a
//! checksum. Every nibble is transmitted `REPEAT` times and recovered by
//! majority vote, so a single corrupted symbol (common on consumer mics)
//! cannot break the frame.

use sosw_core::physical::dtmf;

pub const SYNC: u8 = 0xE;
/// How many times each nibble is repeated on the air.
pub const REPEAT: usize = 3;
pub const MSG_HELLO: u8 = 0x0;
pub const MSG_HELLO_ACK: u8 = 0x1;
pub const MSG_TRAIN: u8 = 0x2;
pub const MSG_TRAIN_ACK: u8 = 0x3;
pub const MSG_BYE: u8 = 0x4;

/// A decoded handshake message. `text` is kept for API compatibility but is
/// not transmitted (the handshake only needs the three numeric fields).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub kind: u8,
    pub node_id: u8,
    pub param: u8,
    pub text: String,
}

impl Message {
    pub fn hello(node_id: u8, param: u8, text: &str) -> Self {
        Self { kind: MSG_HELLO, node_id, param, text: text.to_string() }
    }
    pub fn hello_ack(node_id: u8, param: u8, text: &str) -> Self {
        Self { kind: MSG_HELLO_ACK, node_id, param, text: text.to_string() }
    }
    pub fn train(node_id: u8, count: u8) -> Self {
        Self { kind: MSG_TRAIN, node_id, param: count, text: String::new() }
    }
    pub fn train_ack(node_id: u8, count: u8) -> Self {
        Self { kind: MSG_TRAIN_ACK, node_id, param: count, text: String::new() }
    }
}

/// Nibble-level checksum: detects a mis-recovered symbol.
fn checksum(nibbles: &[u8]) -> u8 {
    nibbles.iter().fold(0u8, |acc, &n| (acc.wrapping_add(n).wrapping_mul(3)) & 0x0F) ^ 0x5
}

/// The 7 logical nibbles of a message:
/// `[SYNC, kind, node_hi, node_lo, param_hi, param_lo, crc]`.
fn logical_nibbles(msg: &Message) -> [u8; 7] {
    let head = [
        SYNC,
        msg.kind & 0x0F,
        (msg.node_id >> 4) & 0x0F,
        msg.node_id & 0x0F,
        (msg.param >> 4) & 0x0F,
        msg.param & 0x0F,
    ];
    let crc = checksum(&head);
    [head[0], head[1], head[2], head[3], head[4], head[5], crc]
}


/// Encode a message into DTMF audio. The 7-symbol frame is repeated
/// `REPEAT` times; the decoder scans the received stream at every offset for a
/// checksum-valid frame, so inserted or dropped symbols do not desynchronize
/// it as long as one clean copy survives.
pub fn encode_message(msg: &Message, cfg: &dtmf::DtmfConfig) -> Vec<f32> {
    let logical = logical_nibbles(msg);
    let mut repeated = Vec::with_capacity(logical.len() * REPEAT);
    for _ in 0..REPEAT {
        repeated.extend_from_slice(&logical);
    }
    dtmf::encode(&repeated, cfg)
}

/// The repeated symbol sequence for a message, for tests and diagnostics.
pub fn encode_symbols(msg: &Message) -> Vec<u8> {
    let logical = logical_nibbles(msg);
    let mut out = Vec::with_capacity(logical.len() * REPEAT);
    for _ in 0..REPEAT {
        out.extend_from_slice(&logical);
    }
    out
}

/// Try to frame a message at any offset of a symbol stream.
/// Frame layout: `[SYNC, kind, node_hi, node_lo, param_hi, param_lo, crc]`.
fn scan_frame(symbols: &[u8]) -> Option<(Message, usize)> {
    if symbols.len() < 7 {
        return None;
    }
    for start in 0..=symbols.len() - 7 {
        if symbols[start] != SYNC {
            continue;
        }
        let f = &symbols[start..start + 7];
        if checksum(&f[..6]) != f[6] {
            continue;
        }
        return Some((
            Message {
                kind: f[1],
                node_id: (f[2] << 4) | f[3],
                param: (f[4] << 4) | f[5],
                text: String::new(),
            },
            start + 7,
        ));
    }
    None
}

/// Decode the first valid message in a received symbol stream.
pub fn decode_message(symbols: &[u8]) -> Option<(Message, usize)> {
    scan_frame(symbols)
}

/// Decode every valid message in a symbol stream, skipping past each match.
pub fn decode_all(symbols: &[u8]) -> Vec<(Message, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 7 <= symbols.len() {
        if let Some((msg, next)) = scan_frame(&symbols[i..]) {
            if !out.iter().any(|(m, _)| m == &msg) {
                out.push((msg, i + next));
            }
            i += next.max(1);
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(msg: Message) -> Message {
        let cfg = dtmf::DtmfConfig::default();
        let audio = encode_message(&msg, &cfg);
        let symbols = dtmf::decode(&audio, &cfg);
        let (decoded, _) = decode_message(&symbols)
            .unwrap_or_else(|| panic!("no message decoded; symbols={:X?}", symbols));
        decoded
    }

    #[test]
    fn test_hello_roundtrip_clean() {
        let m = rt(Message::hello(7, 0, "sosw"));
        assert_eq!((m.kind, m.node_id, m.param), (MSG_HELLO, 7, 0));
    }

    #[test]
    fn test_train_roundtrip_clean() {
        let m = rt(Message::train_ack(42, 100));
        assert_eq!((m.kind, m.node_id, m.param), (MSG_TRAIN_ACK, 42, 100));
    }

    #[test]
    fn test_payload_starting_with_sync() {
        let m = rt(Message::hello_ack(0xEE, 0, "x"));
        assert_eq!((m.kind, m.node_id, m.param), (MSG_HELLO_ACK, 0xEE, 0));
    }

    #[test]
    fn test_leading_garbage_ignored() {
        let mut symbols = vec![0x1, 0x2, 0x3];
        symbols.extend(encode_symbols(&Message::train(3, 10)));
        let (decoded, _) = decode_message(&symbols).expect("no message decoded");
        assert_eq!((decoded.kind, decoded.node_id, decoded.param), (MSG_TRAIN, 3, 10));
    }

    /// Simulate a symbol deletion, the failure mode we actually see on the
    /// acoustic channel. One clean copy of the frame must still recover.
    #[test]
    fn test_dropped_symbol_recovered() {
        let msg = Message::hello_ack(2, 8, "");
        for drop in 0..encode_symbols(&msg).len() {
            let mut s = encode_symbols(&msg);
            s.remove(drop);
            let got = decode_all(&s)
                .into_iter()
                .find(|(m, _)| (m.kind, m.node_id, m.param) == (msg.kind, msg.node_id, msg.param));
            assert!(got.is_some(), "drop at {} broke decode: {:X?}", drop, s);
        }
    }

    /// Simulate an inserted spurious symbol (a false DTMF detection).
    #[test]
    fn test_inserted_symbol_recovered() {
        let msg = Message::train(1, 8);
        for ins in 0..encode_symbols(&msg).len() {
            let mut s = encode_symbols(&msg);
            s.insert(ins, 0x4);
            let got = decode_all(&s)
                .into_iter()
                .find(|(m, _)| (m.kind, m.node_id, m.param) == (msg.kind, msg.node_id, msg.param));
            assert!(got.is_some(), "insert at {} broke decode: {:X?}", ins, s);
        }
    }

    /// Simulate a single corrupted (substituted) symbol.
    #[test]
    fn test_substituted_symbol_recovered() {
        let msg = Message::hello(1, 8, "");
        let n = encode_symbols(&msg).len();
        for flip in 0..n {
            let mut s = encode_symbols(&msg);
            s[flip] = (s[flip] + 1) & 0x0F;
            let got = decode_all(&s)
                .into_iter()
                .find(|(m, _)| (m.kind, m.node_id, m.param) == (msg.kind, msg.node_id, msg.param));
            assert!(got.is_some(), "flip at {} broke decode: {:X?}", flip, s);
        }
    }
}

#[cfg(test)]
mod audio_tests {
    use super::*;
    use sosw_core::physical::dtmf;

    #[test]
    fn all_message_kinds_roundtrip_through_audio() {
        let cfg = dtmf::DtmfConfig::default();
        for msg in [
            Message::hello(1, 8, "sosw"),
            Message::hello_ack(2, 8, "sosw"),
            Message::train(1, 8),
            Message::train_ack(2, 8),
        ] {
            let audio = encode_message(&msg, &cfg);
            let syms = dtmf::decode(&audio, &cfg);
            let decoded = decode_all(&syms);
            assert!(
                decoded.iter().any(|(m, _)| (m.kind, m.node_id, m.param)
                    == (msg.kind, msg.node_id, msg.param)),
                "roundtrip failed; syms={:X?}",
                syms
            );
        }
    }
}
