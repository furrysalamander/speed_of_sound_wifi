use rand::Rng;
use rand::SeedableRng;
use sosw_core::config::Config;
use sosw_core::physical::ofdm_demod::OfdmDemodulator;
use sosw_core::physical::ofdm_mod::OfdmModulator;

#[test]
fn test_single_frame_roundtrip() {
    let config = Config::ofdm_default();
    let mut modulator = OfdmModulator::new(&config);
    let mut demodulator = OfdmDemodulator::new(&config);

    let payload_size = config.payload_size;
    let payload: Vec<u8> = (0..payload_size).map(|i| (i % 256) as u8).collect();

    let audio = modulator.modulate_with_preamble(&payload);
    assert!(!audio.is_empty(), "modulated audio should not be empty");

    let silence_len = config.symbol_duration_samples() * 4;
    let mut padded = audio;
    padded.extend(std::iter::repeat(0.0f32).take(silence_len));

    let result = demodulator.process_samples(&padded);
    assert!(result.is_some(), "demodulator should detect and decode the frame");
    let result = result.unwrap();

    assert!(
        result.preamble_peak > config.preamble_threshold,
        "preamble peak should exceed threshold"
    );

    let check_len = payload.len().min(result.bytes.len());
    assert_eq!(
        &result.bytes[..check_len],
        &payload[..check_len],
        "demodulated bytes should match original payload"
    );
}

#[test]
fn test_multi_frame_roundtrip() {
    let config = Config::ofdm_default();
    let mut modulator = OfdmModulator::new(&config);

    let n_frames = 5usize;
    let ps = config.payload_size;
    let mut all_payload = Vec::with_capacity(n_frames * ps);
    for i in 0..n_frames {
        let frame_data: Vec<u8> = (0..ps).map(|j| ((j + i) % 256) as u8).collect();
        all_payload.extend_from_slice(&frame_data);
    }

    let bits_per_sym = config.active_subcarriers() * 2;
    let data_syms_per_frame = (ps * 8 + bits_per_sym - 1) / bits_per_sym;
    let frame_len = (config.preamble_symbols + data_syms_per_frame) * config.symbol_duration_samples();

    let mut audio = Vec::new();
    for chunk in all_payload.chunks(ps) {
        let frame_audio = modulator.modulate_with_preamble(chunk);
        audio.extend_from_slice(&frame_audio);
    }
    assert!(
        audio.len() >= n_frames * frame_len,
        "audio too short: {} < {}",
        audio.len(),
        n_frames * frame_len
    );

    let mut demodulator = OfdmDemodulator::new(&config);
    let mut received = Vec::new();
    let mut search_pos = 0usize;

    for _ in 0..n_frames {
        if search_pos >= audio.len() {
            break;
        }
        let chunk_end = std::cmp::min(search_pos + frame_len + config.symbol_duration_samples(), audio.len());
        let mut chunk: Vec<f32> = audio[search_pos..chunk_end].to_vec();
        chunk.extend(std::iter::repeat(0.0f32).take(config.symbol_duration_samples() * 6));

        if chunk.len() < config.preamble_samples() + config.symbol_duration_samples() {
            break;
        }

        let result = demodulator.process_samples(&chunk);
        if let Some(r) = result {
            let n_frame_bytes = std::cmp::min(r.bytes.len(), ps);
            received.extend_from_slice(&r.bytes[..n_frame_bytes]);
        }
        search_pos += frame_len;
        demodulator.reset();
    }

    assert!(!received.is_empty(), "should have received at least some data");
    assert_eq!(
        received.len(),
        all_payload.len(),
        "multi-frame: expected {} bytes, got {}",
        all_payload.len(),
        received.len()
    );
    assert_eq!(
        &received[..],
        &all_payload[..],
        "multi-frame: payload mismatch"
    );
}

#[test]
fn test_empty_payload() {
    let config = Config::ofdm_default();
    let mut modulator = OfdmModulator::new(&config);
    let mut demodulator = OfdmDemodulator::new(&config);

    let audio = modulator.modulate_with_preamble(&[]);
    let silence: Vec<f32> = std::iter::repeat(0.0f32)
        .take(config.frame_samples())
        .collect();
    let mut padded = audio;
    padded.extend(silence);

    let result = demodulator.process_samples(&padded);
    assert!(result.is_some(), "should still produce output for empty payload");
    if let Some(r) = result {
        assert!(!r.bytes.is_empty(), "should produce some bits");
    }
}

#[test]
fn test_noise_resistance() {
    let config = Config::ofdm_default();
    let mut modulator = OfdmModulator::new(&config);
    let mut demodulator = OfdmDemodulator::new(&config);

    let payload_size = config.payload_size / 4;
    let payload: Vec<u8> = (0..payload_size).map(|i| (i % 256) as u8).collect();
    let mut audio = modulator.modulate_with_preamble(&payload);

    let snr_db = 10.0f32;
    let signal_power: f32 = audio.iter().map(|&s| s * s).sum::<f32>() / audio.len() as f32;
    let noise_std = (signal_power / 10.0f32.powf(snr_db / 10.0)).sqrt();

    let mut rng = rand_chacha::ChaCha12Rng::seed_from_u64(999);
    for s in audio.iter_mut() {
        let n: f32 = rng.random::<f32>() * 2.0 - 1.0;
        *s += n * noise_std;
    }

    let silence: Vec<f32> = std::iter::repeat(0.0f32)
        .take(config.symbol_duration_samples() * 4)
        .collect();
    audio.extend(silence);

    let result = demodulator.process_samples(&audio);
    assert!(result.is_some(), "should decode with noise");
    if let Some(r) = result {
        let check_len = payload.len().min(r.bytes.len());
        let matches = r.bytes[..check_len]
            .iter()
            .zip(payload[..check_len].iter())
            .filter(|(a, b)| a == b)
            .count();
        assert!(
            matches >= check_len * 90 / 100,
            "noise test: only {}/{} bytes match at {} dB SNR",
            matches,
            check_len,
            snr_db
        );
    }
}
