"""Continuous per-frame OFDM receiver for demo.

Captures microphone audio, continuously detects per-frame preambles,
demodulates each frame, parses frames, and writes payload bytes to stdout.
"""

import json
import logging
import os
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


def frame_data_syms(config):
    """Compute the number of data OFDM symbols per frame from config."""
    nsym = config.fec.nsym
    data_len = 255 - nsym  # info bytes per RS block
    data_for_fec = 4 + config.frame.payload_size
    n_blocks = (data_for_fec + data_len - 1) // data_len
    frame_bytes = 8 + n_blocks * 255 + 4  # sync + RS blocks + CRC
    sc = config.ofdm.subcarrier_max - config.ofdm.subcarrier_min + 1
    bits_per_sym = config.ofdm.bits_per_subcarrier * sc
    return (frame_bytes * 8 + bits_per_sym - 1) // bits_per_sym


def demo_rx(config, output_file=None, save_failures_dir=None):
    demod = OfdmDemodulator(config)
    parser = FrameParser(config)
    sym_len = demod.sym_samples
    plen = demod.preamble_symbols * sym_len
    data_syms = frame_data_syms(config)
    frame_samples = plen + data_syms * sym_len
    preamble_thresh = 0.10

    buf = np.array([], dtype=np.float32)
    rx_q = []
    output_stream = open(output_file, "wb") if output_file else sys.stdout.buffer
    total_received = 0
    n_frames = 0
    search_pos = 0
    running = True

    fail_meta = []

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

            margin = int(sym_len * 0.5)

            if n_frames == 0:
                # Acquisition: search within 1 frame of search_pos to ensure
                # we find the FIRST frame (not a later stronger one).
                win = int(frame_samples + sym_len)
                search_buf = buf[search_pos:search_pos + win]
                if len(search_buf) < plen:
                    search_pos += int(sym_len * 0.5)
                    continue
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
                if npk < preamble_thresh or se < 0.5:
                    search_pos += int(sym_len * 0.5)
                    continue
                abs_pos = search_pos + peak
            else:
                # Tracking: position-based — process_samples handles internal timing
                abs_pos = search_pos
                npk = 0.0
                peak = 0

            chunk_start = max(0, abs_pos - margin)
            chunk_end = abs_pos + frame_samples + 4
            if chunk_end > len(buf):
                # buffer not filled yet — wait for more samples
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
                logger.warning("Frame fail at t=%.1fs (npk=%.4f, n=%d)",
                               time.time() - t0, npk, n_frames)
                search_pos = abs_pos + frame_samples

            if save_failures_dir and not success:
                meta = {
                    "npk": round(npk, 6),
                    "peak_idx": int(peak),
                    "search_pos": int(search_pos - frame_samples),
                    "abs_pos": int(abs_pos),
                    "t_abs": round(time.time() - t0, 3),
                }
                fname = f"fail_{n_frames:04d}.npy"
                np.save(os.path.join(save_failures_dir, fname), chunk)
                meta["audio_file"] = fname
                fail_meta.append(meta)

    finally:
        output_stream.flush()
        stream.stop()
        if output_file:
            output_stream.close()
        if save_failures_dir and fail_meta:
            jpath = os.path.join(save_failures_dir, "metadata.json")
            with open(jpath, "w") as f:
                json.dump(fail_meta, f, indent=2)
            np.save(os.path.join(save_failures_dir, "full_recording.npy"), buf)
            logger.info("Saved %d failure records to %s",
                        len(fail_meta), save_failures_dir)
        logger.info("RX done: %d bytes, %d frames in %.1fs",
                    total_received, n_frames, time.time() - t0)


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", help="Output file (default: stdout)")
    parser.add_argument("--output-name", default="USB_PnP",
                        help="Input device name")
    parser.add_argument("--save-failures", default=None,
                        help="Directory to save failure audio and full recording")
    args = parser.parse_args()

    c = Config()
    c.modulation.use_ofdm = True
    c.audio.device_input_name = args.output_name

    demo_rx(c, args.output, args.save_failures)
