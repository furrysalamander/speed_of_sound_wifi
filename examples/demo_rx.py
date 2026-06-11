"""Continuous per-frame OFDM receiver for demo.

Captures microphone audio, continuously detects per-frame preambles,
demodulates each frame, parses frames, and writes payload bytes to stdout.
"""

import logging
import signal
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

LISTEN_TIMEOUT = 600
FRAME_DATA_SYMS = 60


def demo_rx(config, output_file=None):
    demod = OfdmDemodulator(config)
    parser = FrameParser(config)
    sym_len = demod.sym_samples
    plen = demod.preamble_symbols * sym_len
    frame_samples = plen + FRAME_DATA_SYMS * sym_len
    preamble_thresh = 0.10

    buf = np.array([], dtype=np.float32)
    rx_q = []
    output_stream = open(output_file, "wb") if output_file else sys.stdout.buffer
    total_received = 0
    n_frames = 0
    search_pos = 0
    running = True

    def shutdown(_signum=None, _frame=None):
        nonlocal running
        running = False

    signal.signal(signal.SIGINT, shutdown)
    signal.signal(signal.SIGTERM, shutdown)

    def rx_cb(samples, _time_info):
        rx_q.append(samples.copy())

    stream = AudioStream(config.audio, callback_rx=rx_cb)
    stream.start("rx")
    logger.info("RX started, listening...")

    t0 = time.time()

    try:
        while running and time.time() - t0 < LISTEN_TIMEOUT:
            time.sleep(0.02)
            while rx_q:
                buf = np.concatenate([buf, rx_q.pop(0)])

            if len(buf) < search_pos + frame_samples:
                continue

            search_buf = buf[search_pos:]
            xcorr = np.convolve(search_buf, demod._preamble_audio[::-1],
                                mode="valid")
            xcorr_abs = np.abs(xcorr)
            peak = int(np.argmax(xcorr_abs))
            pe = float(np.dot(demod._preamble_audio, demod._preamble_audio))
            if pe < 1e-10:
                continue
            seg = search_buf[peak:peak + plen]
            se = float(np.dot(seg, seg))
            npk = float(xcorr_abs[peak]) / (np.sqrt(pe * se) + 1e-10)
            if npk < preamble_thresh:
                search_pos += int(sym_len * 0.5)
                continue

            abs_pos = search_pos + peak

            margin = int(sym_len * 0.5)
            chunk_start = max(0, abs_pos - margin)
            chunk_end = abs_pos + frame_samples + 4
            if chunk_end > len(buf):
                continue
            chunk = buf[chunk_start:chunk_end]

            bits = demod.process_samples(chunk)

            success = False
            if bits is not None and len(bits) >= 8:
                byte_est = len(bits) // 8 + 64
                decoded = demod.symbols_to_bytes(bits, byte_est)
                frames = parser.feed_bytes(decoded)

                for payload, seq, ftype, valid in frames:
                    if valid and ftype == 0:
                        output_stream.write(payload)
                        output_stream.flush()
                        total_received += len(payload)
                        n_frames += 1
                        success = True
                        epoch = time.time() - t0
                        logger.info("Frame #%d: %d B (total %d, t=%.1fs)",
                                    seq, len(payload), total_received, epoch)

            if success:
                search_pos = abs_pos + frame_samples
            else:
                logger.warning("Frame fail at t=%.1fs (npk=%.3f, n=%d)",
                               time.time() - t0, npk, n_frames)
                search_pos = abs_pos + frame_samples

    finally:
        output_stream.flush()
        stream.stop()
        if output_file:
            output_stream.close()
        logger.info("RX done: %d bytes, %d frames in %.1fs",
                    total_received, n_frames, time.time() - t0)


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
