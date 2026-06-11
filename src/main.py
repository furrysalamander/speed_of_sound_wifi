#!/usr/bin/env python3
"""Speed of Sound WiFi - Acoustically coupled data transmission system.

Main entry point for the application.

Modes:
  GUI      - Interactive PyQt6 interface (default)
  Headless - Software-only modulation/demodulation round-trip test
  Loopback - Hardware loopback test via physical audio cable
"""

import argparse
import csv
import logging
import sys

from src.config import Config
from src.audio.devices import print_device_list


def setup_logging(verbose: bool = False):
    """Configure logging."""
    level = logging.DEBUG if verbose else logging.INFO
    logging.basicConfig(
        level=level,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        datefmt="%H:%M:%S",
    )


def parse_args():
    """Parse command line arguments."""
    parser = argparse.ArgumentParser(
        description="Speed of Sound WiFi - Acoustically coupled data transmission"
    )
    parser.add_argument(
        "-v", "--verbose", action="store_true", help="Enable verbose/debug logging"
    )
    parser.add_argument(
        "--list-devices", action="store_true", help="List available audio devices and exit"
    )

    # Mode selection
    mode_group = parser.add_argument_group("mode selection")
    mode_group.add_argument(
        "--headless", action="store_true",
        help="Software modulation/demodulation round-trip (no audio hardware)"
    )
    mode_group.add_argument(
        "--loopback", action="store_true",
        help="Hardware loopback test via physical audio cable"
    )

    # Modulation / physical-layer parameters
    mod_group = parser.add_argument_group("modulation parameters")
    mod_group.add_argument(
        "--baud-rate", type=int, default=None, help="Baud rate (symbols/sec)"
    )
    mod_group.add_argument(
        "--m-fsk", type=int, choices=[2, 4, 8, 16], default=None,
        help="Number of FSK tones"
    )
    mod_group.add_argument(
        "--ofdm", action="store_true", help="Use OFDM modulation instead of FSK"
    )
    mod_group.add_argument(
        "--freq-min", type=int, default=None, help="Minimum frequency (Hz)"
    )
    mod_group.add_argument(
        "--freq-max", type=int, default=None, help="Maximum frequency (Hz)"
    )
    mod_group.add_argument(
        "--no-fec", action="store_true", help="Disable Forward Error Correction"
    )

    # Audio device selection
    dev_group = parser.add_argument_group("audio device selection")
    dev_group.add_argument(
        "--input-device", type=int, default=None,
        help="Input device index (use --list-devices to find)"
    )
    dev_group.add_argument(
        "--output-device", type=int, default=None,
        help="Output device index (use --list-devices to find)"
    )
    dev_group.add_argument(
        "--input-name", type=str, default=None,
        help="Input device name substring (e.g. 'USB_PnP')"
    )
    dev_group.add_argument(
        "--output-name", type=str, default=None,
        help="Output device name substring (e.g. 'analog-stereo')"
    )

    # Loopback-specific options
    loop_group = parser.add_argument_group("loopback test options")
    loop_group.add_argument(
        "--payload-size", type=int, default=1024,
        help="Test payload size in bytes (default: 1024)"
    )
    loop_group.add_argument(
        "--test-timeout", type=float, default=15.0,
        help="Max test duration in seconds (default: 15.0)"
    )
    loop_group.add_argument(
        "--output-csv", type=str, default=None,
        help="Append results to CSV file"
    )
    loop_group.add_argument(
        "--sweep-baud", type=int, nargs="+", default=None,
        metavar="BAUD",
        help="Sweep one or more baud rates (e.g. 500 1000 2000 3000 5000)"
    )

    return parser.parse_args()


