"""UI controls for the audio data transmission application.

Provides sliders, spinners, and selectors for all configurable parameters.
"""

import logging
from typing import Callable, Optional

from PyQt6.QtCore import Qt, pyqtSignal
from PyQt6.QtWidgets import (
    QComboBox,
    QDoubleSpinBox,
    QFormLayout,
    QGroupBox,
    QHBoxLayout,
    QLabel,
    QLayout,
    QProgressBar,
    QSpinBox,
    QVBoxLayout,
    QWidget,
)

from src.config import Config

logger = logging.getLogger(__name__)


class StatsDisplay(QWidget):
    """Real-time statistics display."""

    def __init__(self, parent: Optional[QWidget] = None):
        super().__init__(parent)
        self._setup_ui()

    def _setup_ui(self):
        layout = QVBoxLayout(self)
        layout.setSpacing(4)

        self.throughput_label = QLabel("Throughput: 0 bps")
        self.error_rate_label = QLabel("Error Rate: 0.00%")
        self.snr_label = QLabel("SNR: 0.0 dB")
        self.frames_label = QLabel("Frames: 0 sent / 0 received")
        self.retransmit_label = QLabel("Retransmissions: 0")
        self.fec_label = QLabel("FEC Corrections: 0")

        for label in [
            self.throughput_label,
            self.error_rate_label,
            self.snr_label,
            self.frames_label,
            self.retransmit_label,
            self.fec_label,
        ]:
            label.setStyleSheet("font-family: monospace; font-size: 12px;")
            layout.addWidget(label)

        layout.addStretch()

    def update_stats(self, stats: dict):
        """Update the stats display.

        Args:
            stats: Dictionary with stat keys.
        """
        if "throughput_bps" in stats:
            bps = stats["throughput_bps"]
            if bps >= 1000000:
                self.throughput_label.setText(f"Throughput: {bps / 1000000:.2f} Mbps")
            elif bps >= 1000:
                self.throughput_label.setText(f"Throughput: {bps / 1000:.2f} Kbps")
            else:
                self.throughput_label.setText(f"Throughput: {bps:.0f} bps")

        if "error_rate" in stats:
            self.error_rate_label.setText(f"Error Rate: {stats['error_rate']:.4f}%")

        if "snr_db" in stats:
            self.snr_label.setText(f"SNR: {stats['snr_db']:.1f} dB")

        if "frames_sent" in stats:
            self.frames_label.setText(
                f"Frames: {stats['frames_sent']} sent / {stats.get('frames_received', 0)} received"
            )

        if "retransmissions" in stats:
            self.retransmit_label.setText(f"Retransmissions: {stats['retransmissions']}")

        if "fec_corrections" in stats:
            self.fec_label.setText(f"FEC Corrections: {stats['fec_corrections']}")


class TransferProgressWidget(QWidget):
    """File transfer progress display."""

    def __init__(self, parent: Optional[QWidget] = None):
        super().__init__(parent)
        self._setup_ui()

    def _setup_ui(self):
        layout = QVBoxLayout(self)

        self.state_label = QLabel("State: Idle")
        self.state_label.setStyleSheet("font-weight: bold; font-size: 13px;")
        layout.addWidget(self.state_label)

        self.filename_label = QLabel("File: -")
        layout.addWidget(self.filename_label)

        self.size_label = QLabel("Size: -")
        layout.addWidget(self.size_label)

        self.progress_bar = QProgressBar()
        self.progress_bar.setRange(0, 100)
        self.progress_bar.setValue(0)
        self.progress_bar.setTextVisible(True)
        layout.addWidget(self.progress_bar)

        self.speed_label = QLabel("Speed: -")
        layout.addWidget(self.speed_label)

        self.time_label = QLabel("Time: -")
        layout.addWidget(self.time_label)

    def update_progress(self, progress_data: dict):
        """Update the progress display.

        Args:
            progress_data: Dictionary with progress information.
        """
        if "state" in progress_data:
            self.state_label.setText(f"State: {progress_data['state']}")

        if "filename" in progress_data:
            self.filename_label.setText(f"File: {progress_data['filename']}")

        if "file_size" in progress_data:
            size = progress_data["file_size"]
            if size >= 1024 * 1024:
                self.size_label.setText(f"Size: {size / (1024 * 1024):.2f} MB")
            elif size >= 1024:
                self.size_label.setText(f"Size: {size / 1024:.2f} KB")
            else:
                self.size_label.setText(f"Size: {size} B")

        if "progress_percent" in progress_data:
            self.progress_bar.setValue(int(progress_data["progress_percent"]))

        if "throughput_string" in progress_data:
            self.speed_label.setText(f"Speed: {progress_data['throughput_string']}")

        if "elapsed_time" in progress_data:
            elapsed = progress_data["elapsed_time"]
            self.time_label.setText(f"Time: {elapsed:.1f}s")


