"""Per-frame preamble OFDM transmitter for streaming demo.

Reads a file, generates frame audio on-the-fly as the audio callback
requests samples (avoids O(1 GB) pre-generation for long clips).
"""

import logging
import sys
import time

import numpy as np

from src.audio.io import AudioStream
from src.config import Config
from src.link.framing import FrameAssembler
from src.physical.ofdm import OfdmModulator

logging.basicConfig(level=logging.WARNING,
                    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
                    datefmt="%H:%M:%S")
logger = logging.getLogger("demo_tx")


def demo_tx(config, input_file=None):
    modulator = OfdmModulator(config)
    assembler = FrameAssembler(config)
    payload_size = config.frame.payload_size
    # Compute total audio length per frame
    data_for_fec = 4 + config.frame.payload_size
    data_len = 255 - config.fec.nsym
    n_blocks = (data_for_fec + data_len - 1) // data_len
    frame_bytes = 8 + n_blocks * 255 + 4
    sc = config.ofdm.subcarrier_max - config.ofdm.subcarrier_min + 1
    bits_per_sym = config.ofdm.bits_per_subcarrier * sc
    data_syms = (frame_bytes * 8 + bits_per_sym - 1) // bits_per_sym
    frame_samples = (config.ofdm.preamble_symbols + data_syms) * \
                    config.ofdm_symbol_samples
    silence_samples = int(config.audio.sample_rate * 0.5)

    input_stream = open(input_file, "rb") if input_file else sys.stdin.buffer
    data = input_stream.read()
    if input_file:
        input_stream.close()

    if not data:
        logger.error("No data to transmit")
        return

    # Pad to frame boundary
    extra = (payload_size - len(data) % payload_size) % payload_size
    if extra:
        data = data + b"\x00" * extra
        logger.info("Padded %d -> %d bytes (%d frames, %d extra zero bytes)",
                    len(data) - extra, len(data),
                    len(data) // payload_size, extra)

    n_frames = len(data) // payload_size
    total_audio_len = n_frames * frame_samples + silence_samples

    # Buffer of pre-generated audio; extended on-the-fly
    audio_buf = []
    next_frame = 0  # which frame index to generate next
    tx_pos = [0]

    def _ensure_buf(pos):
        """Generate more frame audio until the buffer covers position pos."""
        nonlocal next_frame
        while len(audio_buf) * frame_samples < pos + frame_samples:
            if next_frame < n_frames:
                payload = data[next_frame * payload_size:
                               (next_frame + 1) * payload_size]
                frame = assembler.assemble_frame(payload)
                audio_buf.append(modulator.modulate_with_preamble(frame))
                next_frame += 1
            else:
                break
        # Append trailing silence once all frames are generated
        if next_frame == n_frames and len(audio_buf) == n_frames:
            audio_buf.append(np.zeros(silence_samples, dtype=np.float32))

    def tx_cb(frames, status):
        nonlocal next_frame
        start = tx_pos[0]
        end = min(start + frames, total_audio_len)
        _ensure_buf(end)
        chunk = np.zeros(frames, dtype=np.float32)
        if start < total_audio_len:
            # Copy from the buffer, spanning frames if needed
            remaining = end - start
            pos = start
            out_pos = 0
            while remaining > 0:
                frame_i = pos // frame_samples
                if frame_i >= len(audio_buf):
                    break
                offset = pos % frame_samples
                avail = frame_samples - offset
                take = min(avail, remaining)
                chunk[out_pos:out_pos + take] = \
                    audio_buf[frame_i][offset:offset + take]
                out_pos += take
                remaining -= take
                pos += take
        tx_pos[0] = end
        return chunk

    logger.info("TX: %d frames, %d B payload, %.1f s audio (on-the-fly)",
                n_frames, len(data), total_audio_len / config.audio.sample_rate)

    stream = AudioStream(config.audio, callback_tx=tx_cb)
    stream.start("tx")
    deadline = time.time() + total_audio_len / config.audio.sample_rate + 10.0
    while time.time() < deadline and tx_pos[0] < total_audio_len:
        time.sleep(0.1)
    stream.stop()

    logger.info("TX done")


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", help="Input file (default: stdin)")
    parser.add_argument("--input-name", default="analog-stereo",
                        help="Output device name")
    args = parser.parse_args()

    c = Config()
    c.modulation.use_ofdm = True
    c.audio.device_output_name = args.input_name

    demo_tx(c, args.input)
