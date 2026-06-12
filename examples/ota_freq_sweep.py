"""Over-the-air frequency sweep for OFDM modem.

Tests different subcarrier ranges, FFT sizes, and CP lengths using
real speaker-to-microphone loopback. Reports frame detection rate
for each configuration.

Usage:
    python -m examples.ota_freq_sweep [--frames 10] [--fast]
"""

import argparse
import logging
import sys
import time

import numpy as np

from src.config import Config
from src.link.framing import FrameAssembler, FrameParser
from src.physical.ofdm import OfdmModulator, OfdmDemodulator

logging.basicConfig(level=logging.WARNING,
                    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
                    datefmt="%H:%M:%S")
logger = logging.getLogger("ota_sweep")


def _resolve_device(name, kind):
    import sounddevice as sd
    try:
        info = sd.query_devices(device=name, kind=kind)
        return info["index"]
    except (ValueError, TypeError):
        for i, d in enumerate(sd.query_devices()):
            if (kind == "input" and d["max_input_channels"] > 0
                    and name.lower() in d["name"].lower()):
                return i
            if (kind == "output" and d["max_output_channels"] > 0
                    and name.lower() in d["name"].lower()):
                return i
        raise ValueError(f"No {kind} device matching '{name}'")


def preamble_peak(rx, preamble):
    """Normalized cross-correlation peak."""
    xc = np.convolve(rx, preamble[::-1], mode="valid")
    we = np.sqrt(np.convolve(rx ** 2, np.ones(len(preamble)), mode="valid") + 1e-12)
    pe = np.sqrt(np.dot(preamble, preamble) + 1e-12)
    norm_xc = np.abs(xc) / (pe * we)
    return float(np.max(norm_xc))


def run_ota_test(config, n_frames=10, device_rx="USB_PnP", device_tx="analog-stereo"):
    import sounddevice as sd

    rx_idx = _resolve_device(device_rx, "input")
    tx_idx = _resolve_device(device_tx, "output")
    device_pair = (rx_idx, tx_idx)

    ps = config.frame.payload_size

    # Generate test frames
    modulator = OfdmModulator(config)
    assembler = FrameAssembler(config)
    frame_chunks = []
    for i in range(n_frames):
        p = bytes(b % 256 for b in range(i * ps, (i + 1) * ps))
        fb = assembler.assemble_frame(p)
        audio = modulator.modulate_with_preamble(fb)
        frame_chunks.append(audio)

    tx_audio = np.concatenate(frame_chunks)
    tx_audio = np.concatenate([np.zeros(48000), tx_audio, np.zeros(24000)])
    total_dur = len(tx_audio) / 48000.0

    # Full-duplex
    rx_buffer = []
    write_pos = [0]

    def callback(indata, outdata, frames, time_info, status):
        if status:
            logger.warning(f"Audio status: {status}")
        end = min(write_pos[0] + frames, len(tx_audio))
        n = end - write_pos[0]
        chunk = np.zeros(frames, dtype=np.float32)
        if n > 0:
            chunk[:n] = tx_audio[write_pos[0]:end]
        outdata[:] = chunk.reshape(-1, 1)
        rx_buffer.append(indata.copy())
        write_pos[0] = end

    stream = sd.Stream(
        samplerate=48000, blocksize=1024,
        device=device_pair, channels=1,
        callback=callback, dtype=np.float32,
    )
    stream.start()
    sd.sleep(int(total_dur * 1000) + 2000)
    stream.stop()
    stream.close()

    rx_audio = np.concatenate(rx_buffer).flatten()

    # Demodulate
    demod = OfdmDemodulator(config)
    parser = FrameParser(config)
    plen = demod.preamble_symbols * demod.sym_samples

    # Expected frame size
    nsym = config.fec.nsym
    data_len = 255 - nsym
    data_for_fec = 4 + config.frame.payload_size
    n_blocks = (data_for_fec + data_len - 1) // data_len
    frame_bytes = 8 + n_blocks * 255 + 4
    sc = config.ofdm.subcarrier_max - config.ofdm.subcarrier_min + 1
    bits_per_sym = config.ofdm.bits_per_subcarrier * sc
    data_syms = (frame_bytes * 8 + bits_per_sym - 1) // bits_per_sym
    frame_samples = plen + data_syms * demod.sym_samples

    preamble = demod._preamble_audio

    frames_detected = 0
    frames_valid = 0

    # Scan the first 3s for initial frame
    initial_search = min(len(rx_audio), int(48000 * 3))
    search_region = rx_audio[:initial_search]
    xc = np.convolve(search_region, preamble[::-1], mode="valid")
    we = np.sqrt(np.convolve(search_region ** 2, np.ones(len(preamble)), mode="valid") + 1e-12)
    pe = np.sqrt(np.dot(preamble, preamble) + 1e-12)
    norm_xc = np.abs(xc) / (pe * we)
    first_peak_idx = int(np.argmax(norm_xc))
    first_peak_val = float(norm_xc[first_peak_idx])
    logger.info(f"First preamble peak: {first_peak_val:.4f} at sample {first_peak_idx}")

    # Track frame by frame from first peak
    if first_peak_val >= 0.05:
        pos = first_peak_idx
        while pos + frame_samples < len(rx_audio):
            chunk = rx_audio[pos:pos + frame_samples]
            demod.reset()
            bits = demod.process_samples(chunk)
            success = False

            if bits is not None and len(bits) >= 8:
                byte_est = len(bits) // 8 + 64
                decoded = demod.symbols_to_bytes(bits, byte_est)
                frames = parser.feed_bytes(decoded)

                for payload_bytes, seq, ftype, valid in frames:
                    if ftype == 0:
                        frames_detected += 1
                        if valid:
                            frames_valid += 1
                        success = True
                        logger.info(f"  Frame #{seq}: det={frames_detected}, "
                                    f"valid={valid}")

            if success:
                pos += frame_samples
            elif frames_detected == 0:
                pos += int(demod.sym_samples * 0.5)
            else:
                pos += frame_samples
    else:
        logger.info("No preamble found in initial search window")

    match_pct = 100.0 * frames_valid / max(n_frames, 1)
    logger.info(f"Result: {frames_detected}/{n_frames} detected, "
                f"{frames_valid} valid ({match_pct:.1f}%)")

    return {
        "n_frames_sent": n_frames,
        "n_frames_detected": frames_detected,
        "n_frames_valid": frames_valid,
        "valid_pct": match_pct,
        "first_peak": first_peak_val,
        "frame_dur_s": frame_samples / 48000.0,
        "audio_dur_s": total_dur,
    }


