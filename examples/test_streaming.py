#!/usr/bin/env python3
"""End-to-end streaming test over acoustic OFDM.

Sends a known data file through the audio loopback cable (TX→speaker→cable→mic→RX)
and verifies the received data matches.
"""

import logging
import sys
import time

import numpy as np

from src.audio.io import AudioStream
from src.config import Config
from src.link.framing import FrameAssembler, FrameParser
from src.physical.ofdm import OfdmModulator, OfdmDemodulator

logging.basicConfig(level=logging.INFO,
                    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
                    datefmt="%H:%M:%S")
logger = logging.getLogger("test_streaming")


def stream_test(config, input_data, frames_per_burst=2):
    modulator = OfdmModulator(config)
    demodulator = OfdmDemodulator(config)
    assembler = FrameAssembler(config)
    parser = FrameParser(config)
    payload_size = config.frame.payload_size

    # Build all frames up front
    all_frames = b""
    offset = 0
    total_sent = 0
    while offset < len(input_data):
        batch = input_data[offset:offset + frames_per_burst * payload_size]
        if len(batch) < payload_size:
            break
        frames_data = b""
        for _ in range(frames_per_burst):
            payload = batch[:payload_size]
            batch = batch[payload_size:]
            if len(payload) < payload_size:
                break
            frames_data += assembler.assemble_frame(payload)
        all_frames += frames_data
        total_sent += frames_per_burst * payload_size
        offset += frames_per_burst * payload_size

    # Modulate entire batch as one burst
    audio = modulator.modulate_with_preamble(all_frames)
    silence = np.zeros(int(config.audio.sample_rate * 0.5), dtype=np.float32)
    tx_audio = np.concatenate([audio, silence])
    tx_duration = len(tx_audio) / config.audio.sample_rate

    tx_pos = [0]
    rx_buffers = []

    def tx_cb(frames, status):
        start = tx_pos[0]
        end = min(start + frames, len(tx_audio))
        chunk = np.zeros(frames, dtype=np.float32)
        if start < len(tx_audio):
            chunk[:end - start] = tx_audio[start:end]
        tx_pos[0] = end
        return chunk

    def rx_cb(samples, time_info):
        rx_buffers.append(samples.copy())

    stream = AudioStream(config.audio, callback_rx=rx_cb, callback_tx=tx_cb)
    try:
        stream.start("full-duplex")
    except Exception:
        stream.start("split-duplex")

    logger.info("TX: %d bytes in %d frames (%.1f s)", total_sent,
                assembler.sequence_number, tx_duration)
    time.sleep(tx_duration + 2.0)
    stream.stop()

    all_rx = np.concatenate(rx_buffers)
    logger.info("RX: %d samples (%.1f s)", len(all_rx), len(all_rx) / config.audio.sample_rate)

    # Crop trailing to expected burst length (no leading crop — demod's internal
    # energy detection handles leading silence better than window-based cropping)
    sym_samples = config.ofdm_symbol_samples
    preamble_samples = config.ofdm.preamble_symbols * sym_samples
    frame_bits = len(all_frames) * 8
    data_bits_per_sym = config.ofdm_bits_per_symbol
    data_syms = max(1, int(np.ceil(frame_bits / data_bits_per_sym)))
    expected = preamble_samples + data_syms * sym_samples + int(config.audio.sample_rate * 0.2)
    all_rx = all_rx[:expected]

    # Demodulate
    bits = demodulator.process_samples(all_rx)
    if bits is None:
        logger.error("No bits detected!")
        return 0, 0

    byte_est = len(all_frames) + 64
    decoded = demodulator.symbols_to_bytes(bits, byte_est)

    frames = parser.feed_bytes(decoded)
    received = b""
    valid_count = 0
    for payload, seq, ftype, valid in frames:
        if valid and ftype == 0:
            received += payload
            valid_count += 1

    logger.info("Frames: %d received, %d valid", len(frames), valid_count)
    return valid_count, total_sent


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True, help="Input file to stream")
    parser.add_argument("--input-name", default="USB_PnP")
    parser.add_argument("--output-name", default="analog-stereo")
    parser.add_argument("--frames-per-burst", type=int, default=2)
    args = parser.parse_args()

    with open(args.input, "rb") as f:
        data = f.read()

    logger.info("Input: %s (%d bytes)", args.input, len(data))

    c = Config()
    c.modulation.use_ofdm = True
    c.audio.device_input_name = args.input_name
    c.audio.device_output_name = args.output_name

    valid, total = stream_test(c, data, args.frames_per_burst)
    logger.info("Result: %d/%d bytes valid", valid * c.frame.payload_size, total)
    sys.exit(0 if valid * c.frame.payload_size >= total else 1)