def create_config(args) -> Config:
    """Create configuration from command line arguments."""
    config = Config()

    # Apply CLI overrides
    if args.baud_rate:
        config.modulation.baud_rate = args.baud_rate
    if args.m_fsk:
        config.modulation.m_fsk = args.m_fsk
    if args.ofdm:
        config.modulation.use_ofdm = True
    if args.freq_min:
        config.modulation.freq_min = args.freq_min
    if args.freq_max:
        config.modulation.freq_max = args.freq_max
    if args.input_device is not None:
        config.audio.device_input_index = args.input_device
    if args.output_device is not None:
        config.audio.device_output_index = args.output_device
    if args.input_name is not None:
        config.audio.device_input_name = args.input_name
    if args.output_name is not None:
        config.audio.device_output_name = args.output_name
    if args.no_fec:
        config.fec.enabled = False

    return config


def run_headless(config: Config):
    """Run in headless mode (no GUI) for testing."""
    import time
    import numpy as np

    from src.physical.modulator import FskModulator
    from src.physical.demodulator import FskDemodulator
    from src.link.framing import FrameAssembler, FrameParser
    from src.link.fec import ReedSolomonFec

    logger = logging.getLogger(__name__)

    logger.info("Running in headless mode")
    logger.info(f"Config: {config.theoretical_bps} bps theoretical, "
                f"{config.bits_per_symbol} bits/symbol, "
                f"M={config.modulation.m_fsk}")
    logger.info(f"FSK frequencies: {config.fsk_frequencies}")

    # Test modulation/demodulation round-trip
    modulator = FskModulator(config)
    demodulator = FskDemodulator(config)
    assembler = FrameAssembler(config)
    parser = FrameParser(config)

    test_data = b"Hello, Speed of Sound WiFi! This is a test message. 1234567890"
    logger.info(f"Test data ({len(test_data)} bytes): {test_data}")

    # Assemble frame
    frame = assembler.assemble_frame(test_data)
    logger.info(f"Frame assembled ({len(frame)} bytes)")

    # Modulate to audio
    audio = modulator.modulate_with_preamble(frame)
    logger.info(f"Audio signal generated ({len(audio)} samples, "
                f"{len(audio) / config.audio.sample_rate:.3f}s)")

    # Simulate demodulation (direct loopback, no noise)
    # Feed all audio samples to demodulator
    symbols = demodulator.process_samples(audio)
    if symbols is not None:
        logger.info(f"Demodulated {len(symbols)} symbols")

        # Estimate original byte count and decode
        estimated_bytes = (len(symbols) * config.bits_per_symbol) // 8
        decoded = demodulator.symbols_to_bytes(symbols, estimated_bytes)
        logger.info(f"Decoded {len(decoded)} bytes: {decoded[:len(test_data)]}")

        # Check if test data is in decoded output
        if test_data in decoded:
            logger.info("SUCCESS: Test data recovered correctly!")
        else:
            logger.warning("Test data not found in decoded output (expected with current framing)")

    # Print device list
    print_device_list()