def make_config(sc_min=10, sc_max=69, fft_size=256, cp_length=32,
                 payload_size=64, nsym=32):
    c = Config()
    c.modulation.use_ofdm = True
    c.ofdm.fft_size = fft_size
    c.ofdm.cp_length = cp_length
    c.ofdm.subcarrier_min = sc_min
    c.ofdm.subcarrier_max = sc_max
    c.frame.payload_size = payload_size
    c.fec.nsym = nsym
    return c


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--frames", type=int, default=10,
                        help="Frames per test")
    parser.add_argument("--device-rx", default="USB_PnP")
    parser.add_argument("--device-tx", default="analog-stereo")
    parser.add_argument("--fast", action="store_true",
                        help="Skip presets, only sweep SC ranges")
    args = parser.parse_args()

    # Frequency sub-band sweep (same FFT=256, CP=32, vary SC range)
    sweep_configs = [
        # name,         sc_min, sc_max, fft, cp, ps
        ("low_band",        1,  31,  256, 32, 64),   # 188–5.8 kHz
        ("mid_low",        10,  40,  256, 32, 64),   # 1.9–7.5 kHz
        ("mid",            30,  60,  256, 32, 64),   # 5.6–11.3 kHz
        ("mid_high",       50,  80,  256, 32, 64),   # 9.4–15.0 kHz
        ("high",           70, 100,  256, 32, 64),   # 13.1–18.8 kHz
        ("near_ultra",     90, 120,  256, 32, 64),   # 16.9–22.5 kHz
    ]

    if not args.fast:
        sweep_configs += [
            ("ultrasonic",     80, 120,  256, 32, 64),   # 15.0–22.5 kHz
            ("high_baud",       4,  31,  128, 16, 64),   # 333 sym/s
            ("robust",         20, 120,  512, 64, 64),   # 83 sym/s
            ("ultrawide",       5, 110,  256, 16, 64),   # 176 sym/s
        ]

    print(f"\n{'Config':<14} {'SC':<8} {'Freq (Hz)':<22} "
          f"{'SCs':<5} {'Sym/s':<8} {'Sent':<6} {'Valid':<6} {'%':<7} {'Peak'}")
    print("=" * 90)

    results = []
    for name, sc_min, sc_max, fft, cp, ps in sweep_configs:
        config = make_config(sc_min, sc_max, fft, cp, ps)
        fs = config.audio.sample_rate
        bin_w = fs / fft
        f_low = sc_min * bin_w
        f_high = sc_max * bin_w
        n_sc = sc_max - sc_min + 1
        sym_rate = fs / (fft + cp)

        print(f"\n--- {name} ---")
        print(f"  SC {sc_min}-{sc_max} ({n_sc} SCs), "
              f"{f_low:.0f}-{f_high:.0f} Hz, "
              f"sym={sym_rate:.0f} Hz, FFT={fft}, CP={cp}")

        try:
            t0 = time.time()
            result = run_ota_test(config, n_frames=args.frames,
                                  device_rx=args.device_rx,
                                  device_tx=args.device_tx)
            elapsed = time.time() - t0

            pct = result["valid_pct"]
            results.append((name, sc_min, sc_max, fft, cp, f_low, f_high, result))

            print(f"  Took {elapsed:.0f}s")
            print(f"  >> Valid: {pct:.1f}% "
                  f"({result['n_frames_valid']}/{result['n_frames_sent']})")
            print(f"  >> Detected: {result['n_frames_detected']}, "
                  f"1st peak: {result['first_peak']:.4f}")

        except Exception as e:
            logger.error(f"Error testing {name}: {e}")
            import traceback
            traceback.print_exc()

    print("\n" + "=" * 90)
    print("FREQUENCY RESPONSE SUMMARY")
    print("=" * 90)
    print(f"{'Config':<14} {'SC':<8} {'Freq (Hz)':<22} "
          f"{'SCs':<5} {'Sym/s':<8} {'Sent':<6} {'Valid':<6} {'%':<7} {'Peak'}")
    print("-" * 90)
    for name, sc_min, sc_max, fft, cp, f_low, f_high, result in results:
        sym_rate = 48000 / (fft + cp)
        n_sc = sc_max - sc_min + 1
        print(f"{name:<14} {sc_min}-{sc_max:<5} "
              f"{f_low:.0f}-{f_high:.0f} Hz    "
              f"{n_sc:<5} {sym_rate:<8.0f} "
              f"{result['n_frames_sent']:<6} "
              f"{result['n_frames_valid']:<6} "
              f"{result['valid_pct']:<7.1f} "
              f"{result['first_peak']:.4f}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
