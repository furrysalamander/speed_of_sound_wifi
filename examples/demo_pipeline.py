#!/usr/bin/env python3
"""Launch the full OTA demo pipeline:
   TX (speaker) -> [OTA] -> RX (USB mic) -> ffplay"""

import argparse
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", default="/tmp/shrek_test_30s.bin",
                        help="Input payload file (default: 10-frame Shrek clip)")
    parser.add_argument("--tx-device", default="analog-stereo",
                        help="Output (TX) device name")
    parser.add_argument("--rx-device", default="USB_PnP",
                        help="Input (RX) device name")
    parser.add_argument("--no-ffplay", action="store_true",
                        help="Write to file instead of piping to ffplay")
    parser.add_argument("--output", default="/tmp/demo_rx_out.bin",
                        help="Output file (with --no-ffplay)")
    args = parser.parse_args()

    if args.no_ffplay:
        rx_proc = subprocess.Popen(
            [sys.executable, "-m", "examples.demo_rx",
             "--output", args.output,
             "--output-name", args.rx_device],
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
        )
    else:
        ffplay_proc = subprocess.Popen(
            ["ffplay", "-i", "pipe:0", "-an", "-nodisp", "-loglevel", "quiet"],
            stdin=subprocess.PIPE, stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        time.sleep(0.5)
        rx_proc = subprocess.Popen(
            [sys.executable, "-m", "examples.demo_rx",
             "--output-name", args.rx_device],
            stdout=ffplay_proc.stdin, stderr=subprocess.PIPE,
        )

    time.sleep(1.0)
    print(f"TX: {args.input} ({args.tx_device})", flush=True)
    tx_proc = subprocess.run(
        [sys.executable, "-m", "examples.demo_tx",
         "--input", args.input,
         "--input-name", args.tx_device],
        capture_output=True, text=True, timeout=30,
    )

    try:
        rx_proc.wait(timeout=20)
    except subprocess.TimeoutExpired:
        rx_proc.kill()
        rx_proc.wait()

    err = rx_proc.stderr.read().decode() if rx_proc.stderr else ""
    for line in err.split("\n"):
        ll = line.strip()
        if ll and ("WARNING" in ll or "ERROR" in ll or "Done:" in ll or "RX done" in ll):
            print(f"  {ll}", flush=True)

    if not args.no_ffplay:
        ffplay_proc.terminate()
        ffplay_proc.wait()

    print("Demo done", flush=True)


if __name__ == "__main__":
    main()