def run_loopback(config: Config, payload_size: int = 1024, timeout: float = 15.0,
                 output_csv: str = None, sweep_baud: list = None):
    """Run hardware loopback test via physical audio cable.

    Args:
        config: Test configuration.
        payload_size: Number of bytes to send per test.
        timeout: Maximum test duration in seconds.
        output_csv: Optional path to append CSV results.
        sweep_baud: Optional list of baud rates to sweep.
    """
    import time
    import numpy as np

    from src.audio.loopback import LoopbackTester, LoopbackTestResult
    from src.audio.devices import resolve_device

    logger = logging.getLogger(__name__)

    # Resolve device name substrings to indices before creating the tester
    resolve_device(config.audio)
    if config.audio.device_input_index is None or config.audio.device_output_index is None:
        logger.warning("Input or output device not set via --input-device / --output-device")
        logger.warning("Using default system audio devices (may not be correct)")
        print_device_list()
        logger.warning("Re-run with --input-device N --output-device N for reliable results")

    # Determine which baud rates to test
    baud_rates = sweep_baud if sweep_baud else [config.modulation.baud_rate]

    # Prepare CSV file
    csv_file = None
    csv_writer = None
    if output_csv:
        csv_file = open(output_csv, "a", newline="")
        fieldnames = [
            "baud_rate", "m_fsk", "fec_enabled", "fec_nsym",
            "freq_min", "freq_max", "payload_bytes_sent",
            "sync_acquired", "frames_received", "frames_valid",
            "frames_crc_fail", "frames_fec_fail",
            "payload_bytes_received", "elapsed_seconds",
            "success", "packet_loss", "throughput_bps",
            "error_message", "timestamp",
        ]
        csv_writer = csv.DictWriter(csv_file, fieldnames=fieldnames)
        # Write header if file is new
        if csv_file.tell() == 0:
            csv_writer.writeheader()
        csv_file.flush()

    all_results = []

    for baud_rate in baud_rates:
        test_config = Config()
        test_config.modulation.baud_rate = baud_rate
        test_config.modulation.use_ofdm = config.modulation.use_ofdm
        test_config.modulation.m_fsk = config.modulation.m_fsk
        test_config.modulation.freq_min = config.modulation.freq_min
        test_config.modulation.freq_max = config.modulation.freq_max
        test_config.ofdm.subcarrier_min = config.ofdm.subcarrier_min
        test_config.ofdm.subcarrier_max = config.ofdm.subcarrier_max
        test_config.ofdm.bits_per_subcarrier = config.ofdm.bits_per_subcarrier
        test_config.ofdm.preamble_symbols = config.ofdm.preamble_symbols
        test_config.fec.enabled = config.fec.enabled
        test_config.fec.nsym = config.fec.nsym
        test_config.audio.device_input_index = config.audio.device_input_index
        test_config.audio.device_output_index = config.audio.device_output_index
        test_config.audio.sample_rate = config.audio.sample_rate
        test_config.audio.buffer_size = config.audio.buffer_size

        logger.info("=" * 50)
        if test_config.modulation.use_ofdm:
            logger.info("LOOPBACK TEST: OFDM, SC %d-%d (%d subcarriers), nsym=%d",
                         test_config.ofdm.subcarrier_min, test_config.ofdm.subcarrier_max,
                         test_config.ofdm_subcarrier_count, test_config.fec.nsym)
        else:
            logger.info("LOOPBACK TEST: %d baud, %d-FSK, nsym=%d",
                         baud_rate, test_config.modulation.m_fsk, test_config.fec.nsym)
        logger.info("=" * 50)

        # Generate deterministic test payload
        rng = np.random.default_rng(seed=42)
        payload = rng.integers(0, 256, size=payload_size, dtype=np.uint8).tobytes()

        tester = LoopbackTester(test_config)
        result = tester.run_test(payload, timeout=timeout)

        # Fill in config values for CSV
        result_dict = result.to_dict()
        result_dict["baud_rate"] = test_config.modulation.baud_rate
        result_dict["m_fsk"] = test_config.modulation.m_fsk
        result_dict["fec_enabled"] = test_config.fec.enabled
        result_dict["fec_nsym"] = test_config.fec.nsym
        result_dict["freq_min"] = test_config.modulation.freq_min
        result_dict["freq_max"] = test_config.modulation.freq_max
        result_dict["timestamp"] = time.strftime("%Y-%m-%d %H:%M:%S")

        # Print summary
        status = "PASS" if result.success else "FAIL"
        logger.info("---")
        if test_config.modulation.use_ofdm:
            logger.info("RESULT [%s]: OFDM", status)
        else:
            logger.info("RESULT [%s]: %d baud, %d-FSK", status, baud_rate,
                         test_config.modulation.m_fsk)
        logger.info("  Sync: %s | Frames: %d valid / %d received",
                     "yes" if result.sync_acquired else "no",
                     result.frames_valid, result.frames_received)
        logger.info("  Payload: %d sent / %d received",
                     result.payload_bytes_sent, result.payload_bytes_received)
        logger.info("  CRC fails: %d | FEC fails: %d",
                     result.frames_crc_fail, result.frames_fec_fail)
        logger.info("  Throughput: %.1f bps | Elapsed: %.2f s",
                     result.throughput_bps, result.elapsed_seconds)
        if result.error_message:
            logger.warning("  Error: %s", result.error_message)
        logger.info("")

        all_results.append(result_dict)

        if csv_writer:
            csv_writer.writerow(result_dict)
            csv_file.flush()

    if csv_file:
        csv_file.close()
        logger.info("Results appended to %s", output_csv)

    # Print summary table
    logger.info("=" * 70)
    logger.info("%-6s %-6s %-6s %-8s %-8s %-8s %s",
                "Baud", "M", "FEC", "Sync?", "Frames", "Bytes", "Throughput")
    logger.info("-" * 70)
    for r in all_results:
        if r.get("use_ofdm"):
            logger.info("OFDM  %-6s %-6s %-8s %-8d %-8d %.1f bps",
                         "OFDM", "Y" if r["fec_enabled"] else "N",
                         "Y" if r["sync_acquired"] else "N",
                         r["frames_valid"], r["payload_bytes_received"],
                         r["throughput_bps"])
        else:
            logger.info("%-6d %-6d %-6s %-8s %-8d %-8d %.1f bps",
                         r["baud_rate"], r["m_fsk"],
                         "Y" if r["fec_enabled"] else "N",
                         "Y" if r["sync_acquired"] else "N",
                         r["frames_valid"], r["payload_bytes_received"],
                         r["throughput_bps"])
    logger.info("=" * 70)


