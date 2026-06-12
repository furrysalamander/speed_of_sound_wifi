#!/usr/bin/env python3
# Launch the full OTA demo pipeline:
#    TX (speaker) -> [OTA] -> RX (USB mic) -> ffplay
#
# ffplay needs a complete WebM header to start, so the pipeline buffers
# the first ~16 kB of decoded bytes before spawning ffplay (with display).
#
# Usage:
#   python -m examples.demo_pipeline                  # auto 30s test clip + ffplay
#   python -m examples.demo_pipeline --input clip.bin  # custom payload
#   python -m examples.demo_pipeline --no-ffplay       # write to file instead

import argparse
import os
import subprocess
import sys
import threading
import time

from src.config import Config

DEFAULT_INPUT = "/tmp/shrek_test_30s.bin"
PROBE_THRESHOLD = 16384  # bytes needed before ffplay can probe the format


def main():
    parser = argparse.ArgumentParser(
        description="OTA demo: TX -> [air] -> RX -> ffplay")
    parser.add_argument("--input", default=DEFAULT_INPUT,
                        help="Input payload file (auto-generates 30s if missing)")
    parser.add_argument("--tx-device", default="analog-stereo",
                        help="Output (TX) device name")
    parser.add_argument("--rx-device", default="USB_PnP",
                        help="Input (RX) device name")
    parser.add_argument("--no-ffplay", action="store_true",
                        help="Write decoded bytes to file instead")
    parser.add_argument("--output", default="/tmp/demo_rx_out.bin",
                        help="Output file path (with --no-ffplay)")
    args = parser.parse_args()

    # --- prepare payload ---
    input_path = args.input
    if not os.path.exists(input_path):
        print(f"Generating test clip: {input_path}", flush=True)
        c = Config()
        ps = c.frame.payload_size
        n = 68
        with open(input_path, "wb") as f:
            f.write(bytes(i % 256 for i in range(n * ps)))
        print(f"  {n} frames, {n * ps} B", flush=True)

    # --- start receiver ---
    rx_proc = subprocess.Popen(
        [sys.executable, "-m", "examples.demo_rx",
         "--output-name", args.rx_device],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    time.sleep(1.0)

    # --- start transmitter ---
    print(f"TX: {input_path} ({args.tx_device})", flush=True)
    tx_proc = subprocess.Popen(
        [sys.executable, "-m", "examples.demo_tx",
         "--input", input_path,
         "--input-name", args.tx_device],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )

    # --- handle RX output ---
    if args.no_ffplay:
        done = threading.Event()
        def to_file():
            with open(args.output, "wb") as f:
                while True:
                    chunk = rx_proc.stdout.read(65536)
                    if not chunk:
                        break
                    f.write(chunk)
            done.set()
        threading.Thread(target=to_file, daemon=True).start()
        tx_proc.wait()
        time.sleep(3.0)
        rx_proc.kill()
        rx_proc.wait()
        done.wait(timeout=5)
        print(f"Written to {args.output}", flush=True)
    else:
        buf = []
        ffplay_proc = None
        lock = threading.Lock()
        probe_ev = threading.Event()

        def reader():
            nonlocal ffplay_proc
            total = 0
            probed = False
            while True:
                chunk = rx_proc.stdout.read(4096)  # small reads for early ffplay
                if not chunk:
                    break
                with lock:
                    if not probed:
                        buf.append(chunk)
                        total += len(chunk)
                        if total >= PROBE_THRESHOLD:
                            probed = True
                            print(f"Spawning ffplay ({total:,} B)...", flush=True)
                            ffplay_proc = subprocess.Popen(
                                ["ffplay", "-i", "pipe:0", "-an",
                                 "-loglevel", "quiet"],
                                stdin=subprocess.PIPE,
                                stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL,
                            )
                            for b in buf:
                                ffplay_proc.stdin.write(b)
                            ffplay_proc.stdin.flush()
                            buf.clear()
                            probe_ev.set()
                    else:
                        ffplay_proc.stdin.write(chunk)
                        ffplay_proc.stdin.flush()
            if ffplay_proc is not None:
                try:
                    ffplay_proc.stdin.close()
                except (BrokenPipeError, AttributeError):
                    pass

        reader_thread = threading.Thread(target=reader, daemon=True)
        reader_thread.start()

        if probe_ev.wait(timeout=30):
            print("ffplay started — streaming live", flush=True)
        else:
            print("Still buffering after 30s...", flush=True)

        tx_proc.wait()
        time.sleep(3.0)
        rx_proc.kill()
        rx_proc.wait()
        reader_thread.join(timeout=5)

    # --- report ---
    err = rx_proc.stderr.read().decode() if rx_proc.stderr else ""
    for line in err.split("\n"):
        ll = line.strip()
        if ll and ("WARNING" in ll or "ERROR" in ll or "RX done" in ll):
            print(f"  {ll}", flush=True)
    print("Demo done", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
