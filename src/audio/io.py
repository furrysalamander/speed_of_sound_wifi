"""Audio stream management using sounddevice.

Handles low-latency simultaneous audio input/output streams.
"""

import logging
import queue
import threading
from typing import Callable, Optional

import numpy as np
import sounddevice as sd

from src.config import AudioConfig

logger = logging.getLogger(__name__)


class AudioStream:
    """Manages audio input/output streams with callback-based processing.

    Supports four modes:
    - TX only: output stream for transmitting audio
    - RX only: input stream for receiving audio
    - Full-duplex: simultaneous input and output (one combined stream)
    - Split-duplex: simultaneous input and output (two separate streams)

    Full-duplex requires both devices belong to the same host API.
    Split-duplex works with devices from different host APIs (e.g. USB
    microphone and motherboard analog output).
    """

    def __init__(
        self,
        config: AudioConfig,
        callback_rx: Optional[Callable] = None,
        callback_tx: Optional[Callable] = None,
    ):
        """
        Args:
            config: Audio configuration.
            callback_rx: Called with (samples, stream_info) for input data.
            callback_tx: Called with (frames_count, status) -> returns output samples.
        """
        self.config = config
        self.callback_rx = callback_rx
        self.callback_tx = callback_tx
        self.stream: Optional[sd.Stream] = None
        self._input_stream: Optional[sd.InputStream] = None
        self._output_stream: Optional[sd.OutputStream] = None
        self._running = False
        self._lock = threading.Lock()

        # Queues for inter-thread communication
        self.input_queue: queue.Queue = queue.Queue(maxsize=100)
        self.output_queue: queue.Queue = queue.Queue(maxsize=100)

    def _build_callback(self):
        """Build the combined callback for the duplex stream."""

        def callback(indata, outdata, frames, time_info, status):
            if status:
                logger.warning(f"Audio stream status: {status}")

            # Handle input (RX)
            if indata is not None and self.callback_rx:
                # Copy data to avoid buffer issues
                samples = indata.copy()
                if self.config.channels == 1:
                    samples = samples.flatten()
                self.callback_rx(samples, time_info)
                # Also queue for UI/waterfall
                try:
                    self.input_queue.put_nowait(samples.copy())
                except queue.Full:
                    pass  # drop if queue is full

            # Handle output (TX)
            if outdata is not None and self.callback_tx:
                output_samples = self.callback_tx(frames, status)
                if output_samples is not None:
                    if self.config.channels == 1:
                        outdata[:] = output_samples[:frames].reshape(-1, 1)
                    else:
                        outdata[:] = output_samples[:frames].reshape(-1, self.config.channels)

        return callback

    def _build_rx_callback(self):
        """Build callback for RX-only mode."""

        def callback(indata, frames, time_info, status):
            if status:
                logger.warning(f"Audio stream status: {status}")
            samples = indata.copy()
            if self.config.channels == 1:
                samples = samples.flatten()
            if self.callback_rx:
                self.callback_rx(samples, time_info)
            try:
                self.input_queue.put_nowait(samples.copy())
            except queue.Full:
                pass

        return callback

    def _build_tx_callback(self):
        """Build callback for TX-only mode."""

        def callback(outdata, frames, time_info, status):
            if status:
                logger.warning(f"Audio stream status: {status}")
            output_samples = self.callback_tx(frames, status)
            if output_samples is not None:
                if self.config.channels == 1:
                    outdata[:] = output_samples[:frames].reshape(-1, 1)
                else:
                    outdata[:] = output_samples[:frames].reshape(-1, self.config.channels)

        return callback

    def _build_split_duplex_callbacks(self, in_channels: int, out_channels: int):
        """Build rx/tx callbacks that handle different I/O channel counts."""

        def rx_callback(indata, frames, time_info, status):
            if status:
                logger.warning(f"Audio stream status: {status}")
            samples = indata.copy()
            # Flatten multi-channel to mono by averaging channels
            if in_channels > 1:
                samples = samples.mean(axis=1)
            else:
                samples = samples.flatten()
            if self.callback_rx:
                self.callback_rx(samples, time_info)
            try:
                self.input_queue.put_nowait(samples.copy())
            except queue.Full:
                pass

        def tx_callback(outdata, frames, time_info, status):
            if status:
                logger.warning(f"Audio stream status: {status}")
            output_samples = self.callback_tx(frames, status) if self.callback_tx else None
            if output_samples is not None:
                if out_channels > 1:
                    # Duplicate mono output to all channels
                    mono = output_samples[:frames]
                    outdata[:] = np.column_stack([mono] * out_channels)
                else:
                    outdata[:] = output_samples[:frames].reshape(-1, 1)
            else:
                outdata.fill(0)

        return rx_callback, tx_callback

    def _build_split_duplex(self):
        """Build separate input and output streams for split-duplex mode.

        Used when the input and output devices belong to different host APIs
        and cannot be combined in a single sd.Stream.  Uses the device's
        native channel count for each direction rather than a shared count.
        """
        input_device = (
            self.config.device_input_index
            if self.config.device_input_index is not None
            else None
        )
        output_device = (
            self.config.device_output_index
            if self.config.device_output_index is not None
            else None
        )

        # Use per-device channel counts in split mode
        try:
            devices = sd.query_devices()
            input_info = devices[input_device] if input_device is not None and input_device < len(devices) else None
            output_info = devices[output_device] if output_device is not None and output_device < len(devices) else None
        except Exception:
            input_info = None
            output_info = None

        in_channels = min(input_info["max_input_channels"], 2) if input_info else self.config.channels
        out_channels = min(output_info["max_output_channels"], 2) if output_info else self.config.channels

        logger.info("  Split-duplex: in ch=%d (max=%s), out ch=%d (max=%s)",
                     in_channels, input_info["max_input_channels"] if input_info else "?",
                     out_channels, output_info["max_output_channels"] if output_info else "?")

        rx_callback, tx_callback = self._build_split_duplex_callbacks(in_channels, out_channels)

        self._input_stream = sd.InputStream(
            samplerate=self.config.sample_rate,
            blocksize=self.config.buffer_size,
            channels=in_channels,
            dtype="float32",
            latency="low",
            device=input_device,
            callback=rx_callback,
        )
        self._output_stream = sd.OutputStream(
            samplerate=self.config.sample_rate,
            blocksize=self.config.buffer_size,
            channels=out_channels,
            dtype="float32",
            latency="low",
            device=output_device,
            callback=tx_callback,
        )
        self._input_stream.start()
        self._output_stream.start()

    def start(self, mode: str = "full-duplex"):
        """Start the audio stream.

        Args:
            mode: 'tx', 'rx', 'full-duplex', or 'split-duplex'
        """
        if self._running:
            logger.warning("Stream already running")
            return

        with self._lock:
            try:
                if mode == "full-duplex":
                    device = None
                    if self.config.device_input_index is not None or self.config.device_output_index is not None:
                        device = (
                            self.config.device_input_index,
                            self.config.device_output_index,
                        )
                    callback = self._build_callback()
                    self.stream = sd.Stream(
                        samplerate=self.config.sample_rate,
                        blocksize=self.config.buffer_size,
                        channels=self.config.channels,
                        dtype="float32",
                        latency="low",
                        device=device,
                        callback=callback,
                    )
                    self.stream.start()
                elif mode == "split-duplex":
                    self._build_split_duplex()
                elif mode == "rx":
                    callback = self._build_rx_callback()
                    device = (
                        self.config.device_input_index
                        if self.config.device_input_index is not None
                        else None
                    )
                    self.stream = sd.InputStream(
                        samplerate=self.config.sample_rate,
                        blocksize=self.config.buffer_size,
                        channels=self.config.channels,
                        dtype="float32",
                        latency="low",
                        device=device,
                        callback=callback,
                    )
                    self.stream.start()
                elif mode == "tx":
                    callback = self._build_tx_callback()
                    device = (
                        self.config.device_output_index
                        if self.config.device_output_index is not None
                        else None
                    )
                    self.stream = sd.OutputStream(
                        samplerate=self.config.sample_rate,
                        blocksize=self.config.buffer_size,
                        channels=self.config.channels,
                        dtype="float32",
                        latency="low",
                        device=device,
                        callback=callback,
                    )
                    self.stream.start()
                else:
                    raise ValueError(f"Unknown mode: {mode}")

                self._running = True
                logger.info(f"Audio stream started in {mode} mode")

            except sd.PortAudioError as e:
                logger.error(f"Failed to start audio stream: {e}")
                raise

    def stop(self):
        """Stop the audio stream."""
        with self._lock:
            if not self._running:
                return
            self._running = False

            if self.stream:
                self.stream.stop()
                self.stream.close()
                self.stream = None

            if self._input_stream:
                self._input_stream.stop()
                self._input_stream.close()
                self._input_stream = None

            if self._output_stream:
                self._output_stream.stop()
                self._output_stream.close()
                self._output_stream = None

            logger.info("Audio stream stopped")

    @property
    def is_running(self) -> bool:
        return self._running

    def get_input_samples(self, timeout: float = 0.1) -> Optional[np.ndarray]:
        """Get the latest input samples from the queue."""
        try:
            return self.input_queue.get(timeout=timeout)
        except queue.Empty:
            return None

    def queue_output_samples(self, samples: np.ndarray):
        """Queue samples for output (alternative to callback_tx)."""
        try:
            self.output_queue.put_nowait(samples)
        except queue.Full:
            pass

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.stop()
        return False
