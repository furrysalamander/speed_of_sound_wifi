"""Loopback testing for audio data transmission via physical cable.

Sends modulated audio through the output device, captures it through the
input device, and verifies the received data matches the original.
"""

import logging
import math
import time
from typing import Optional

import numpy as np

from src.config import Config
from src.audio.io import AudioStream
from src.physical.modulator import FskModulator
from src.physical.demodulator import FskDemodulator
from src.link.framing import FrameAssembler, FrameParser

logger = logging.getLogger(__name__)


class LoopbackTestResult:
    """Results from a single loopback test run."""

    def __init__(self):
        self.sync_acquired: bool = False
        self.frames_sent: int = 0
        self.frames_received: int = 0
        self.frames_valid: int = 0
        self.frames_crc_fail: int = 0
        self.frames_fec_fail: int = 0
        self.fec_corrections: int = 0
        self.payload_bytes_sent: int = 0
        self.payload_bytes_received: int = 0
        self.elapsed_seconds: float = 0.0
        self.test_payload: bytes = b""
        self.received_payload: bytes = b""
        self.packet_loss: float = 1.0
        self.throughput_bps: float = 0.0
        self.error_message: str = ""

    @property
    def success(self) -> bool:
        return self.payload_bytes_received == self.payload_bytes_sent

    def to_dict(self) -> dict:
        return {
            "baud_rate": 0,
            "m_fsk": 0,
            "fec_enabled": True,
            "fec_nsym": 0,
            "freq_min": 0,
            "freq_max": 0,
            "sync_acquired": self.sync_acquired,
            "frames_received": self.frames_received,
            "frames_valid": self.frames_valid,
            "frames_crc_fail": self.frames_crc_fail,
            "frames_fec_fail": self.frames_fec_fail,
            "payload_bytes_sent": self.payload_bytes_sent,
            "payload_bytes_received": self.payload_bytes_received,
            "elapsed_seconds": self.elapsed_seconds,
            "success": self.success,
            "packet_loss": self.packet_loss,
            "throughput_bps": self.throughput_bps,
            "error_message": self.error_message,
        }


