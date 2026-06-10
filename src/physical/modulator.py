"""FSK (Frequency Shift Keying) modulator for audio data transmission.

Generates audio tones for M-FSK modulation with configurable number of tones,
frequency range, and baud rate.
"""

import logging
from typing import Optional

import numpy as np
from scipy.signal import windows

from src.config import Config

logger = logging.getLogger(__name__)


class FskModulator:
    """M-FSK modulator that converts bytes to audio tones.

    Each symbol (log2(M) bits) is represented by a different frequency tone.
    The modulator maintains phase continuity between symbols to avoid clicks.
    """

    def __init__(self, config: Config, tone_compensation: Optional[list] = None):
        self.config = config
        self._phase: float = 0.0  # maintain phase continuity
        self._fsk_freqs = config.fsk_frequencies
        self._bits_per_symbol = config.bits_per_symbol
        self._symbol_duration_samples = config.symbol_duration_samples
        # Optional per-tone amplitude compensation to flatten mic frequency response
        self._tone_comp = tone_compensation

    def modulate_bytes(self, data: bytes) -> np.ndarray:
        """Convert bytes to an FSK-modulated audio signal.

        Args:
            data: Raw bytes to modulate.

        Returns:
            numpy array of float32 audio samples.
        """
        symbols = self._bytes_to_symbols(data)
        return self._modulate_symbols(symbols)

    def modulate_symbols(self, symbols: np.ndarray) -> np.ndarray:
        """Convert symbol indices to audio signal.

        Args:
            symbols: Array of symbol indices (0 to M-1).

        Returns:
            numpy array of float32 audio samples.
        """
        return self._modulate_symbols(symbols)

    def _bytes_to_symbols(self, data: bytes) -> np.ndarray:
        """Convert bytes to symbol indices.

        Each symbol carries log2(M) bits.
        """
        bits_per_symbol = self._bits_per_symbol
        total_bits = len(data) * 8
        # Pad with zeros if needed to fill complete symbols
        padded_bits = ((total_bits + bits_per_symbol - 1) // bits_per_symbol) * bits_per_symbol
        padding_bits = padded_bits - total_bits

        # Convert bytes to bit array
        bit_array = np.unpackbits(np.frombuffer(data, dtype=np.uint8))
        bit_array = bit_array[:total_bits]

        # Pad with zeros
        if padding_bits > 0:
            bit_array = np.pad(bit_array, (0, padding_bits), mode="constant")

        # Group bits into symbols
        num_symbols = len(bit_array) // bits_per_symbol
        symbol_bits = bit_array[: num_symbols * bits_per_symbol].reshape(-1, bits_per_symbol)

        # Convert bit groups to symbol indices
        symbols = symbol_bits.dot(1 << np.arange(bits_per_symbol)[::-1]).astype(np.int32)

        return symbols

    def _modulate_symbols(self, symbols: np.ndarray) -> np.ndarray:
        """Generate audio waveform for a sequence of symbols.

        Uses phase-continuous tone generation with windowing to reduce
        spectral leakage at symbol boundaries.
        """
        sample_rate = self.config.audio.sample_rate
        symbol_samples = self._symbol_duration_samples
        num_symbols = len(symbols)

        # Pre-allocate output buffer
        total_samples = num_symbols * symbol_samples
        output = np.zeros(total_samples, dtype=np.float32)

        # Generate tones for each symbol
        for i, symbol_idx in enumerate(symbols):
            start = i * symbol_samples
            end = start + symbol_samples
            freq = self._fsk_freqs[symbol_idx % len(self._fsk_freqs)]

            # Generate time indices for this symbol
            t = np.arange(symbol_samples, dtype=np.float64) / sample_rate

            # Generate tone with phase continuity
            tone = np.sin(2 * np.pi * freq * t + self._phase).astype(np.float32)

            # Apply per-tone amplitude compensation if configured
            if self._tone_comp is not None and symbol_idx < len(self._tone_comp):
                tone = tone * self._tone_comp[symbol_idx]

            # Apply window to reduce spectral leakage
            window = windows.hann(symbol_samples, sym=False).astype(np.float32)
            # Use raised cosine overlap to smooth transitions
            tone = tone * window

            # Update phase for next symbol (phase continuity)
            self._phase = (2 * np.pi * freq * symbol_samples / sample_rate + self._phase) % (
                2 * np.pi
            )

            output[start:end] = tone

        # Normalize to avoid clipping
        max_val = np.max(np.abs(output))
        if max_val > 0:
            output = output * (0.9 / max_val)

        return output

    def modulate_with_preamble(self, data: bytes, preamble_len: int = 64) -> np.ndarray:
        """Modulate data with a sync preamble for receiver synchronization.

        The preamble alternates between the lowest and highest FSK frequencies
        to create a distinctive pattern for frame synchronization.

        Args:
            data: Raw bytes to modulate.
            preamble_len: Number of preamble symbols.

        Returns:
            numpy array with preamble + data.
        """
        # Generate alternating preamble symbols (low/high frequency)
        preamble_symbols = np.zeros(preamble_len, dtype=np.int32)
        preamble_symbols[0::2] = 0  # lowest frequency
        preamble_symbols[1::2] = len(self._fsk_freqs) - 1  # highest frequency

        preamble_audio = self._modulate_symbols(preamble_symbols)
        data_audio = self.modulate_bytes(data)

        return np.concatenate([preamble_audio, data_audio])

    def reset_phase(self):
        """Reset the phase accumulator (call before each frame)."""
        self._phase = 0.0

    def get_symbol_rate(self) -> int:
        """Return the symbol rate (baud rate)."""
        return self.config.modulation.baud_rate

    def get_frequencies(self) -> list:
        """Return the list of FSK frequencies."""
        return list(self._fsk_freqs)
