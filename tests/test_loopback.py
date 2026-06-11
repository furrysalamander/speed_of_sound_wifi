#!/usr/bin/env python3
"""Hardware loopback test suite for Speed of Sound WiFi.

Requires a physical audio loopback cable (3.5mm TRS) connecting the
output (speaker/headphone) jack to the input (microphone/line-in) jack.

Usage:
    # Single test with defaults
    python tests/test_loopback.py --input-device 23 --output-device 18

    # Parameter sweep across baud rates
    python tests/test_loopback.py --input-device 23 --output-device 18 --sweep-baud 500 1000 2000 3000 5000

    # Sweep with custom M-FSK and FEC settings
    python tests/test_loopback.py -i 23 -o 18 --baud-rate 1000 --m-fsk 8 --no-fec

    # Save results to CSV
    python tests/test_loopback.py -i 23 -o 18 --output-csv loopback_results.csv

    # Quick smoke test
    python tests/test_loopback.py -i 23 -o 18 --payload-size 64

Use --list-devices first to find the correct input and output device indices:
    python -m src.main --list-devices
"""

import argparse
import csv
import logging
import sys
import time

import numpy as np

from src.config import Config
from src.audio.devices import print_device_list
from src.audio.loopback import LoopbackTester

logger = logging.getLogger("test_loopback")


