"""Main application window for the audio data transmission system.

Provides the complete UI with waterfall displays, controls, and status indicators.
"""

import logging
import os
import queue
import threading
import time
from typing import Optional

import numpy as np
from PyQt6.QtCore import QThread, QTimer, pyqtSignal, Qt
from PyQt6.QtGui import QAction, QFont, QIcon
from PyQt6.QtWidgets import (
    QComboBox,
    QFileDialog,
    QHBoxLayout,
    QLabel,
    QMainWindow,
    QMessageBox,
    QPushButton,
    QSplitter,
    QStatusBar,
    QVBoxLayout,
    QWidget,
)

from src.application.fileio import write_file_data

from src.audio.devices import list_input_devices, list_output_devices
from src.audio.io import AudioStream
from src.application.protocol import FileTransferProtocol, ProtocolCommand
from src.config import Config
from src.link.framing import FrameParser
from src.physical.demodulator import FskDemodulator
from src.physical.modulator import FskModulator
from src.ui.controls import ParameterControls, StatsDisplay, TransferProgressWidget
from src.ui.waterfall import WaterfallPlot

logger = logging.getLogger(__name__)


class AudioWorkerThread(QThread):
    """Background thread for audio processing and data transmission.

    Handles the audio stream, modulation/demodulation, and protocol state machine
    in a separate thread to keep the UI responsive.
    """

    # Signals to update UI
    signal_waterfall_tx = pyqtSignal(object)  # numpy array
    signal_waterfall_rx = pyqtSignal(object)  # numpy array
    signal_stats_update = pyqtSignal(object)  # dict
    signal_progress_update = pyqtSignal(object)  # dict
    signal_status_message = pyqtSignal(str)
    signal_transfer_complete = pyqtSignal(bool, str)  # (success, message)

    def __init__(self, config: Config, mode: str = "full-duplex"):
        super().__init__()
        self.config = config
        self.mode = mode  # "full-duplex", "tx", "rx"

        self.modulator: Optional[FskModulator] = None
        self.demodulator: Optional[FskDemodulator] = None
        self.frame_parser: Optional[FrameParser] = None
        self.audio_stream: Optional[AudioStream] = None
        self.protocol: Optional[FileTransferProtocol] = None

        self._running = False
        self._output_path: Optional[str] = None
        self._tx_buffer: queue.Queue = queue.Queue()
        self._current_tx_audio: np.ndarray = np.zeros(1)
        self._tx_position: int = 0

        # Stats tracking
        self._frames_sent: int = 0
        self._frames_received: int = 0
        self._bytes_transferred: int = 0
        self._start_time: float = 0
        self._fec_corrections: int = 0
        self._retransmissions: int = 0

    def setup(self):
        """Initialize components."""
        self.modulator = FskModulator(self.config)
        self.demodulator = FskDemodulator(self.config)
        self.frame_parser = FrameParser(self.config)
        self.protocol = FileTransferProtocol(self.config)

        # Setup protocol callbacks
        self.protocol.on_frame_send = self._on_frame_send
        self.protocol.on_progress_update = self._on_progress_update

        # Setup audio stream
        self.audio_stream = AudioStream(
            self.config.audio,
            callback_rx=self._on_audio_rx,
            callback_tx=self._on_audio_tx,
        )

    def run(self):
        """Main thread loop."""
        self._running = True
        self._start_time = time.time()

        try:
            self.audio_stream.start(self.mode)
            self.signal_status_message.emit("Audio stream started")

            # Process until stopped
            while self._running:
                # Check for TX data
                self._process_tx()

                # Small sleep to prevent busy-waiting
                self.msleep(10)

        except Exception as e:
            logger.error(f"Audio worker error: {e}", exc_info=True)
            self.signal_status_message.emit(f"Error: {e}")
        finally:
            self.cleanup()

    def _process_tx(self):
        """Process transmit data queue."""
        # Check if we have audio data to send
        if hasattr(self, "_current_tx_audio") and len(self._current_tx_audio) > 0:
            # The audio callback will read from this buffer
            pass

    def _on_audio_tx(self, frames: int, status) -> Optional[np.ndarray]:
        """Callback for audio output (TX).

        Args:
            frames: Number of frames to generate.
            status: Stream status flags.

        Returns:
            Audio samples to output.
        """
        if len(self._current_tx_audio) > 0 and self._tx_position < len(self._current_tx_audio):
            end = min(self._tx_position + frames, len(self._current_tx_audio))
            output = self._current_tx_audio[self._tx_position : end]
            self._tx_position = end

            # Pad if needed
            if len(output) < frames:
                output = np.pad(output, (0, frames - len(output)))

            # Send to TX waterfall (every callback chunk)
            try:
                self.signal_waterfall_tx.emit(output.copy())
            except Exception:
                pass

            # Reset position if buffer exhausted
            if self._tx_position >= len(self._current_tx_audio):
                self._current_tx_audio = np.zeros(1)
                self._tx_position = 0

            return output.astype(np.float32)
        else:
            # Silence
            return np.zeros(frames, dtype=np.float32)

    def _on_audio_rx(self, samples: np.ndarray, time_info):
        """Callback for audio input (RX).

        Args:
            samples: Input audio samples.
            time_info: Stream timing information.
        """
        # Send to waterfall
        try:
            self.signal_waterfall_rx.emit(samples.copy())
        except Exception:
            pass

        # Demodulate
        if self.demodulator:
            symbols = self.demodulator.process_samples(samples)
            if symbols is not None:
                # Convert symbols to bytes
                estimated_bytes = (len(symbols) * self.config.bits_per_symbol) // 8
                decoded_bytes = self.demodulator.symbols_to_bytes(symbols, estimated_bytes)

                # Feed bytes to frame parser (returns list of tuples)
                # Each tuple: (payload, seq_number, frame_type, is_valid)
                if self.frame_parser:
                    frames = self.frame_parser.feed_bytes(decoded_bytes)
                    for payload, seq, frame_type, valid in frames:
                        self._frames_received += 1
                        self._bytes_transferred += len(payload)

                        # Feed parsed frame to protocol
                        if self.protocol:
                            self.protocol.handle_received_frame(payload, seq, frame_type, valid)

    def _on_frame_send(self, frame_bytes: bytes):
        """Handle frame transmission request from protocol.

        Args:
            frame_bytes: Complete frame bytes to modulate and transmit.
        """
        if not self.modulator:
            return

        self._frames_sent += 1

        # Modulate frame to audio
        audio = self.modulator.modulate_with_preamble(frame_bytes)
        self._current_tx_audio = audio
        self._tx_position = 0

        # Send to TX waterfall
        try:
            self.signal_waterfall_tx.emit(audio[: len(audio) // 2])
        except Exception:
            pass

    def _on_progress_update(self, progress):
        """Handle progress update from protocol."""
        progress_data = {
            "state": progress.state,
            "filename": progress.filename,
            "file_size": progress.file_size,
            "bytes_transferred": progress.bytes_transferred,
            "progress_percent": progress.progress_percent,
            "throughput_string": progress.throughput_string,
            "elapsed_time": progress.elapsed_time,
            "frames_sent": progress.frames_sent,
            "frames_received": progress.frames_received,
            "frames_retransmitted": progress.frames_retransmitted,
        }
        try:
            self.signal_progress_update.emit(progress_data)
        except Exception:
            pass

        # Check for completion
        if progress.state in ("complete", "failed"):
            success = progress.state == "complete"
            if success and self._output_path:
                try:
                    write_file_data(self._output_path, bytes(self.protocol._received_data), overwrite=True)
                    msg = f"Transfer complete. Saved to {os.path.basename(self._output_path)}"
                except Exception as e:
                    success = False
                    msg = f"Failed to save file: {e}"
            else:
                msg = "Transfer complete" if success else f"Transfer failed: {progress.error_message}"
            self.signal_transfer_complete.emit(success, msg)

    def send_file(self, filepath: str):
        """Queue a file for transmission.

        Args:
            filepath: Path to the file to send.
        """
        self._start_time = time.time()
        self.protocol.initiate_send(filepath)
        self.signal_status_message.emit(f"Sending: {os.path.basename(filepath)}")

    def start_receive(self, output_path: str):
        """Start receiving a file.

        Args:
            output_path: Path to save the received file.
        """
        self._start_time = time.time()
        self._output_path = output_path
        self.protocol.initiate_receive()
        self.signal_status_message.emit("Ready to receive")

    def update_stats(self):
        """Emit current stats to UI."""
        elapsed = time.time() - self._start_time if self._start_time > 0 else 1
        throughput = self._bytes_transferred / elapsed if elapsed > 0 else 0

        stats = {
            "throughput_bps": throughput,
            "frames_sent": self._frames_sent,
            "frames_received": self._frames_received,
            "retransmissions": self._retransmissions,
            "fec_corrections": self._fec_corrections,
        }
        try:
            self.signal_stats_update.emit(stats)
        except Exception:
            pass

    def cleanup(self):
        """Clean up resources."""
        self._running = False
        if self.audio_stream and self.audio_stream.is_running:
            self.audio_stream.stop()

    def stop(self):
        """Signal the thread to stop."""
        self._running = False


class MainWindow(QMainWindow):
    """Main application window."""

    def __init__(self, config: Config):
        super().__init__()
        self.config = config
        self.worker: Optional[AudioWorkerThread] = None

        self._setup_ui()
        self._setup_menu()
        self._setup_connections()

        # Update timer for stats
        self._stats_timer = QTimer(self)
        self._stats_timer.timeout.connect(self._update_stats)
        self._stats_timer.start(1000)  # Update every second

    def _setup_ui(self):
        """Setup the main window UI."""
        self.setWindowTitle(self.config.ui.title)
        self.resize(1400, 900)

        # Central widget
        central = QWidget()
        self.setCentralWidget(central)
        main_layout = QVBoxLayout(central)

        # Control bar
        control_bar = self._create_control_bar()
        main_layout.addWidget(control_bar)

        # Main content splitter
        splitter = QSplitter(Qt.Orientation.Horizontal)

        # Left panel: TX waterfall
        tx_panel = self._create_panel("Transmit (TX)")
        self.tx_waterfall = WaterfallPlot(self.config, "Transmit Spectrum", tx_panel)
        tx_layout = QVBoxLayout(tx_panel)
        tx_layout.addWidget(self.tx_waterfall)

        # Right panel: RX waterfall
        rx_panel = self._create_panel("Receive (RX)")
        self.rx_waterfall = WaterfallPlot(self.config, "Receive Spectrum", rx_panel)
        rx_layout = QVBoxLayout(rx_panel)
        rx_layout.addWidget(self.rx_waterfall)

        splitter.addWidget(tx_panel)
        splitter.addWidget(rx_panel)
        splitter.setStretchFactor(0, 1)
        splitter.setStretchFactor(1, 1)
        main_layout.addWidget(splitter, stretch=1)

        # Bottom panel: stats and progress
        bottom_panel = self._create_bottom_panel()
        main_layout.addWidget(bottom_panel)

        # Status bar
        self.status_bar = QStatusBar()
        self.setStatusBar(self.status_bar)
        self.status_label = QLabel("Ready")
        self.status_bar.addWidget(self.status_label)

    def _create_control_bar(self) -> QWidget:
        """Create the top control bar with buttons and device selection."""
        widget = QWidget()
        layout = QHBoxLayout(widget)
        layout.setContentsMargins(5, 5, 5, 5)

        # Mode selection
        layout.addWidget(QLabel("Mode:"))
        self.mode_combo = QComboBox()
        self.mode_combo.addItems(["Full-Duplex", "TX Only", "RX Only"])
        layout.addWidget(self.mode_combo)

        layout.addSpacing(10)

        # Start/Stop buttons
        self.btn_start = QPushButton("▶ Start")
        self.btn_start.setStyleSheet(
            "QPushButton { background-color: #4CAF50; color: white; "
            "font-weight: bold; padding: 8px 16px; border-radius: 4px; }"
        )
        self.btn_start.clicked.connect(self._on_start_stop)
        layout.addWidget(self.btn_start)

        self.btn_stop = QPushButton("⏹ Stop")
        self.btn_stop.setStyleSheet(
            "QPushButton { background-color: #f44336; color: white; "
            "font-weight: bold; padding: 8px 16px; border-radius: 4px; }"
        )
        self.btn_stop.clicked.connect(self._on_stop)
        self.btn_stop.setEnabled(False)
        layout.addWidget(self.btn_stop)

        layout.addSpacing(20)

        # File transfer buttons (enabled by default — user picks file first, then starts audio)
        self.btn_send = QPushButton("📤 Send File")
        self.btn_send.clicked.connect(self._on_send_file)
        layout.addWidget(self.btn_send)

        self.btn_receive = QPushButton("📥 Receive File")
        self.btn_receive.clicked.connect(self._on_receive_file)
        layout.addWidget(self.btn_receive)

        layout.addStretch()

        # Device selection
        layout.addWidget(QLabel("Input Device:"))
        self.input_device_combo = QComboBox()
        self._populate_devices(self.input_device_combo, list_input_devices())
        layout.addWidget(self.input_device_combo)

        layout.addWidget(QLabel("Output Device:"))
        self.output_device_combo = QComboBox()
        self._populate_devices(self.output_device_combo, list_output_devices())
        layout.addWidget(self.output_device_combo)

        return widget

    def _create_panel(self, title: str) -> QWidget:
        """Create a panel widget with a title."""
        return QWidget()

    def _create_bottom_panel(self) -> QWidget:
        """Create the bottom panel with stats and progress."""
        widget = QWidget()
        layout = QHBoxLayout(widget)
        layout.setContentsMargins(5, 5, 5, 5)

        # Parameter controls
        self.param_controls = ParameterControls(self.config)
        self.param_controls.parameters_changed.connect(self._on_params_changed)
        layout.addWidget(self.param_controls, stretch=1)

        # Transfer progress
        self.transfer_progress = TransferProgressWidget()
        layout.addWidget(self.transfer_progress, stretch=1)

        # Stats display
        self.stats_display = StatsDisplay()
        layout.addWidget(self.stats_display, stretch=0)

        return widget

    def _populate_devices(self, combo, devices):
        """Populate a combo box with audio devices."""
        combo.clear()
        combo.addItem("Default", -1)
        for dev in devices:
            combo.addItem(f"[{dev.index}] {dev.name}", dev.index)

    def _setup_menu(self):
        """Setup the menu bar."""
        menubar = self.menuBar()

        # File menu
        file_menu = menubar.addMenu("&File")

        send_action = file_menu.addAction("&Send File...")
        send_action.setShortcut("Ctrl+S")
        send_action.triggered.connect(self._on_send_file)

        receive_action = file_menu.addAction("&Receive File...")
        receive_action.setShortcut("Ctrl+R")
        receive_action.triggered.connect(self._on_receive_file)

        file_menu.addSeparator()

        exit_action = file_menu.addAction("E&xit")
        exit_action.setShortcut("Ctrl+Q")
        exit_action.triggered.connect(self.close)

        # View menu
        view_menu = menubar.addMenu("&View")
        reset_waterfalls = view_menu.addAction("Reset &Waterfalls")
        reset_waterfalls.triggered.connect(self._reset_waterfalls)

        # Help menu
        help_menu = menubar.addMenu("&Help")
        about_action = help_menu.addAction("&About")
        about_action.triggered.connect(self._on_about)

    def _setup_connections(self):
        """Setup signal connections."""
        pass

    def _on_start_stop(self):
        """Handle start button click."""
        if self.worker and self.worker.isRunning():
            return  # already running

        mode_map = {
            "Full-Duplex": "full-duplex",
            "TX Only": "tx",
            "RX Only": "rx",
        }
        mode = mode_map.get(self.mode_combo.currentText(), "full-duplex")

        # Get device indices
        input_idx = self.input_device_combo.currentData()
        output_idx = self.output_device_combo.currentData()

        if input_idx != -1:
            self.config.audio.device_input_index = input_idx
        if output_idx != -1:
            self.config.audio.device_output_index = output_idx

        # Create and start worker
        self.worker = AudioWorkerThread(self.config, mode)
        self.worker.setup()

        # Connect signals
        self.worker.signal_waterfall_tx.connect(self._update_tx_waterfall)
        self.worker.signal_waterfall_rx.connect(self._update_rx_waterfall)
        self.worker.signal_stats_update.connect(self._update_stats_from_worker)
        self.worker.signal_progress_update.connect(self._update_progress_from_worker)
        self.worker.signal_status_message.connect(self._update_status)
        self.worker.signal_transfer_complete.connect(self._on_transfer_complete)

        self.worker.start()

        self.btn_start.setEnabled(False)
        self.btn_stop.setEnabled(True)

    def _on_stop(self):
        """Handle stop button click."""
        if self.worker:
            if self.worker.protocol:
                self.worker.protocol.cancel()
            self.worker.stop()
            self.worker.wait(2000)  # wait up to 2 seconds

        self.btn_start.setEnabled(True)
        self.btn_stop.setEnabled(False)
        self.status_label.setText("Stopped")

    def _on_send_file(self):
        """Handle send file button click."""
        filepath, _ = QFileDialog.getOpenFileName(
            self, "Select File to Send", "", "All Files (*)"
        )
        if filepath:
            if self.worker:
                self.worker.send_file(filepath)
            else:
                QMessageBox.warning(self, "Not Running", "Please start the audio stream first.")

    def _on_receive_file(self):
        """Handle receive file button click."""
        filepath, _ = QFileDialog.getSaveFileName(
            self, "Save Received File As", "", "All Files (*)"
        )
        if filepath:
            if self.worker:
                self.worker.start_receive(filepath)
            else:
                QMessageBox.warning(self, "Not Running", "Please start the audio stream first.")

    def _on_params_changed(self, params: dict):
        """Handle parameter changes from the controls."""
        # Update config
        self.config.modulation.baud_rate = params.get("baud_rate", self.config.modulation.baud_rate)
        self.config.modulation.m_fsk = params.get("m_fsk", self.config.modulation.m_fsk)
        self.config.modulation.freq_min = params.get("freq_min", self.config.modulation.freq_min)
        self.config.modulation.freq_max = params.get("freq_max", self.config.modulation.freq_max)
        self.config.fec.nsym = params.get("nsym", self.config.fec.nsym)

        # Note: parameter changes take effect on next frame/transmission
        self.status_label.setText(f"Params updated: {params.get('baud_rate')} baud, {params.get('m_fsk')}-FSK")

    def _update_tx_waterfall(self, samples: np.ndarray):
        """Update TX waterfall display."""
        self.tx_waterfall.update_fft(samples)

    def _update_rx_waterfall(self, samples: np.ndarray):
        """Update RX waterfall display."""
        self.rx_waterfall.update_fft(samples)

    def _update_stats_from_worker(self, stats: dict):
        """Update stats display from worker."""
        self.stats_display.update_stats(stats)

    def _update_progress_from_worker(self, progress: dict):
        """Update progress display from worker."""
        self.transfer_progress.update_progress(progress)

    def _update_status(self, message: str):
        """Update status bar."""
        self.status_label.setText(message)

    def _on_transfer_complete(self, success: bool, message: str):
        """Handle transfer completion."""
        self.status_label.setText(message)
        if success:
            QMessageBox.information(self, "Transfer Complete", message)
        else:
            QMessageBox.warning(self, "Transfer Failed", message)

    def _update_stats(self):
        """Periodic stats update."""
        if self.worker and self.worker.isRunning():
            self.worker.update_stats()

    def _reset_waterfalls(self):
        """Reset waterfall displays."""
        self.tx_waterfall.reset()
        self.rx_waterfall.reset()

    def _on_about(self):
        """Show about dialog."""
        QMessageBox.about(
            self,
            "About Speed of Sound WiFi",
            "<h3>Speed of Sound WiFi v0.1.0</h3>"
            "<p>Acoustically coupled data transmission system.</p>"
            "<p>Test the maximum baud rate for audio-based data links.</p>"
            "<p>Uses M-FSK modulation with Reed-Solomon FEC.</p>",
        )

    def closeEvent(self, event):
        """Handle window close."""
        if self.worker and self.worker.isRunning():
            self.worker.stop()
            self.worker.wait(2000)
        event.accept()
