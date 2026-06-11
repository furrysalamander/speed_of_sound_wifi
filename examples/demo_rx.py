"""Continuous per-frame OFDM receiver for demo.

Captures microphone audio, continuously detects per-frame preambles,
demodulates each frame, parses frames, and writes payload bytes to stdout.
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
    search_pos = 0  # byte position in buf to search from

    def rx_cb(samples, _time_info):
        rx_q.append(samples.copy())

    stream = AudioStream(config.audio, callback_rx=rx_cb)
    stream.start("rx")
    logger.info("RX started, listening...")

    t0 = time.time()

    try:
        while time.time() - t0 < LISTEN_TIMEOUT:
            time.sleep(0.02)
            while rx_q:
                buf = np.concatenate([buf, rx_q.pop(0)])

            if len(buf) < search_pos + frame_samples:
                continue

            # Cross-correlation from search_pos
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
                # No preamble found — keep accumulating, advance search_pos to
                # avoid re-scanning old data (but leave a frame window for overlap)
                search_pos = max(0, len(buf) - frame_samples)
                continue

            abs_pos = search_pos + peak

            # Extract exactly one frame: preamble + margin + data
            margin = int(sym_len * 0.5)
            chunk_start = max(0, abs_pos - margin)
            chunk_end = abs_pos + frame_samples + 4
            if chunk_end > len(buf):
                continue  # wait for more data
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
                # Use precise preamble position from process_samples to avoid drift
                plen_start = getattr(demod, '_last_preamble_start', None)
                if plen_start is not None:
                    data_syms = len(bits) // demod.data_bits_per_sym
                    search_pos = chunk_start + plen_start + plen + data_syms * sym_len
                    search_pos = int(search_pos)
                else:
                    search_pos = abs_pos + frame_samples
            else:
                # Advance just past this position
                logger.warning("Frame skipped: npk=%.3f at t=%.1fs", npk,
                               time.time() - t0)
                search_pos = search_pos + 1

            # Prune old data to keep buffer bounded
            if search_pos > 2 * frame_samples:
                buf = buf[search_pos:]
                search_pos = 0
                logger.info("Buffer pruned: %d samples remaining", len(buf))

    except KeyboardInterrupt:
        pass
    finally:
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
