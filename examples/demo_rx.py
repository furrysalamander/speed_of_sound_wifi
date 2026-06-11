"""Single-burst OFDM receiver for demo.

Captures microphone audio until a burst is detected, demodulates all
symbols, parses all frames, and writes payload bytes to stdout
(for piping into ffplay or similar).
"""

import logging
import sys
import time

import numpy as np

from src.audio.io import AudioStream
from src.config import Config
from src.link.framing import FrameParser
from src.physical.ofdm import OfdmDemodulator

logging.basicConfig(level=logging.WARNING,
                    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
                    datefmt="%H:%M:%S")
logger = logging.getLogger("demo_rx")

MAX_DATA_SYMS = 700


def demo_rx(config, output_file=None):
    demod = OfdmDemodulator(config)
    parser = FrameParser(config)
    sym_len = demod.sym_samples
    plen = len(demod._preamble_audio)
    preamble_thresh = 0.10

    buf = np.array([], dtype=np.float32)
    rx_q = []
    output_stream = open(output_file, "wb") if output_file else sys.stdout.buffer
    total_received = 0

    def rx_cb(samples, _time_info):
        rx_q.append(samples.copy())

    stream = AudioStream(config.audio, callback_rx=rx_cb)
    stream.start("rx")
    logger.info("RX started, listening...")

    burst_seek_start = time.time()
    listen_timeout = MAX_DATA_SYMS * sym_len / config.audio.sample_rate + 6.0  # burst dur + margin

    try:
        while time.time() - burst_seek_start < listen_timeout:
            time.sleep(0.05)
            while rx_q:
                buf = np.concatenate([buf, rx_q.pop(0)])

            if len(buf) < plen + sym_len * 4:
                continue

            # Cross-correlation preamble detection
            preamble = demod._preamble_audio
            xcorr = np.convolve(buf, preamble[::-1], mode="valid")
            xcorr_abs = np.abs(xcorr)
            peak = int(np.argmax(xcorr_abs))
            pe = float(np.dot(preamble, preamble))
            if pe < 1e-10:
                continue
            seg = buf[peak:peak + plen]
            se = float(np.dot(seg, seg))
            npk = float(xcorr_abs[peak]) / (np.sqrt(pe * se) + 1e-10)
            if npk < preamble_thresh:
                continue

            logger.info("Preamble detected: npk=%.3f at sample %d", npk, peak)

            # Wait for enough samples
            needed = peak + plen + MAX_DATA_SYMS * sym_len
            while len(buf) < needed and time.time() - burst_seek_start < listen_timeout:
                time.sleep(0.05)
                while rx_q:
                    buf = np.concatenate([buf, rx_q.pop(0)])

            # Process
            actual_data_syms = min((len(buf) - peak - plen) // sym_len, MAX_DATA_SYMS)
            if actual_data_syms < 1:
                logger.warning("Not enough data symbols")
                break

            chunk_end = peak + plen + actual_data_syms * sym_len
            chunk = buf[peak:chunk_end]
            bits = demod.process_samples(chunk)

            if bits is None or len(bits) < 8:
                logger.warning("Demod failed")
                break

            byte_est = len(bits) // 8 + 64
            decoded = demod.symbols_to_bytes(bits, byte_est)
            frames = parser.feed_bytes(decoded)

            valid_count = 0
            for payload, seq, ftype, valid in frames:
                if valid and ftype == 0:
                    output_stream.write(payload)
                    output_stream.flush()
                    total_received += len(payload)
                    valid_count += 1

            logger.info("Done: %d data syms, %d frames (%d valid), %d B",
                        actual_data_syms, len(frames), valid_count, total_received)
            break

    except KeyboardInterrupt:
        pass
    finally:
        stream.stop()
        if output_file:
            output_stream.close()
        logger.info("RX done: %d bytes", total_received)


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", help="Output file (default: stdout)")
    parser.add_argument("--output-name", default="USB_PnP",
                        help="Input device name")
    args = parser.parse_args()

    c = Config()
    c.modulation.use_ofdm = True
    c.audio.device_input_name = args.output_name

    demo_rx(c, args.output)
