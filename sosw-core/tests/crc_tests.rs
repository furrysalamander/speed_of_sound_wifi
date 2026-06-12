use sosw_core::link::crc;

#[test]
fn test_crc_roundtrip() {
    let data = b"Hello, OFDM!";
    let crc_val = crc::compute_crc32(data);

    let mut with_crc = data.to_vec();
    crc::compute_and_append_crc(&mut with_crc);
    assert_eq!(with_crc.len(), data.len() + 4);

    let stored = u32::from_be_bytes([
        with_crc[data.len()],
        with_crc[data.len() + 1],
        with_crc[data.len() + 2],
        with_crc[data.len() + 3],
    ]);
    assert_eq!(stored, crc_val);

    let mut verify_data = with_crc.clone();
    assert!(crc::verify_and_strip_crc(&mut verify_data));
    assert_eq!(&verify_data, data);
}

#[test]
fn test_crc_detects_corruption() {
    let data = b"Hello, OFDM!";
    let crc_bytes = crc::compute_crc32(data).to_be_bytes();
    assert!(crc::verify_crc32(data, &crc_bytes));

    let mut corrupted = data.to_vec();
    corrupted[5] = corrupted[5].wrapping_add(1);
    assert!(!crc::verify_crc32(&corrupted, &crc_bytes));
}

#[test]
fn test_crc_bitflip_in_crc() {
    let data = b"Hello, OFDM!";
    let mut crc_bytes = crc::compute_crc32(data).to_be_bytes();
    assert!(crc::verify_crc32(data, &crc_bytes));

    crc_bytes[2] = crc_bytes[2].wrapping_add(0x80);
    assert!(!crc::verify_crc32(data, &crc_bytes));
}

#[test]
fn test_crc_empty_data() {
    let data: &[u8] = b"";
    let crc_val = crc::compute_crc32(data);
    let crc_bytes = crc_val.to_be_bytes();
    assert!(crc::verify_crc32(data, &crc_bytes));
}

#[test]
fn test_crc_single_byte() {
    let data = &[0xAB];
    let crc_bytes = crc::compute_crc32(data).to_be_bytes();
    assert!(crc::verify_crc32(data, &crc_bytes));

    let mut with_crc = data.to_vec();
    crc::compute_and_append_crc(&mut with_crc);
    assert_eq!(with_crc.len(), 5);

    let mut verify_data = with_crc.clone();
    assert!(crc::verify_and_strip_crc(&mut verify_data));
    assert_eq!(verify_data, data);
}

#[test]
fn test_crc_large_data() {
    let data: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
    let crc_bytes = crc::compute_crc32(&data).to_be_bytes();
    assert!(crc::verify_crc32(&data, &crc_bytes));
}

#[test]
fn test_crc_truncated_data() {
    // verify_and_strip_crc on data < 4 bytes should return false
    let mut short = vec![0x01, 0x02, 0x03];
    assert!(!crc::verify_and_strip_crc(&mut short));

    // verify_crc32 on crc_bytes < 4 should return false
    assert!(!crc::verify_crc32(b"data", &[0x00, 0x00, 0x00]));
}

#[test]
fn test_crc_all_zeros() {
    let data = [0u8; 64];
    let crc_bytes = crc::compute_crc32(&data).to_be_bytes();
    assert!(crc::verify_crc32(&data, &crc_bytes));
}

#[test]
fn test_crc_all_ones() {
    let data = [0xFFu8; 64];
    let crc_bytes = crc::compute_crc32(&data).to_be_bytes();
    assert!(crc::verify_crc32(&data, &crc_bytes));
}