def parse_args():
    parser = argparse.ArgumentParser(
        description="Hardware loopback test for Speed of Sound WiFi",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("-v", "--verbose", action="store_true", help="Debug logging")
    parser.add_argument("--list-devices", action="store_true",
                        help="List audio devices and exit")

    # Required device specification
    dev_group = parser.add_argument_group("audio devices (required)")
    dev_group.add_argument("-i", "--input-device", type=int, default=None,
                           required=True,
                           help="Input device index")
    dev_group.add_argument("-o", "--output-device", type=int, default=None,
                           required=True,
                           help="Output device index")

    # Modulation parameters
    mod_group = parser.add_argument_group("modulation parameters")
    mod_group.add_argument("--baud-rate", type=int, default=1000,
                           help="Baud rate (default: 1000)")
    mod_group.add_argument("--m-fsk", type=int, choices=[2, 4, 8, 16], default=4,
                           help="M-FSK order (default: 4)")
    mod_group.add_argument("--ofdm", action="store_true",
                           help="Use OFDM modulation instead of FSK")
    mod_group.add_argument("--freq-min", type=int, default=200,
                           help="Min frequency Hz (default: 200)")
    mod_group.add_argument("--freq-max", type=int, default=18000,
                           help="Max frequency Hz (default: 18000)")
    mod_group.add_argument("--no-fec", action="store_true",
                           help="Disable FEC")
    mod_group.add_argument("--nsym", type=int, default=32,
                           help="FEC parity symbols (default: 32)")

    # Test parameters
    test_group = parser.add_argument_group("test parameters")
    test_group.add_argument("--payload-size", type=int, default=1024,
                            help="Payload bytes per test (default: 1024)")
    test_group.add_argument("--timeout", type=float, default=15.0,
                            help="Per-test timeout seconds (default: 15)")
    test_group.add_argument("--iterations", type=int, default=1,
                            help="Repeat each config N times (default: 1)")

    # Sweep options
    sweep_group = parser.add_argument_group("parameter sweep")
    sweep_group.add_argument("--sweep-baud", type=int, nargs="+", default=None,
                             metavar="BAUD",
                             help="Baud rates to sweep (e.g. 500 1000 2000)")
    sweep_group.add_argument("--sweep-mfsk", type=int, nargs="+",
                             choices=[2, 4, 8, 16], default=None,
                             help="M-FSK values to sweep (e.g. 2 4 8)")

    # Output
    parser.add_argument("--output-csv", type=str, default=None,
                        help="Path to write/append results CSV")

    return parser.parse_args()


def build_configs(args) -> list:
    """Build a list of Config objects for all parameter combinations to test."""
    baud_rates = args.sweep_baud if args.sweep_baud else [args.baud_rate]
    mfsk_values = args.sweep_mfsk if args.sweep_mfsk else [args.m_fsk]

    configs = []
    for baud in baud_rates:
        for mfsk in mfsk_values:
            config = Config()
            config.modulation.baud_rate = baud
            config.modulation.m_fsk = mfsk
            config.modulation.use_ofdm = args.ofdm
            config.modulation.freq_min = args.freq_min
            config.modulation.freq_max = args.freq_max
            config.fec.enabled = not args.no_fec
            config.fec.nsym = args.nsym
            config.audio.device_input_index = args.input_device
            config.audio.device_output_index = args.output_device
            configs.append(config)
    return configs


def run_single_test(config, payload_size, timeout) -> dict:
    """Run one loopback test and return results as a dict."""
    rng = np.random.default_rng(seed=42)
    payload = rng.integers(0, 256, size=payload_size, dtype=np.uint8).tobytes()

    tester = LoopbackTester(config)
    result = tester.run_test(payload, timeout=timeout)

    d = result.to_dict()
    d["baud_rate"] = config.modulation.baud_rate
    d["m_fsk"] = config.modulation.m_fsk
    d["fec_enabled"] = config.fec.enabled
    d["fec_nsym"] = config.fec.nsym
    d["freq_min"] = config.modulation.freq_min
    d["freq_max"] = config.modulation.freq_max
    d["timestamp"] = time.strftime("%Y-%m-%d %H:%M:%S")
    return d


CSV_FIELDS = [
    "timestamp", "baud_rate", "m_fsk", "fec_enabled", "fec_nsym",
    "freq_min", "freq_max", "payload_bytes_sent",
    "sync_acquired", "frames_received", "frames_valid",
    "frames_crc_fail", "frames_fec_fail",
    "payload_bytes_received", "elapsed_seconds",
    "success", "packet_loss", "throughput_bps", "error_message",
]


def append_csv(filepath: str, rows: list):
    """Append result rows to a CSV file."""
    if not rows:
        return
    is_new = not __import__("os").path.exists(filepath) or __import__("os").path.getsize(filepath) == 0
    with open(filepath, "a", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=CSV_FIELDS)
        if is_new:
            writer.writeheader()
        writer.writerows(rows)


def main():
    args = parse_args()

    log_level = logging.DEBUG if args.verbose else logging.INFO
    logging.basicConfig(
        level=log_level,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        datefmt="%H:%M:%S",
    )

    if args.list_devices:
        print_device_list()
        return

    logger.info("Speed of Sound WiFi - Loopback Test")
    logger.info("Input device:  %d", args.input_device)
    logger.info("Output device: %d", args.output_device)
    logger.info("")

    configs = build_configs(args)
    logger.info("Test configurations: %d (baud rates: %s, M-FSK: %s)",
                 len(configs),
                 [c.modulation.baud_rate for c in configs],
                 [c.modulation.m_fsk for c in configs])

    all_results = []

    for idx, config in enumerate(configs):
        baud = config.modulation.baud_rate
        mfsk = config.modulation.m_fsk
        fec = "on" if config.fec.enabled else "off"

        for iteration in range(args.iterations):
            header = f"Test {idx + 1}/{len(configs)}"
            if args.iterations > 1:
                header += f" (iter {iteration + 1}/{args.iterations})"
            logger.info("=" * 60)
            logger.info("%s: %d baud, %d-FSK, FEC %s", header, baud, mfsk, fec)
            logger.info("=" * 60)

            result = run_single_test(config, args.payload_size, args.timeout)
            all_results.append(result)

            status = "PASS" if result["success"] else "FAIL"
            logger.info("")
            logger.info("Result [%s]:", status)
            logger.info("  Sync: %s", "yes" if result["sync_acquired"] else "no")
            logger.info("  Frames: %d received, %d valid, %d CRC fail, %d FEC fail",
                         result["frames_received"], result["frames_valid"],
                         result["frames_crc_fail"], result["frames_fec_fail"])
            logger.info("  Payload: %d sent / %d received",
                         result["payload_bytes_sent"], result["payload_bytes_received"])
            logger.info("  Throughput: %.1f bps | Elapsed: %.2f s",
                         result["throughput_bps"], result["elapsed_seconds"])
            if result["error_message"]:
                logger.warning("  Error: %s", result["error_message"])
            logger.info("")

    # Write CSV
    if args.output_csv:
        append_csv(args.output_csv, all_results)
        logger.info("Results saved to %s (%d rows)", args.output_csv, len(all_results))

    # Summary table
    logger.info("=" * 90)
    logger.info("%-4s %-6s %-5s %-4s %-6s %-7s %-7s %-6s %s",
                "Run", "Baud", "M", "FEC", "Sync?", "Valid", "Recv'd", "Loss%", "Throughput")
    logger.info("-" * 90)
    for i, r in enumerate(all_results):
        loss_pct = r["packet_loss"] * 100
        logger.info("%-4d %-6d %-5d %-4s %-6s %-7d %-7d %-6.0f %.1f bps",
                     i + 1, r["baud_rate"], r["m_fsk"],
                     "Y" if r["fec_enabled"] else "N",
                     "Y" if r["sync_acquired"] else "N",
                     r["frames_valid"], r["payload_bytes_received"],
                     loss_pct, r["throughput_bps"])
    logger.info("=" * 90)

    # Determine overall pass/fail
    successes = sum(1 for r in all_results if r["success"])
    total = len(all_results)
    logger.info("")
    if successes == total:
        logger.info("ALL %d TESTS PASSED", total)
    else:
        logger.warning("%d/%d TESTS PASSED, %d FAILED",
                        successes, total, total - successes)

    return 0 if successes == total else 1


if __name__ == "__main__":
    sys.exit(main())
