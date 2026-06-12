use sosw_core::config::Config;
use sosw_core::link::crc;
use sosw_core::link::frame::{FrameAssembler, FrameParser};

#[test]
fn test_crc_roundtrip() {
    let data = b"Hello, OFDM!";
    let _crc = crc::compute_crc32(data);

    let mut with_crc = data.to_vec();
    crc::compute_and_append_crc(&mut with_crc);
    assert_eq!(with_crc.len(), data.len() + 4);

    let mut verify_data = with_crc.clone();
    assert!(crc::verify_and_strip_crc(&mut verify_data), "CRC verification should pass");
    assert_eq!(&verify_data, data, "CRC should be stripped correctly");
}

#[test]
fn test_frame_assemble_parse() {
    let config = Config::ofdm_default();
    let mut assembler = FrameAssembler::new(&config);
    let mut parser = FrameParser::new(&config);

    let payload: Vec<u8> = (0..config.payload_size).map(|i| (i % 256) as u8).collect();
    let frame = assembler.assemble_frame(&payload);

    let frames = parser.feed_bytes(&frame);
    assert_eq!(frames.len(), 1, "should parse exactly one frame");
    assert!(frames[0].valid, "frame should be valid");
    assert_eq!(frames[0].payload, payload, "payload should match");
    assert_eq!(frames[0].sequence_number, 0, "first frame seq=0");
}

#[test]
fn test_frame_multiple_sequence() {
    let config = Config::ofdm_default();
    let mut assembler = FrameAssembler::new(&config);
    let mut parser = FrameParser::new(&config);

    let payloads: Vec<Vec<u8>> = (0..3)
        .map(|i| {
            let p: Vec<u8> = (0..config.payload_size).map(|j| ((j + i * 64) % 256) as u8).collect();
            p
        })
        .collect();

    let mut all_data = Vec::new();
    for p in &payloads {
        let frame = assembler.assemble_frame(p);
        all_data.extend_from_slice(&frame);
    }

    let frames = parser.feed_bytes(&all_data);
    assert_eq!(frames.len(), 3, "should parse 3 frames");
    for (i, f) in frames.iter().enumerate() {
        assert!(f.valid, "frame {} should be valid", i);
        assert_eq!(f.sequence_number, i as u16, "frame {} seq should be {}", i, i);
        assert_eq!(&f.payload, &payloads[i], "frame {} payload mismatch", i);
    }
}

#[test]
fn test_frame_with_noise() {
    let config = Config::ofdm_default();
    let mut assembler = FrameAssembler::new(&config);
    let mut parser = FrameParser::new(&config);

    let payload: Vec<u8> = vec![0x42; 100];
    let frame = assembler.assemble_frame(&payload);

    // Add some byte noise before the frame
    let noisy: Vec<u8> = vec![0xAA, 0xBB, 0xCC, 0xDD];
    let mut input = noisy.clone();
    input.extend_from_slice(&frame);

    let frames = parser.feed_bytes(&input);
    assert_eq!(frames.len(), 1, "should find frame despite prefix noise");
    assert!(frames[0].valid, "frame should be valid");
    assert_eq!(&frames[0].payload[..100], &payload[..], "payload should match after noise");
}

#[test]
fn test_frame_empty_payload() {
    let config = Config::ofdm_default();
    let mut assembler = FrameAssembler::new(&config);
    let mut parser = FrameParser::new(&config);

    let empty_frame = assembler.assemble_frame(&[]);
    let frames = parser.feed_bytes(&empty_frame);
    assert_eq!(frames.len(), 1, "should parse empty-payload frame");
    assert!(frames[0].valid, "empty frame should be valid");
}
