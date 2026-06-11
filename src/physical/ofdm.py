"""OFDM modulator and demodulator for audio data transmission.

Uses Hermitian-symmetric OFDM (DMT) for real-valued audio output.
Configurable FFT size, CP length, subcarrier range, and bits per subcarrier.
"""

import logging
from typing import Optional

import numpy as np

from src.config import Config

logger = logging.getLogger(__name__)


def _qpsk_map(bits: np.ndarray) -> np.ndarray:
    """Map pairs of bits to QPSK complex symbols (Gray-coded)."""
    symbols = np.zeros(len(bits) // 2, dtype=np.complex64)
    for i in range(len(symbols)):
        b0 = bits[2 * i]
        b1 = bits[2 * i + 1]
        # Gray code: 00->+1+j, 01->-1+j, 11->-1-j, 10->+1-j
        I = 1.0 if b0 == 0 else -1.0
        Q = 1.0 if b1 == 0 else -1.0
        symbols[i] = complex(I, Q)
    return symbols / np.sqrt(2.0)


def _qpsk_demap(symbols: np.ndarray) -> np.ndarray:
    """Demap QPSK complex symbols back to bits (Gray-coded)."""
    bits = np.zeros(len(symbols) * 2, dtype=np.uint8)
    for i, s in enumerate(symbols):
        bits[2 * i] = 0 if s.real > 0 else 1
        bits[2 * i + 1] = 0 if s.imag > 0 else 1
    return bits


def _bpsk_map(bits: np.ndarray) -> np.ndarray:
    """Map bits to BPSK symbols: 0->+1, 1->-1."""
    return np.where(bits == 0, 1.0, -1.0).astype(np.complex64)


def _bpsk_demap(symbols: np.ndarray) -> np.ndarray:
    """Demap BPSK symbols to bits."""
    return (symbols.real < 0).astype(np.uint8)


class OfdmModulator:
    """OFDM modulator that converts bytes to time-domain audio signal."""

    def __init__(self, config: Config):
        self.config = config
        self.ofdm = config.ofdm
        self.sample_rate = config.audio.sample_rate
        self.fft_size = self.ofdm.fft_size
        self.cp_length = self.ofdm.cp_length
        self.sub_min = self.ofdm.subcarrier_min
        self.sub_max = self.ofdm.subcarrier_max
        self.sub_count = self.ofdm_subcarrier_count
        self.bits_per_sc = self.ofdm.bits_per_subcarrier
        self.preamble_symbols = self.ofdm.preamble_symbols

        # Subcarrier spacing
        self.subcarrier_spacing = self.sample_rate / self.fft_size

        # Preamble: QPSK symbols on all active subcarriers (known sequence)
        rng = np.random.default_rng(seed=42)
        half_bits = self.sub_count * self.preamble_symbols * 2
        preamble_bits = rng.integers(0, 2, size=half_bits, dtype=np.uint8)
        self._preamble_sc_symbols = _qpsk_map(preamble_bits).reshape(
            self.preamble_symbols, self.sub_count
        )

    @property
    def ofdm_subcarrier_count(self) -> int:
        return self.sub_max - self.sub_min + 1

    def _build_ofdm_symbol(self, fd_data: np.ndarray) -> np.ndarray:
        """Build a real-valued OFDM symbol from frequency-domain data.

        Args:
            fd_data: Complex QPSK symbols for active subcarriers (len = sub_count).

        Returns:
            Time-domain samples (real) of length fft_size + cp_length.
        """
        N = self.fft_size
        fd = np.zeros(N, dtype=np.complex64)

        # Place data on active subcarriers
        fd[self.sub_min:self.sub_max + 1] = fd_data

        # Hermitian symmetry for real-valued IFFT output
        # fd[N - k] = conj(fd[k])
        fd[N - self.sub_max:N - self.sub_min + 1] = np.conj(fd_data[::-1])

        # IFFT (ortho-normalized)
        td = np.fft.ifft(fd, norm="ortho")

        # Add cyclic prefix
        td_with_cp = np.concatenate([td[-self.cp_length:], td])

        return td_with_cp.real.astype(np.float32)

    def _extract_ofdm_symbol(self, td: np.ndarray) -> np.ndarray:
        """Extract frequency-domain data from a real-valued OFDM symbol.

        Args:
            td: Time-domain samples of one OFDM symbol (CP + FFT), real.

        Returns:
            Complex QPSK symbols on active subcarriers.
        """
        N = self.fft_size
        # Remove CP
        body = td[self.cp_length:self.cp_length + N]
        # FFT
        fd = np.fft.fft(body, norm="ortho")
        # Extract active subcarriers
        return fd[self.sub_min:self.sub_max + 1]

    def modulate_with_preamble(self, data: bytes) -> np.ndarray:
        """Modulate data bytes into an OFDM audio signal with preamble.

        Args:
            data: Bytes to transmit.

        Returns:
            Audio samples (float32) ready for playback.
        """
        # Convert bytes to bits
        bits = np.unpackbits(np.frombuffer(data, dtype=np.uint8))
        # Pad to multiple of bits per symbol
        bits_per_ofdm_sym = self.sub_count * self.bits_per_sc
        remainder = len(bits) % bits_per_ofdm_sym
        if remainder:
            pad_len = bits_per_ofdm_sym - remainder
            bits = np.pad(bits, (0, pad_len), mode="constant")

        num_data_syms = len(bits) // bits_per_ofdm_sym
        # Map bits to QPSK symbols per OFDM symbol
        fd_symbols = bits.reshape(num_data_syms, bits_per_ofdm_sym)
        data_sc_symbols = np.array([
            _qpsk_map(fd_symbols[i]) for i in range(num_data_syms)
        ])

        # Build preamble (known symbols)
        preamble_audio = []
        for i in range(self.preamble_symbols):
            preamble_audio.append(self._build_ofdm_symbol(self._preamble_sc_symbols[i]))
        preamble_audio = np.concatenate(preamble_audio)

        # Build data symbols
        if num_data_syms > 0:
            data_audio = np.concatenate([
                self._build_ofdm_symbol(data_sc_symbols[i])
                for i in range(num_data_syms)
            ])
        else:
            data_audio = np.array([], dtype=np.float32)

        # Apply raised-cosine onset to preamble for smooth start
        onset_len = min(self.cp_length, len(preamble_audio))
        if onset_len > 0:
            ramp = 0.5 * (1 - np.cos(np.pi * np.arange(onset_len) / onset_len))
            preamble_audio[:onset_len] *= ramp

        return np.concatenate([preamble_audio, data_audio])


class OfdmDemodulator:
    """OFDM demodulator that converts audio samples back to bytes."""

    def __init__(self, config: Config):
        self.config = config
        self.ofdm = config.ofdm
        self.sample_rate = config.audio.sample_rate
        self.fft_size = self.ofdm.fft_size
        self.cp_length = self.ofdm.cp_length
        self.sub_min = self.ofdm.subcarrier_min
        self.sub_max = self.ofdm.subcarrier_max
        self.sub_count = self.ofdm_subcarrier_count
        self.bits_per_sc = self.ofdm.bits_per_subcarrier
        self.preamble_symbols = self.ofdm.preamble_symbols
        self.sym_samples = self.fft_size + self.cp_length

        # Preamble in frequency domain (same as modulator)
        rng = np.random.default_rng(seed=42)
        half_bits = self.sub_count * self.preamble_symbols * 2
        preamble_bits = rng.integers(0, 2, size=half_bits, dtype=np.uint8)
        self._preamble_sc_symbols = _qpsk_map(preamble_bits).reshape(
            self.preamble_symbols, self.sub_count
        )

        # Preamble time-domain signal (for cross-correlation timing)
        self._preamble_audio = self._generate_preamble_audio()

        # Estimated channel (frequency domain)
        self._channel_est: Optional[np.ndarray] = None

    def _generate_preamble_audio(self) -> np.ndarray:
        """Generate the full time-domain preamble signal (same as TX)."""
        N = self.fft_size
        cp = self.cp_length
        audio = []
        for i in range(self.preamble_symbols):
            fd = np.zeros(N, dtype=np.complex64)
            fd[self.sub_min:self.sub_max + 1] = self._preamble_sc_symbols[i]
            fd[N - self.sub_max:N - self.sub_min + 1] = np.conj(self._preamble_sc_symbols[i][::-1])
            td = np.fft.ifft(fd, norm="ortho")
            td_cp = np.concatenate([td[-cp:], td])
            audio.append(td_cp.real.astype(np.float32))
        result = np.concatenate(audio)
        onset_len = min(cp, len(result))
        if onset_len > 0:
            ramp = 0.5 * (1 - np.cos(np.pi * np.arange(onset_len) / onset_len))
            result[:onset_len] *= ramp
        return result

    @property
    def ofdm_subcarrier_count(self) -> int:
        return self.sub_max - self.sub_min + 1

    def _estimate_channel(self, symbols: list) -> np.ndarray:
        """Estimate channel from preamble symbols (zero-forcing).

        Args:
            symbols: List of preamble OFDM symbols (frequency domain, complex).

        Returns:
            Channel estimate per active subcarrier.
        """
        H = np.zeros(self.sub_count, dtype=np.complex64)
        for i in range(min(len(symbols), self.preamble_symbols)):
            H += symbols[i] / (self._preamble_sc_symbols[i] + 1e-10)
        H /= min(len(symbols), self.preamble_symbols)
        return H

    def _equalize(self, fd_data: np.ndarray, H: np.ndarray) -> np.ndarray:
        """One-tap zero-forcing equalization."""
        return fd_data / (H + 1e-10)

    def process_samples(self, samples: np.ndarray) -> Optional[np.ndarray]:
        """Process incoming audio samples and extract symbols.

        Uses cross-correlation with stored preamble for robust timing.

        Args:
            samples: Audio samples (float32).

        Returns:
            Array of demodulated bits, or None if no valid data.
        """
        N = self.fft_size
        cp = self.cp_length
        sym_len = self.sym_samples
        preamble_len = len(self._preamble_audio)
        min_required = preamble_len + sym_len

        if len(samples) < min_required:
            return None

        # 1. Cross-correlate with stored preamble for timing
        xcorr = np.convolve(samples, self._preamble_audio[::-1], mode="valid")
        peak_idx = int(np.argmax(np.abs(xcorr)))
        peak_val = np.abs(xcorr[peak_idx])

        # Normalize: compute expected energy
        preamble_energy = np.dot(self._preamble_audio, self._preamble_audio)
        if preamble_energy < 1e-10:
            return None

        # Reject weak correlations
        if peak_val < 0.3 * preamble_energy:
            return None

        # The peak of the valid cross-correlation is at the start of the preamble
        start_idx = peak_idx

        # 2. Extract preamble symbols for channel estimation
        preamble_fd = []
        for i in range(self.preamble_symbols):
            offset = start_idx + i * sym_len
            if offset + sym_len > len(samples):
                return None
            sym = samples[offset:offset + sym_len]
            fd = np.fft.fft(sym[cp:cp + N], norm="ortho")
            preamble_fd.append(fd[self.sub_min:self.sub_max + 1])

        # 3. Channel estimation (always re-estimate for new burst)
        self._channel_est = self._estimate_channel(preamble_fd)
        H = self._channel_est

        # 4. Extract data symbols
        data_offset = start_idx + preamble_len
        num_data = (len(samples) - data_offset) // sym_len
        if num_data == 0:
            return None

        all_bits = []
        for i in range(num_data):
            offset = data_offset + i * sym_len
            if offset + sym_len > len(samples):
                break
            sym = samples[offset:offset + sym_len]
            fd = np.fft.fft(sym[cp:cp + N], norm="ortho")
            fd_active = fd[self.sub_min:self.sub_max + 1]
            fd_eq = self._equalize(fd_active, H)
            bits = _qpsk_demap(fd_eq)
            all_bits.append(bits)

        if not all_bits:
            return None

        return np.concatenate(all_bits)

    def symbols_to_bytes(self, symbols: np.ndarray, original_byte_count: int) -> bytes:
        """Convert demodulated bits back to bytes.

        Args:
            symbols: Array of bits (uint8, 0 or 1).
            original_byte_count: Expected number of output bytes.

        Returns:
            Decoded bytes.
        """
        total_bits = original_byte_count * 8
        if len(symbols) > total_bits:
            symbols = symbols[:total_bits]
        # Pad to multiple of 8
        pad = (8 - len(symbols) % 8) % 8
        if pad:
            symbols = np.pad(symbols, (0, pad), mode="constant")
        byte_array = np.packbits(symbols)
        return byte_array.tobytes()

    def reset(self):
        """Reset demodulator state (for new transmission)."""
        self._channel_est = None