class ParameterControls(QWidget):
    """Controls for modulation and transmission parameters."""

    # Signal emitted when parameters change
    parameters_changed = pyqtSignal(dict)

    def __init__(self, config: Config, parent: Optional[QWidget] = None):
        super().__init__(parent)
        self.config = config
        self._setup_ui()

    def _setup_ui(self):
        main_layout = QVBoxLayout(self)

        # Modulation group
        mod_group = QGroupBox("Modulation")
        mod_layout = QFormLayout()

        self.baud_rate_spin = QSpinBox()
        self.baud_rate_spin.setRange(100, 50000)
        self.baud_rate_spin.setValue(self.config.modulation.baud_rate)
        self.baud_rate_spin.setSingleStep(100)
        self.baud_rate_spin.setSuffix(" baud")
        self.baud_rate_spin.valueChanged.connect(self._on_param_changed)
        mod_layout.addRow("Baud Rate:", self.baud_rate_spin)

        self.m_fsk_combo = QComboBox()
        self.m_fsk_combo.addItems(["2-FSK", "4-FSK", "8-FSK", "16-FSK"])
        m_index = {2: 0, 4: 1, 8: 2, 16: 3}.get(self.config.modulation.m_fsk, 1)
        self.m_fsk_combo.setCurrentIndex(m_index)
        self.m_fsk_combo.currentIndexChanged.connect(self._on_param_changed)
        mod_layout.addRow("M-FSK:", self.m_fsk_combo)

        self.freq_min_spin = QSpinBox()
        self.freq_min_spin.setRange(20, 10000)
        self.freq_min_spin.setValue(self.config.modulation.freq_min)
        self.freq_min_spin.setSingleStep(100)
        self.freq_min_spin.setSuffix(" Hz")
        self.freq_min_spin.valueChanged.connect(self._on_param_changed)
        mod_layout.addRow("Freq Min:", self.freq_min_spin)

        self.freq_max_spin = QSpinBox()
        self.freq_max_spin.setRange(10000, 20000)
        self.freq_max_spin.setValue(self.config.modulation.freq_max)
        self.freq_max_spin.setSingleStep(500)
        self.freq_max_spin.setSuffix(" Hz")
        self.freq_max_spin.valueChanged.connect(self._on_param_changed)
        mod_layout.addRow("Freq Max:", self.freq_max_spin)

        mod_group.setLayout(mod_layout)
        main_layout.addWidget(mod_group)

        # FEC group
        fec_group = QGroupBox("Forward Error Correction")
        fec_layout = QFormLayout()

        self.fec_strength_combo = QComboBox()
        self.fec_strength_combo.addItems(["RS(255,223)", "RS(255,239)", "RS(255,207)", "RS(255,191)"])
        # Map nsym values
        nsym_values = [32, 16, 48, 64]
        current_nsym = self.config.fec.nsym
        if current_nsym in nsym_values:
            self.fec_strength_combo.setCurrentIndex(nsym_values.index(current_nsym))
        self.fec_strength_combo.currentIndexChanged.connect(self._on_param_changed)
        fec_layout.addRow("RS Code:", self.fec_strength_combo)

        fec_group.setLayout(fec_layout)
        main_layout.addWidget(fec_group)

        # Info label
        self.info_label = QLabel(self._get_info_text())
        self.info_label.setStyleSheet("font-family: monospace; color: #888;")
        self.info_label.setWordWrap(True)
        main_layout.addWidget(self.info_label)

        main_layout.addStretch()

    def _on_param_changed(self):
        """Handle parameter change."""
        params = {
            "baud_rate": self.baud_rate_spin.value(),
            "m_fsk": 2 ** (self.m_fsk_combo.currentIndex() + 1),
            "freq_min": self.freq_min_spin.value(),
            "freq_max": self.freq_max_spin.value(),
            "nsym": [32, 16, 48, 64][self.fec_strength_combo.currentIndex()],
        }
        self.info_label.setText(self._get_info_text())
        self.parameters_changed.emit(params)

    def _get_info_text(self) -> str:
        """Generate info text about current configuration."""
        m_fsk = 2 ** (self.m_fsk_combo.currentIndex() + 1)
        baud = self.baud_rate_spin.value()
        bits_per_symbol = m_fsk.bit_length() - 1
        theoretical_bps = baud * bits_per_symbol

        return (
            f"M={m_fsk}, {bits_per_symbol} bit/symbol\n"
            f"Theoretical: {theoretical_bps:,} bps\n"
            f"Freq range: {self.freq_min_spin.value()}-{self.freq_max_spin.value()} Hz"
        )

    def get_params(self) -> dict:
        """Get current parameter values."""
        return {
            "baud_rate": self.baud_rate_spin.value(),
            "m_fsk": 2 ** (self.m_fsk_combo.currentIndex() + 1),
            "freq_min": self.freq_min_spin.value(),
            "freq_max": self.freq_max_spin.value(),
            "nsym": [32, 16, 48, 64][self.fec_strength_combo.currentIndex()],
        }
