use sosw_core::link::fec::ReedSolomonFec;

#[test]
fn test_fec_partial_final_block() {
    let fec = ReedSolomonFec::new(32);
    let msg_len = fec.max_data_bytes();

    // Data that doesn't fill the last block: msg_len + 50 bytes
    let data: Vec<u8> = (0..msg_len + 50).map(|i| (i % 256) as u8).collect();
    let encoded = fec.encode(&data);
    let n_blocks = (data.len() + msg_len - 1) / msg_len;
    assert_eq!(encoded.len(), n_blocks * 255, "encoder outputs whole 255-byte blocks");

    let (decoded, success) = fec.decode(&encoded);
    assert!(success, "decode should succeed");
    // decoded has msg_len*2 bytes (padded), but the first data.len() should match
    assert_eq!(&decoded[..data.len()], &data[..], "partial final block data mismatch");
}

#[test]
fn test_fec_single_byte_data() {
    let fec = ReedSolomonFec::new(32);
    let msg_len = fec.max_data_bytes();

    let data = vec![0xABu8];
    let encoded = fec.encode(&data);
    assert_eq!(encoded.len(), 255, "single byte encoded as 1 whole block of 255 bytes");

    let (decoded, success) = fec.decode(&encoded);
    assert!(success);
    assert_eq!(decoded[0], data[0]);
    // decoded has msg_len bytes, rest are padding zeros
    assert_eq!(decoded.len(), msg_len);
}

#[test]
fn test_fec_errors_in_parity_only() {
    let fec = ReedSolomonFec::new(32);
    let msg_len = fec.max_data_bytes();

    let data: Vec<u8> = (0..msg_len as u8).collect();
    let mut encoded = fec.encode(&data);
    let parity_start = msg_len;
    // Corrupt 8 parity bytes
    for i in 0..8 {
        encoded[parity_start + i] = encoded[parity_start + i].wrapping_add(0xAA);
    }

    let (decoded, success) = fec.decode(&encoded);
    assert!(success, "should correct parity-only errors");
    assert_eq!(&decoded[..msg_len], &data[..]);
}

#[test]
fn test_fec_burst_errors() {
    let fec = ReedSolomonFec::new(32);
    let msg_len = fec.max_data_bytes();

    let data: Vec<u8> = (0..msg_len as u8).collect();
    let mut encoded = fec.encode(&data);

    // Corrupt a contiguous 8-byte burst in the data section
    for i in 10..18 {
        encoded[i] = encoded[i].wrapping_add(0xFF);
    }

    let (decoded, success) = fec.decode(&encoded);
    assert!(success, "should correct burst errors");
    assert_eq!(&decoded[..msg_len], &data[..]);
}

#[test]
fn test_fec_header_corruption() {
    let fec = ReedSolomonFec::new(32);
    let _msg_len = fec.max_data_bytes();

    let mut data = vec![0u8; 10];
    // Simulate a 4-byte header
    data[0] = 0x01;
    data[1] = 0x00;
    data[2] = 0x00;
    data[3] = 0x05;
    let original = data.clone();

    let mut encoded = fec.encode(&data);
    // Corrupt first 4 bytes (simulated header)
    for i in 0..4 {
        encoded[i] = encoded[i].wrapping_add(0x55);
    }

    let (decoded, success) = fec.decode(&encoded);
    assert!(success, "should recover corrupted header bytes");
    assert_eq!(&decoded[..original.len()], &original[..]);
}

#[test]
fn test_fec_truncated_encoded_no_panic() {
    let fec = ReedSolomonFec::new(32);
    let msg_len = fec.max_data_bytes();

    let data: Vec<u8> = (0..msg_len as u8).collect();
    let mut encoded = fec.encode(&data);
    // Remove some ending parity bytes (truncated reception)
    encoded.truncate(encoded.len() - 10);

    let (decoded, _success) = fec.decode(&encoded);
    // Should not panic, and should produce msg_len bytes
    assert_eq!(decoded.len(), msg_len, "decoded should have msg_len bytes even when truncated");
}

#[test]
fn test_fec_multi_block_partial_last() {
    let fec = ReedSolomonFec::new(16);
    let msg_len = fec.max_data_bytes();

    // 2 full blocks + partial 3rd block
    let data: Vec<u8> = (0..msg_len * 2 + 100).map(|i| (i % 256) as u8).collect();
    let encoded = fec.encode(&data);
    let n_blocks = (data.len() + msg_len - 1) / msg_len;
    assert_eq!(encoded.len(), n_blocks * 255, "encoder outputs whole 255-byte blocks");

    let (decoded, success) = fec.decode(&encoded);
    assert!(success);
    assert_eq!(&decoded[..data.len()], &data[..]);
}

#[test]
fn test_fec_exact_fill() {
    let fec = ReedSolomonFec::new(32);
    let msg_len = fec.max_data_bytes();

    // Exactly msg_len bytes (fills one block exactly)
    let data: Vec<u8> = (0..msg_len).map(|i| (i % 256) as u8).collect();
    let encoded = fec.encode(&data);
    assert_eq!(encoded.len(), msg_len + 32);

    let (decoded, success) = fec.decode(&encoded);
    assert!(success);
    assert_eq!(&decoded[..msg_len], &data[..]);
}