def run_gui(config: Config):
    """Run the GUI application."""
    from PyQt6.QtWidgets import QApplication
    from src.ui.main_window import MainWindow

    app = QApplication(sys.argv)
    app.setStyle("Fusion")  # Cross-platform consistent style

    window = MainWindow(config)
    window.show()

    sys.exit(app.exec())


def main():
    """Main entry point."""
    args = parse_args()
    setup_logging(args.verbose)

    logger = logging.getLogger(__name__)

    if args.list_devices:
        print_device_list()
        return

    config = create_config(args)

    logger.info(f"Speed of Sound WiFi v0.1.0")
    logger.info(f"Sample rate: {config.audio.sample_rate} Hz")
    if config.modulation.use_ofdm:
        sub_count = config.ofdm_subcarrier_count
        sym_rate = config.ofdm_symbol_rate
        logger.info(f"OFDM: {config.ofdm.subcarrier_min}-{config.ofdm.subcarrier_max} "
                    f"({sub_count} subcarriers, {config.ofdm.bits_per_subcarrier} bit/sc)")
        logger.info(f"OFDM param: FFT={config.ofdm.fft_size} CP={config.ofdm.cp_length} "
                    f"preamble={config.ofdm.preamble_symbols} sym")
        logger.info(f"Symbol rate: {sym_rate:.1f} Hz, {sym_rate * sub_count * config.ofdm.bits_per_subcarrier:.0f} bps raw")
    else:
        logger.info(f"Baud rate: {config.modulation.baud_rate} symbols/sec")
        logger.info(f"M-FSK: {config.modulation.m_fsk} tones ({config.bits_per_symbol} bits/symbol)")
        logger.info(f"Freq range: {config.modulation.freq_min}-{config.modulation.freq_max} Hz")
    logger.info(f"FEC: {'Enabled' if config.fec.enabled else 'Disabled'} "
                f"(RS nsym={config.fec.nsym})")

    if args.headless:
        run_headless(config)
    elif args.loopback:
        run_loopback(
            config,
            payload_size=args.payload_size,
            timeout=args.test_timeout,
            output_csv=args.output_csv,
            sweep_baud=args.sweep_baud,
        )
    else:
        run_gui(config)


if __name__ == "__main__":
    main()
