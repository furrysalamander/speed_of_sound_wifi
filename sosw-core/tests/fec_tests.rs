use sosw_core::link::fec::ReedSolomonFec;

#[test]
fn test_fec_roundtrip() {
    let fec = ReedSolomonFec::new(32);
    let msg_len = fec.max_data_bytes();

    let data: Vec<u8> = (0..msg_len as u8).collect();
    let encoded = fec.encode(&data);
    assert_eq!(encoded.len(), data.len() + 32, "encoded should add nsym parity bytes");

    let (decoded, success) = fec.decode(&encoded);
    assert!(success, "decode should succeed");
    assert_eq!(&decoded[..msg_len], &data[..], "decoded data should match original");
}

#[test]
fn test_fec_error_correction() {
    let fec = ReedSolomonFec::new(32);
    let msg_len = fec.max_data_bytes();

    let data: Vec<u8> = (0..msg_len as u8).collect();
    let mut encoded = fec.encode(&data);

    // Corrupt the first few bytes
    let n_errors = 8usize;
    for i in 0..n_errors.min(encoded.len()) {
        encoded[i] = encoded[i].wrapping_add(0x55);
    }

    let (decoded, success) = fec.decode(&encoded);
    assert!(success, "decode should correct {} byte errors", n_errors);
    assert_eq!(&decoded[..msg_len], &data[..], "should correct all errors");
}

#[test]
fn test_fec_large_data() {
    let fec = ReedSolomonFec::new(32);
    let msg_len = fec.max_data_bytes();

    let data: Vec<u8> = (0..msg_len * 3).map(|i| (i % 256) as u8).collect();
    let encoded = fec.encode(&data);
    let expected_len = data.len() + 3 * 32;
    assert_eq!(encoded.len(), expected_len, "3 blocks = 3*32 parity bytes");

    let (decoded, success) = fec.decode(&encoded);
    assert!(success, "multi-block decode should succeed");

    let n_check = std::cmp::min(decoded.len(), data.len());
    assert_eq!(&decoded[..n_check], &data[..n_check], "multi-block data should match");
}

#[test]
fn test_fec_max_errors() {
    let fec = ReedSolomonFec::new(32);
    let msg_len = fec.max_data_bytes();

    let data: Vec<u8> = (0..msg_len as u8).collect();
    let mut encoded = fec.encode(&data);

    // 16 errors should be correctable (nsym/2)
    let n_errors = 16usize;
    for i in 0..n_errors.min(encoded.len()) {
        encoded[i] = encoded[i].wrapping_add(0xAA);
    }

    let (decoded, success) = fec.decode(&encoded);
    assert!(success, "decode should correct {} errors (nsym/2)", n_errors);
    assert_eq!(&decoded[..msg_len], &data[..], "should correct {} byte errors", n_errors);
}

#[test]
fn test_fec_too_many_errors() {
    let fec = ReedSolomonFec::new(32);
    let msg_len = fec.max_data_bytes();

    let data: Vec<u8> = (0..msg_len as u8).collect();
    let mut encoded = fec.encode(&data);

    // 24 errors exceeds nsym/2, should fail
    let n_errors = 24usize;
    for i in 0..n_errors.min(encoded.len()) {
        encoded[i] = encoded[i].wrapping_add(0xFF);
    }

    let (decoded, success) = fec.decode(&encoded);
    assert!(!success, "decode should report failure for {} errors", n_errors);
    assert_eq!(decoded.len(), msg_len, "decoded should have msg_len bytes even on failure");
}