class LoopbackTester:
    """Tests the full TX -> audio -> RX chain via a physical loopback cable."""

    def __init__(self, config: Config):
        self.config = config
        self.modulator = FskModulator(config)
        self.demodulator = FskDemodulator(config)
        self.assembler = FrameAssembler(config)
        self.parser = FrameParser(config)
        self.stream: Optional[AudioStream] = None

    def run_test(self, payload: bytes, timeout: float = 15.0) -> LoopbackTestResult:
        """Run one loopback test.

        Args:
            payload: Data bytes to send.
            timeout: Maximum time to wait for completion (seconds).

        Returns:
            LoopbackTestResult with test statistics.
        """
        result = LoopbackTestResult()
        result.test_payload = payload
        result.payload_bytes_sent = len(payload)

        logger.info("Starting loopback test")
        logger.info("  Payload: %d bytes", len(payload))
        logger.info("  Baud rate: %d sym/s", self.config.modulation.baud_rate)
        logger.info("  M-FSK: %d (%d bits/symbol)",
                     self.config.modulation.m_fsk,
                     self.config.bits_per_symbol)
        logger.info("  FEC: %s (nsym=%d)",
                     "enabled" if self.config.fec.enabled else "disabled",
                     self.config.fec.nsym)
        logger.info("  Freq range: %d-%d Hz",
                     self.config.modulation.freq_min,
                     self.config.modulation.freq_max)
        logger.info("  Input device: %s", self.config.audio.device_input_index)
        logger.info("  Output device: %s", self.config.audio.device_output_index)

        # Assemble frame and modulate
        frame = self.assembler.assemble_frame(payload)
        logger.info("  Frame assembled (%d bytes)", len(frame))
        tx_audio = self.modulator.modulate_with_preamble(frame)

        # Add trailing silence so RX captures the full signal tail
        silence_len = int(self.config.audio.sample_rate * 0.5)
        silence = np.zeros(silence_len, dtype=np.float32)
        tx_audio = np.concatenate([tx_audio, silence])

        total_tx_samples = len(tx_audio)
        tx_duration = total_tx_samples / self.config.audio.sample_rate
        logger.info("  TX audio: %d samples (%.2f s)", total_tx_samples, tx_duration)

        tx_pos = [0]
        rx_buffers = []
        callback_errors = []

        def tx_callback(frames, status):
            start = tx_pos[0]
            end = min(start + frames, len(tx_audio))
            chunk = np.zeros(frames, dtype=np.float32)
            if start < len(tx_audio):
                available = tx_audio[start:end]
                chunk[:len(available)] = available
            tx_pos[0] = end
            return chunk

        def rx_callback(samples, time_info):
            rx_buffers.append(samples.copy())

        self.stream = AudioStream(
            self.config.audio,
            callback_rx=rx_callback,
            callback_tx=tx_callback,
        )
        try:
            self.stream.start("full-duplex")
        except Exception as e:
            logger.info("  Full-duplex failed (%s), trying split-duplex...", e)
            try:
                self.stream.start("split-duplex")
            except Exception as e2:
                logger.error("Failed to start audio stream (both modes): %s", e2)
                result.error_message = f"Audio stream error: {e2}"
                return result

        test_start = time.time()
        wait_time = min(tx_duration + 2.0, timeout)
        logger.info("  Running for %.2f s...", wait_time)
        time.sleep(wait_time)

        self.stream.stop()
        result.elapsed_seconds = time.time() - test_start
        logger.info("  Elapsed: %.2f s", result.elapsed_seconds)

        # Concatenate all RX buffers
        if not rx_buffers:
            logger.warning("  No RX samples collected")
            result.error_message = "No RX samples collected"
            return result
        all_rx = np.concatenate(rx_buffers)
        logger.info("  RX samples: %d (%.2f s)",
                     len(all_rx), len(all_rx) / self.config.audio.sample_rate)

        # Crop leading silence so the demodulator's preamble search works
        # with a reasonable search range.
        symbol_samples = self.config.symbol_duration_samples
        window_size = max(1, symbol_samples // 4)
        num_windows = len(all_rx) // window_size
        if num_windows > 0:
            rx_trim = all_rx[:num_windows * window_size]
            windows = rx_trim.reshape(-1, window_size)
            energies = np.sqrt(np.mean(windows ** 2, axis=1))
            energy_threshold = max(np.max(energies) * 0.05, 0.001)
            above = energies > energy_threshold
            above_streak = np.convolve(above.astype(np.int32), np.ones(5, dtype=np.int32), mode='same') >= 5
            signal_start_idx = np.argmax(above_streak) if np.any(above_streak) else 0
            signal_start = signal_start_idx * window_size
            if signal_start > 0:
                all_rx = all_rx[signal_start:]
                logger.info("  Cropped %d samples (%.0f ms) of leading silence",
                            signal_start, signal_start / self.config.audio.sample_rate * 1000)
        else:
            logger.info("  No silence cropping (RX buffer too short)")

        demod = FskDemodulator(self.config)
        symbols = demod.process_samples(all_rx)
        if symbols is None or len(symbols) == 0:
            logger.warning("  No symbols detected after cropping")
            result.error_message = "No symbols detected"
            return result
        result.sync_acquired = True
        logger.info("  Symbols: %d total", len(symbols))
        logger.info("  First 20 symbols: %s", symbols[:20].tolist())

        # Compute generous byte estimate: frame bytes + preamble margin
        frame_size_est = 8 + 4 + len(payload) + self.config.fec.nsym + 4
        if not self.config.fec.enabled:
            frame_size_est = 8 + 4 + len(payload) + 4
        # FEC encoder pads to full RS blocks of 255 bytes
        rs_block_data = 255 - self.config.fec.nsym
        if self.config.fec.enabled:
            data_for_fec = 4 + len(payload)
            num_blocks = math.ceil(data_for_fec / rs_block_data)
            fec_size = num_blocks * 255
            frame_size_est = 8 + fec_size + 4
        byte_est = frame_size_est + 64  # +64 for preamble margin

        decoded_bytes = self.demodulator.symbols_to_bytes(symbols, byte_est)
        logger.info("  Decoded %d bytes", len(decoded_bytes))
        logger.info("  First 24 decoded bytes hex: %s", decoded_bytes[:24].hex())
        logger.info("  Symbols 60-80: %s", symbols[60:81].tolist() if len(symbols) >= 81 else symbols[60:].tolist())

        # Feed to frame parser
        frames = self.parser.feed_bytes(decoded_bytes)
        parser_stats = self.parser.stats
        result.frames_received = parser_stats["frames_received"]
        result.frames_valid = parser_stats["frames_valid"]
        result.frames_crc_fail = parser_stats["frames_crc_fail"]
        result.frames_fec_fail = parser_stats["frames_fec_fail"]

        for f_payload, seq, frame_type, valid in frames:
            if valid:
                result.received_payload = f_payload
                if f_payload == payload:
                    result.payload_bytes_received = len(f_payload)
                    logger.info("  SUCCESS: payload received correctly (%d bytes)",
                                len(f_payload))
                else:
                    logger.warning("  Payload mismatch (%d B sent, %d B received)",
                                   len(payload), len(f_payload))
            else:
                logger.warning("  Invalid frame received")

        if result.payload_bytes_received > 0:
            result.packet_loss = 0.0
            tx_time = max(result.elapsed_seconds, 0.001)
            result.throughput_bps = result.payload_bytes_received * 8 / tx_time
            logger.info("  Throughput: %.1f bps (%.2f kB/s)",
                        result.throughput_bps,
                        result.throughput_bps / 8000)
        else:
            result.packet_loss = 1.0
            if not result.error_message:
                result.error_message = "No valid frames received"

        if callback_errors:
            logger.warning("  RX callback errors: %s", callback_errors)

        return result
