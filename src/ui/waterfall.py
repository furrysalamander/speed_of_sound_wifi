"""Real-time waterfall/spectrogram display using PyQtGraph.

Provides GPU-accelerated waterfall displays for monitoring TX and RX audio signals
in real-time at 30-60 FPS.
"""

import logging
from typing import Optional

import numpy as np
import pyqtgraph as pg
from PyQt6.QtCore import QTimer, Qt
from PyQt6.QtGui import QColor, QImage
from PyQt6.QtWidgets import QWidget

from src.config import Config

logger = logging.getLogger(__name__)

# Set PyQtGraph style
pg.setConfigOptions(antialias=True, imageAxisOrder="row-major")


class WaterfallPlot(QWidget):
    """Real-time waterfall (spectrogram) display.

    Shows frequency on the Y-axis, time scrolling on the X-axis,
    and amplitude as color intensity.
    """

    def __init__(
        self,
        config: Config,
        title: str = "Spectrum",
        parent: Optional[QWidget] = None,
    ):
        super().__init__(parent)
        self.config = config
        self.title = title

        self.sample_rate = config.audio.sample_rate
        self.fft_size = config.ui.waterfall_fft_size
        self.num_cols = config.ui.waterfall_cols

        # Spectrogram buffer: new columns are added on the right
        self._spectrogram_buffer = np.zeros((self.fft_size // 2 + 1, self.num_cols), dtype=np.float32)

        # Setup UI
        self._setup_ui()

        # Update timer
        self._timer = QTimer(self)
        self._timer.timeout.connect(self._update_display)
        self._timer.start(int(1000 / config.ui.waterfall_update_rate))

        # Latest FFT data
        self._latest_fft: Optional[np.ndarray] = None
        self._lock = False  # Simple re-entrancy guard

    def _setup_ui(self):
        """Setup the PyQtGraph display."""
        layout = pg.GraphicsLayoutWidget(self)
        layout.addLabel(self.title, color='w', size='bold 14px')

        # Main spectrum plot
        self.plot = layout.nextRow()
        self.plot = layout.addPlot()
        self.plot.setLabel("left", "Frequency", "Hz")
        self.plot.setLabel("bottom", "Time", "")
        self.plot.setLimits(
            xMin=0,
            xMax=self.num_cols,
            yMin=0,
            yMax=self.sample_rate // 2,
        )
        self.plot.showGrid(x=True, y=True, alpha=0.3)

        # Waterfall image item
        self._image_item = pg.ImageItem(
            autoscale=False,
            axes="raw",
        )
        # Use viridis colormap
        cmap = pg.colormap.get("viridis")
        self._image_item.setLookupTable(cmap.getLookupTable(256))
        self._image_item.setRect(
            pg.QtCore.QRectF(0, 0, self.num_cols, self.sample_rate // 2)
        )
        self.plot.addItem(self._image_item)

        # Layout
        main_layout = pg.QtWidgets.QVBoxLayout(self)
        main_layout.addWidget(layout)
        main_layout.setContentsMargins(0, 0, 0, 0)

    def update_fft(self, audio_samples: np.ndarray):
        """Update the waterfall with new audio samples.

        Args:
            audio_samples: New audio samples (float32).
        """
        # Take exactly fft_size samples (or pad if shorter)
        samples = audio_samples[: self.fft_size]
        if len(samples) < self.fft_size:
            samples = np.pad(samples, (0, self.fft_size - len(samples)))

        # Compute FFT with Hann window matching the sample length
        window = np.hanning(self.fft_size)
        windowed = samples * window
        fft_result = np.fft.rfft(windowed)
        magnitudes = np.abs(fft_result)

        # Convert to dB scale
        db_values = 20 * np.log10(magnitudes + 1e-10)

        # Clip to reasonable range
        db_values = np.clip(db_values, -100, 0)

        self._latest_fft = db_values

    def _update_display(self):
        """Update the display (called by timer)."""
        if self._lock or self._latest_fft is None:
            return

        self._lock = True
        try:
            # Shift buffer left and add new column
            self._spectrogram_buffer = np.roll(self._spectrogram_buffer, -1, axis=1)
            self._spectrogram_buffer[:, -1] = self._latest_fft

            # Update image (transpose so frequency is Y-axis)
            # The buffer has shape (freq_bins, time_cols)
            # We need to flip Y so high frequencies are at top
            display_data = np.flipud(self._spectrogram_buffer)
            self._image_item.setImage(
                display_data,
                autoscale=False,
                xmin=0,
                xmax=self.num_cols,
                ymin=0,
                ymax=self.sample_rate // 2,
            )

        finally:
            self._lock = False

    def reset(self):
        """Clear the waterfall display."""
        self._spectrogram_buffer = np.zeros_like(self._spectrogram_buffer)
        self._latest_fft = None
        self._update_display()
