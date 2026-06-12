use crc::{Crc, CRC_32_ISO_HDLC};

const CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);

pub fn compute_crc32(data: &[u8]) -> u32 {
    CRC32.checksum(data)
}

pub fn compute_and_append_crc(data: &mut Vec<u8>) {
    let crc = compute_crc32(data);
    data.extend_from_slice(&crc.to_be_bytes());
}

pub fn verify_and_strip_crc(data: &mut Vec<u8>) -> bool {
    if data.len() < 4 {
        return false;
    }
    let payload_len = data.len() - 4;
    let stored_crc = u32::from_be_bytes([
        data[payload_len],
        data[payload_len + 1],
        data[payload_len + 2],
        data[payload_len + 3],
    ]);
    let computed = compute_crc32(&data[..payload_len]);
    if computed == stored_crc {
        data.truncate(payload_len);
        true
    } else {
        false
    }
}

pub fn verify_crc32(data: &[u8], crc_bytes: &[u8]) -> bool {
    if crc_bytes.len() < 4 {
        return false;
    }
    let stored = u32::from_be_bytes([crc_bytes[0], crc_bytes[1], crc_bytes[2], crc_bytes[3]]);
    compute_crc32(data) == stored
}
