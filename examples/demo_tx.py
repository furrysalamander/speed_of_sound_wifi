"""Single-burst OFDM transmitter for demo.

Reads a file, assembles all frames, modulates as one OFDM burst
with a single preamble, plays through speaker, and exits.
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
        logger.info("Padded %d → %d bytes (%d frames, %d extra zero bytes)",
                    len(data) - extra, len(data),
                    len(data) // payload_size, extra)

    # Assemble all frames
    frames_data = b""
    for i in range(0, len(data), payload_size):
        payload = data[i:i + payload_size]
        frames_data += assembler.assemble_frame(payload)

    # Modulate as single burst
    audio = modulator.modulate_with_preamble(frames_data)
    total_dur = len(audio) / config.audio.sample_rate

    logger.info("TX: %d frames, %d B payload, %.1f s audio",
                assembler.sequence_number, len(data), total_dur)

    # Append silence so the RX can finish processing
    audio = np.concatenate([audio,
                            np.zeros(int(config.audio.sample_rate * 0.5),
                                     dtype=np.float32)])

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
    total_dur = len(audio) / config.audio.sample_rate
    time.sleep(total_dur + 1.0)
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
