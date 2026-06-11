"""Stream data over acoustic OFDM — transmitter.

Reads from file/stdin, frames data, modulates with OFDM, plays over speaker.
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
logger = logging.getLogger("stream_tx")


def stream_tx(config, frames_per_burst=2, input_file=None):
    modulator = OfdmModulator(config)
    assembler = FrameAssembler(config)
    payload_size = config.frame.payload_size

    input_stream = open(input_file, "rb") if input_file else sys.stdin.buffer

    while True:
        byte_buf = b""
        needed = frames_per_burst * payload_size
        while len(byte_buf) < needed:
            chunk = input_stream.read(8192)
            if not chunk:
                break
            byte_buf += chunk
        if len(byte_buf) < payload_size:
            break

        frames_data = b""
        for _ in range(frames_per_burst):
            payload = byte_buf[:payload_size]
            byte_buf = byte_buf[payload_size:]
            if len(payload) < payload_size:
                break
            frames_data += assembler.assemble_frame(payload)
        if not frames_data:
            break

        audio = modulator.modulate_with_preamble(frames_data)
        silence = np.zeros(int(config.audio.sample_rate * 0.1), dtype=np.float32)
        audio = np.concatenate([audio, silence])

        tx_pos = [0]

        def tx_cb(frames, status):
            start = tx_pos[0]
            end = min(start + frames, len(audio))
            chunk = np.zeros(frames, dtype=np.float32)
            if start < len(audio):
                chunk[:end - start] = audio[start:end]
            tx_pos[0] = end
            return chunk

        stream = AudioStream(config.audio, callback_tx=tx_cb)
        stream.start("tx")
        dur = len(audio) / config.audio.sample_rate
        time.sleep(dur + 0.15)
        stream.stop()
        time.sleep(0.05)

    if input_file:
        input_stream.close()
    logger.info("TX done: %d frames", assembler.sequence_number)


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", help="Input file (default: stdin)")
    parser.add_argument("--input-name", default="analog-stereo",
                        help="Output device name")
    parser.add_argument("--frames-per-burst", type=int, default=2)
    args = parser.parse_args()

    c = Config()
    c.modulation.use_ofdm = True
    c.audio.device_output_name = args.input_name

    stream_tx(c, args.frames_per_burst, args.input)
