use sosw_core::link::scrambler::Scrambler;

#[test]
fn test_scrambler_roundtrip() {
    let scrambler = Scrambler::new(12345);
    let data = b"Speed of Sound WiFi!";
    let scrambled = scrambler.scramble(data);
    let descrambled = scrambler.descramble(&scrambled);
    assert_eq!(&descrambled, data, "scramble then descramble should return original");
}

#[test]
fn test_scrambler_deterministic() {
    let s1 = Scrambler::new(42);
    let s2 = Scrambler::new(42);
    let data: Vec<u8> = (0..256).map(|i| i as u8).collect();
    assert_eq!(s1.scramble(&data), s2.scramble(&data), "same seed should produce same output");
}

#[test]
fn test_scrambler_different_seeds() {
    let s1 = Scrambler::new(1);
    let s2 = Scrambler::new(2);
    let data: Vec<u8> = (0..256).map(|i| i as u8).collect();
    let out1 = s1.scramble(&data);
    let out2 = s2.scramble(&data);
    assert_ne!(out1, out2, "different seeds should produce different output");
}

#[test]
fn test_scrambler_empty() {
    let scrambler = Scrambler::new(12345);
    let scrambled = scrambler.scramble(b"");
    assert!(scrambled.is_empty(), "empty input -> empty output");
    let descrambled = scrambler.descramble(&scrambled);
    assert!(descrambled.is_empty());
}

#[test]
fn test_scrambler_single_byte() {
    let scrambler = Scrambler::new(9999);
    for &b in &[0x00u8, 0xFF, 0x55, 0xAA, 0x42] {
        let scrambled = scrambler.scramble(&[b]);
        assert_eq!(scrambled.len(), 1, "scrambled should be 1 byte");
        let descrambled = scrambler.descramble(&scrambled);
        assert_eq!(descrambled[0], b);
    }
}

#[test]
fn test_scrambler_all_zeros() {
    let scrambler = Scrambler::new(7777);
    let data = [0u8; 128];
    let scrambled = scrambler.scramble(&data);
    // Scrambling zeros should produce non-zero output (unless RNG outputs 0 for every byte, astronomically unlikely)
    assert!(
        scrambled.iter().any(|&b| b != 0),
        "scrambling zeros should produce non-zero output"
    );
    let descrambled = scrambler.descramble(&scrambled);
    assert_eq!(&descrambled, &data);
}

#[test]
fn test_scrambler_large() {
    let scrambler = Scrambler::new(42);
    let data: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
    let scrambled = scrambler.scramble(&data);
    assert_eq!(scrambled.len(), data.len());
    let descrambled = scrambler.descramble(&scrambled);
    assert_eq!(&descrambled, &data);
}

#[test]
fn test_scrambler_mask_length() {
    let scrambler = Scrambler::new(12345);
    let mask = scrambler.mask(100);
    assert_eq!(mask.len(), 100);
    let mask_again = scrambler.mask(100);
    assert_eq!(mask, mask_again, "mask should be deterministic");
}

#[test]
fn test_scrambler_xor_identity() {
    let scrambler = Scrambler::new(12345);
    let data = b"Rust FTW!";
    let scrambled = scrambler.scramble(data);
    // scrambling twice = XOR mask twice = identity
    let descrambled = scrambler.scramble(&scrambled);
    assert_eq!(&descrambled, data);
}
