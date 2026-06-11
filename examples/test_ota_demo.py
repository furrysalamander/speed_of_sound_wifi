"""Reproducible OTA test (68 frames ~16 s, matches the standard 30s clip size).

Usage:
    python -m examples.test_ota_demo

Generates 68 frames of test payload (~16s of audio at current config),
transmits over speaker, captures on USB mic, analyzes results, and
exits with code 0 if >= 90% frames received.
"""

import os
import subprocess
import sys
import tempfile
import time

from src.config import Config


def frame_data_syms(config):
    """Compute the number of data OFDM symbols per frame from config."""
    nsym = config.fec.nsym
    data_len = 255 - nsym
    data_for_fec = 4 + config.frame.payload_size
    n_blocks = (data_for_fec + data_len - 1) // data_len
    frame_bytes = 8 + n_blocks * 255 + 4
    sc = config.ofdm.subcarrier_max - config.ofdm.subcarrier_min + 1
    bits_per_sym = config.ofdm.bits_per_subcarrier * sc
    return (frame_bytes * 8 + bits_per_sym - 1) // bits_per_sym


def main():
    config = Config()
    config.modulation.use_ofdm = True

    ps = config.frame.payload_size

    # Fixed frame count (68 frames = standard 30s clip, ~16s audio)
    n_frames = 68
    payload = bytes(i % 256 for i in range(n_frames * ps))

    with tempfile.NamedTemporaryFile(prefix="ota_test_", suffix=".bin",
                                     delete=False) as f:
        input_path = f.name
        f.write(payload)

    output_path = input_path + ".rx"

    try:
        rx_proc = subprocess.Popen(
            [sys.executable, "-m", "examples.demo_rx",
             "--output-name", "USB_PnP",
             "--output", output_path],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )

        time.sleep(1.0)

        tx_proc = subprocess.run(
            [sys.executable, "-m", "examples.demo_tx",
             "--input", input_path,
             "--input-name", "analog-stereo"],
            capture_output=True, text=True, timeout=60,
        )

        rx_proc.wait(timeout=30)

        if not os.path.exists(output_path):
            nf = 0
        else:
            with open(output_path, "rb") as f:
                r = f.read()
            nf_received = len(r) // ps
            recv_set = set(r[i * ps:(i + 1) * ps] for i in range(nf_received))
            sent_set = set(payload[i * ps:(i + 1) * ps]
                           for i in range(n_frames))
            matching = sum(1 for p in sent_set if p in recv_set)
            nf = matching

        pct = 100 * nf // n_frames
        print(f"OTA test: {nf}/{n_frames} frames = {pct}%")

        ok = pct >= 90
        print(f"Result: {'PASS' if ok else 'FAIL'} (threshold >= 90%)")
        return 0 if ok else 1

    finally:
        for path in [input_path, output_path]:
            try:
                os.unlink(path)
            except FileNotFoundError:
                pass


if __name__ == "__main__":
    sys.exit(main())
