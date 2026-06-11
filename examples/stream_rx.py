"""Stream data over acoustic OFDM — receiver.

Captures microphone audio, detects OFDM bursts, demodulates, parses frames,
and writes payloads to stdout (for piping into ffplay or similar).
"""

import logging
import sys
import time

import numpy as np

from src.audio.io import AudioStream
from src.config import Config
from src.link.framing import FrameParser
from src.physical.ofdm import OfdmDemodulator

logging.basicConfig(level=logging.DEBUG,
                    format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
                    datefmt="%H:%M:%S")
logger = logging.getLogger("stream_rx")

BUF_SECONDS = 6


def stream_rx(config, output_file=None):
    demod = OfdmDemodulator(config)
    parser = FrameParser(config)
    sym_len = demod.sym_samples
    plen = len(demod._preamble_audio)
    data_bps = demod.data_bits_per_sym
    preamble_thresh = 0.10  # match demod's threshold

    # Compute actual frame size (sync + RS-encoded data + CRC)
    rs_data_len = config.frame.payload_size + 4  # header(4) + payload
    data_per_block = 255 - config.fec.nsym
    blocks_per_frame = max(1, int(np.ceil(rs_data_len / data_per_block)))
    frame_size = 8 + blocks_per_frame * 255 + 4
    syms_per_frame = max(1, int(np.ceil(frame_size * 8 / data_bps)))
    chunk_syms = syms_per_frame * 2  # 2 frames per burst (no margin to avoid eating next preamble)

    buf = np.array([], dtype=np.float32)
    rx_q = []
    output_stream = open(output_file, "wb") if output_file else sys.stdout.buffer
    total_received = 0

    def rx_cb(samples, _time_info):
        rx_q.append(samples.copy())

    stream = AudioStream(config.audio, callback_rx=rx_cb)
    stream.start("rx")

    try:
        while True:
            time.sleep(0.1)
            while rx_q:
                buf = np.concatenate([buf, rx_q.pop(0)])

            max_samp = int(BUF_SECONDS * config.audio.sample_rate)
            if len(buf) > max_samp:
                buf = buf[-max_samp:]

            if len(buf) < plen + sym_len:
                continue

            # Coarse energy detection
            preamble = demod._preamble_audio
            energy = np.convolve(buf ** 2, np.ones(plen) / plen, mode="same")
            thresh = np.max(energy) * 0.1
            above = energy > thresh
            if not np.any(above):
                logger.debug("buf=%d no energy", len(buf))
                continue

            coarse = max(0, np.argmax(above) - plen // 2)
            search_end = min(len(buf), coarse + 2 * plen + sym_len)
            if search_end - coarse < plen:
                logger.debug("buf=%d short search region", len(buf))
                continue

            region = buf[coarse:search_end]
            xcorr = np.convolve(region, preamble[::-1], mode="valid")
            peak = int(np.argmax(np.abs(xcorr)))
            pe = float(np.dot(preamble, preamble))
            if pe < 1e-10:
                continue
            seg = region[peak:peak + plen]
            se = float(np.dot(seg, seg))
            npk = float(np.abs(xcorr[peak])) / (np.sqrt(pe * se) + 1e-10)
            if npk < preamble_thresh:
                logger.debug("buf=%d weak npk=%.3f", len(buf), npk)
                continue

            found = coarse + peak

            # Pass a limited chunk to the demod to prevent processing
            # the entire buffer (which includes noise + next bursts).
            needed = found + plen + chunk_syms * sym_len
            if len(buf) < needed:
                logger.debug("buf=%d need=%d not enough samples", len(buf), needed)
                continue

            chunk = buf[found:needed]
            bits = demod.process_samples(chunk)
            if bits is None or len(bits) < 8:
                logger.debug("buf=%d demod returned None", len(buf))
                buf = buf[found + 1:]
                continue

            byte_est = len(bits) // 8 + 64
            decoded = demod.symbols_to_bytes(bits, byte_est)

            frames = parser.feed_bytes(decoded)

            for payload, seq, ftype, valid in frames:
                if valid and ftype == 0:
                    output_stream.write(payload)
                    output_stream.flush()
                    total_received += len(payload)
                    logger.info("Frame #%d: %d B (total %d)", seq, len(payload), total_received)

            logger.info("Consuming %d samples (found=%d, %d frames)", found + plen + chunk_syms * sym_len - 4, found, len(frames))

            # Consume chunk_syms symbols worth of samples with a 4-sample
            # safety margin to avoid consuming into the next burst's preamble
            # (cross-correlation peak can be 1-2 samples off due to the
            # raised-cosine onset on the preamble).
            margin = 4
            consumed_samples = found + plen + chunk_syms * sym_len - margin
            buf = buf[min(int(consumed_samples), len(buf)):]

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
    parser.add_argument("--output-index", type=int, default=None,
                        help="Input device index")
    args = parser.parse_args()

    c = Config()
    c.modulation.use_ofdm = True
    c.audio.device_input_name = args.output_name
    if args.output_index is not None:
        c.audio.device_input_index = args.output_index

    stream_rx(c, args.output)
