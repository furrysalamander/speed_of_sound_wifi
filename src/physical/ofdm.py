"""OFDM modulator and demodulator for audio data transmission.

Uses Hermitian-symmetric OFDM (DMT) for real-valued audio output.
Configurable FFT size, CP length, subcarrier range, and bits per subcarrier.
Supports QPSK (2 bits/subcarrier) with optional pilot subcarriers.
Data bits are XOR-scrambled with a PRNG (seed=12345) before modulation.
"""

import logging
from typing import Optional

import numpy as np

from src.config import Config

logger = logging.getLogger(__name__)


def _qpsk_map(bits: np.ndarray) -> np.ndarray:
    """Map pairs of bits to QPSK complex symbols (Gray-coded)."""
    n = len(bits) // 2
    sym = np.zeros(n, dtype=np.complex64)
    for i in range(n):
        b0 = bits[2 * i]
        b1 = bits[2 * i + 1]
        I = 1.0 if b0 == 0 else -1.0
        Q = 1.0 if b1 == 0 else -1.0
        sym[i] = complex(I, Q)
    return sym / np.sqrt(2.0)


def _qpsk_demap(symbols: np.ndarray) -> np.ndarray:
    """Demap QPSK complex symbols back to bits (Gray-coded)."""
    n = len(symbols)
    bits = np.zeros(n * 2, dtype=np.uint8)
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
        self.subcarrier_spacing = self.sample_rate / self.fft_size

        # Pilot subcarriers
        self.pilot_indices = sorted(set(self.ofdm.pilot_subcarriers))
        self.num_pilots = len(self.pilot_indices)
        self.data_bits_per_sym = (self.sub_count - self.num_pilots) * self.bits_per_sc

        # Known pilot symbol value
        if self.bits_per_sc == 1:
            self._pilot_symbol = complex(1.0, 0.0)
        else:
            self._pilot_symbol = (1.0 + 1.0j) / np.sqrt(2.0)

        # Non-pilot subcarrier indices (for placing data)
        self._data_sc_mask = np.ones(self.sub_count, dtype=bool)
        self._data_sc_mask[self.pilot_indices] = False
        self._data_sc_indices = np.where(self._data_sc_mask)[0]
        self._pilot_sc_indices = np.where(~self._data_sc_mask)[0]

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
        """Build a real-valued OFDM symbol from frequency-domain data."""
        N = self.fft_size
        fd = np.zeros(N, dtype=np.complex64)

        # Place data on active subcarriers
        fd[self.sub_min:self.sub_max + 1] = fd_data

        # Hermitian symmetry for real-valued IFFT output
        fd[N - self.sub_max:N - self.sub_min + 1] = np.conj(fd_data[::-1])

        # IFFT
        td = np.fft.ifft(fd, norm="ortho")

        # Add cyclic prefix
        td_with_cp = np.concatenate([td[-self.cp_length:], td])

        return td_with_cp.real.astype(np.float32)

    def _extract_ofdm_symbol(self, td: np.ndarray) -> np.ndarray:
        """Extract frequency-domain data from a real-valued OFDM symbol."""
        N = self.fft_size
        body = td[self.cp_length:self.cp_length + N]
        fd = np.fft.fft(body, norm="ortho")
        return fd[self.sub_min:self.sub_max + 1]

    def modulate_with_preamble(self, data: bytes) -> np.ndarray:
        """Modulate data bytes into an OFDM audio signal with preamble.

        Uses BPSK or QPSK per config. Pilot subcarriers carry known symbols
        (overriding data) for receiver-side phase tracking.

        Data is XOR-scrambled with a known PRNG pattern before modulation
        to avoid problematic bit patterns (e.g. alternating 1s/0s in sync).

        Args:
            data: Bytes to transmit.

        Returns:
            Audio samples (float32) ready for playback.
        """
        # XOR-scramble with known pattern
        rng = np.random.default_rng(seed=12345)
        mask = bytes(rng.integers(0, 256, size=len(data), dtype=np.uint8).tolist())
        data = bytes(a ^ b for a, b in zip(data, mask))

        # Convert bytes to bits
        bits = np.unpackbits(np.frombuffer(data, dtype=np.uint8))

        # Pad to multiple of data_bits_per_sym
        remainder = len(bits) % self.data_bits_per_sym
        if remainder:
            bits = np.pad(bits, (0, self.data_bits_per_sym - remainder), mode="constant")

        num_data_syms = len(bits) // self.data_bits_per_sym

        # Map bits to subcarrier symbols (data subcarriers only)
        fd_symbols = bits.reshape(num_data_syms, self.data_bits_per_sym)
        _map_fn = _bpsk_map if self.bits_per_sc == 1 else _qpsk_map
        data_symbols = np.array([_map_fn(fd_symbols[i]) for i in range(num_data_syms)])

        # Insert pilot symbols at pilot subcarrier positions
        full_symbols = np.zeros((num_data_syms, self.sub_count), dtype=np.complex64)
        full_symbols[:, self._data_sc_indices] = data_symbols
        full_symbols[:, self._pilot_sc_indices] = self._pilot_symbol

        # Build preamble
        preamble_audio = np.concatenate([
            self._build_ofdm_symbol(self._preamble_sc_symbols[i])
            for i in range(self.preamble_symbols)
        ])

        # Build data symbols
        if num_data_syms > 0:
            data_audio = np.concatenate([
                self._build_ofdm_symbol(full_symbols[i])
                for i in range(num_data_syms)
            ])
        else:
            data_audio = np.array([], dtype=np.float32)

        # Raised-cosine onset
        onset_len = min(self.cp_length, len(preamble_audio))
        if onset_len > 0:
            ramp = 0.5 * (1 - np.cos(np.pi * np.arange(onset_len) / onset_len))
            preamble_audio[:onset_len] *= ramp

        result = np.concatenate([preamble_audio, data_audio])

        # Normalize
        max_val = np.max(np.abs(result))
        if max_val > 0:
            result = result * (self.config.modulation.output_amplitude / max_val)

        return result


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

        # Pilot subcarriers
        self.pilot_indices = sorted(set(self.ofdm.pilot_subcarriers))
        self.num_pilots = len(self.pilot_indices)
        self.data_bits_per_sym = (self.sub_count - self.num_pilots) * self.bits_per_sc

        # Known pilot symbol value
        if self.bits_per_sc == 1:
            self._pilot_symbol = complex(1.0, 0.0)
        else:
            self._pilot_symbol = (1.0 + 1.0j) / np.sqrt(2.0)

        # Non-pilot bit mask (for removing pilot bits from output)
        pilot_bit_indices = []
        for p in self.pilot_indices:
            start = p * self.bits_per_sc
            pilot_bit_indices.extend(range(start, start + self.bits_per_sc))
        self._data_bit_mask = np.ones(self.sub_count * self.bits_per_sc, dtype=bool)
        self._data_bit_mask[pilot_bit_indices] = False

        # Preamble in frequency domain (same as modulator)
        rng = np.random.default_rng(seed=42)
        half_bits = self.sub_count * self.preamble_symbols * 2
        preamble_bits = rng.integers(0, 2, size=half_bits, dtype=np.uint8)
        self._preamble_sc_symbols = _qpsk_map(preamble_bits).reshape(
            self.preamble_symbols, self.sub_count
        )

        # Preamble time-domain signal (for cross-correlation timing)
        self._preamble_audio = self._generate_preamble_audio()

        # Subcarrier indices (active range) for phase slope computation
        self._k_all = np.arange(self.sub_min, self.sub_max + 1, dtype=np.float32)
        self._k_ref = np.mean(self._k_all)
        self._pilot_k = self._k_all[list(self.pilot_indices)]

        # Estimated channel (frequency domain)
        self._channel_est: Optional[np.ndarray] = None

        # Decision-directed phase tracking state
        self._dd_common = 0.0  # absolute phase correction (radians)
        self._dd_slope = 0.0  # subcarrier-dependent phase slope (rad/index)
        self._cfo_freq = 0.0  # CFO frequency (radians per symbol)
        self._dd_alpha = 0.3  # phase tracking gain

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
        """Estimate channel from preamble symbols (zero-forcing)."""
        H = np.zeros(self.sub_count, dtype=np.complex64)
        n = min(len(symbols), self.preamble_symbols)
        for i in range(n):
            H += symbols[i] / (self._preamble_sc_symbols[i] + 1e-10)
        H /= n
        return H

    def _equalize(self, fd_data: np.ndarray, H: np.ndarray) -> np.ndarray:
        """One-tap zero-forcing equalization."""
        return fd_data / (H + 1e-10)

    def process_samples(self, samples: np.ndarray) -> Optional[np.ndarray]:
        """Process incoming audio samples and extract symbols.

        Uses cross-correlation with stored preamble for robust timing.
        Applies decision-directed phase tracking per symbol.

        Args:
            samples: Audio samples (float32).

        Returns:
            Array of demodulated bits (data subcarriers only, pilot bits removed),
            or None if no valid data.
        """
        N = self.fft_size
        cp = self.cp_length
        sym_len = self.sym_samples
        preamble_len = len(self._preamble_audio)
        min_required = preamble_len + sym_len

        if len(samples) < min_required:
            logger.debug("OFDM: too few samples (%d < %d)", len(samples), min_required)
            return None

        # 1. Energy-based coarse detection
        window = min(preamble_len, len(samples))
        if window < 1:
            return None
        energy = np.convolve(samples ** 2, np.ones(window) / window, mode="same")
        energy_thresh = np.max(energy) * 0.1
        above = energy > energy_thresh
        if not np.any(above):
            logger.debug("OFDM: no energy above threshold")
            return None
        coarse_start = max(0, np.argmax(above) - window // 2)
        search_end = min(len(samples), coarse_start + 2 * preamble_len + sym_len)

        if search_end - coarse_start < preamble_len:
            logger.debug("OFDM: signal too short after coarse start")
            return None

        search_region = samples[coarse_start:search_end]

        # 2. Cross-correlation timing
        xcorr = np.convolve(search_region, self._preamble_audio[::-1], mode="valid")
        peak_idx = int(np.argmax(np.abs(xcorr)))
        peak_val = np.abs(xcorr[peak_idx])

        preamble_energy = np.dot(self._preamble_audio, self._preamble_audio)
        if preamble_energy < 1e-10:
            return None

        corr_window = search_region[peak_idx:peak_idx + preamble_len]
        signal_energy = np.dot(corr_window, corr_window)
        norm_peak = peak_val / (np.sqrt(preamble_energy * signal_energy) + 1e-10)

        logger.debug("OFDM: xcorr peak=%f norm=%f", peak_val, norm_peak)

        if norm_peak < 0.10:
            logger.debug("OFDM: weak correlation (norm=%f < 0.10)", norm_peak)
            return None

        start_idx = coarse_start + peak_idx
        self._last_preamble_start = coarse_start + peak_idx

        # 3. Extract preamble symbols
        preamble_fd = []
        for i in range(self.preamble_symbols):
            offset = start_idx + i * sym_len
            if offset + sym_len > len(samples):
                return None
            sym = samples[offset:offset + sym_len]
            fd = np.fft.fft(sym[cp:cp + N], norm="ortho")
            preamble_fd.append(fd[self.sub_min:self.sub_max + 1])

        # 4. Channel estimation
        self._channel_est = self._estimate_channel(preamble_fd)
        H = self._channel_est

        if np.mean(np.abs(H)) < 0.01:
            logger.debug("OFDM: channel too weak (H=%.4f < 0.01)", np.mean(np.abs(H)))
            return None

        # 5. Estimate CFO from preamble symbols to initialize DD tracking.
        #    We measure the per-symbol phase rotation (CFO drift) and project
        #    it to the first data symbol. The static channel phase in H is NOT
        #    included — we only track the residual DRIFT over symbols.
        preamble_phases = []
        for i in range(self.preamble_symbols):
            pd = preamble_fd[i] / (self._preamble_sc_symbols[i] + 1e-10)
            preamble_phases.append(np.mean(np.angle(pd)))

        if self.preamble_symbols >= 2:
            # Extract per-symbol drift from consecutive preamble symbol pairs
            per_sym_drift = np.mean([
                np.arctan2(np.sin(preamble_phases[i+1] - preamble_phases[i]),
                           np.cos(preamble_phases[i+1] - preamble_phases[i]))
                for i in range(self.preamble_symbols - 1)
            ])
            # Store CFO frequency for second-order phase tracking.
            # Clamp to reasonable range — unreliable estimates from weak
            # signals (< 0.10 H) can produce wild CFO values.
            self._cfo_freq = np.clip(per_sym_drift, -0.05, 0.05)
            # Channel estimate H includes the CFO phase at the preamble center
            # (~symbol 1.5 for 4 preamble symbols). The first data symbol is
            # at index 4. So the residual phase at first data symbol (after
            # equalization by H) is (4 - 1.5) * per_sym_drift = 2.5 * per_sym_drift.
            self._dd_common = self._cfo_freq * (self.preamble_symbols - 1.5)
            self._dd_common = np.arctan2(np.sin(self._dd_common), np.cos(self._dd_common))
            self._dd_slope = 0.0
            logger.debug("OFDM: initial dd_common=%.4f cfo_freq=%.4f rad/sym",
                         self._dd_common, self._cfo_freq)
        else:
            self._dd_common = 0.0
            self._cfo_freq = 0.0
            self._dd_slope = 0.0

        # 6. Extract all data symbols
        data_offset = start_idx + preamble_len
        num_data = (len(samples) - data_offset) // sym_len
        if num_data == 0:
            return None

        all_bits = []
        _demap_fn = _bpsk_demap if self.bits_per_sc == 1 else _qpsk_demap
        _map_fn = _bpsk_map if self.bits_per_sc == 1 else _qpsk_map

        for i in range(num_data):
            offset = data_offset + i * sym_len
            if offset + sym_len > len(samples):
                break
            sym = samples[offset:offset + sym_len]
            fd = np.fft.fft(sym[cp:cp + N], norm="ortho")
            fd_active = fd[self.sub_min:self.sub_max + 1]

            # Apply phase correction from DD tracking
            phase_correction = self._dd_common + self._dd_slope * (self._k_all - self._k_ref)
            fd_corrected = fd_active * np.exp(-1j * phase_correction)

            # Equalize and demap
            fd_eq = self._equalize(fd_corrected, H)
            all_sc_bits = _demap_fn(fd_eq)

            # Remove pilot bits -> data bits only
            data_bits = all_sc_bits[self._data_bit_mask]
            all_bits.append(data_bits)

            # Phase tracking: weighted LS fit over all subcarriers
            # Pilots get boosted weight for higher confidence
            X_hat = _map_fn(all_sc_bits)
            if self.pilot_indices:
                for p in self.pilot_indices:
                    X_hat[p] = self._pilot_symbol

            expected = H * X_hat
            phase_err = np.angle(fd_corrected / (expected + 1e-10))
            weights = np.abs(fd_eq)
            if self.pilot_indices:
                for p in self.pilot_indices:
                    weights[p] = max(weights[p], 2.0)

            A_mat = np.column_stack([np.ones_like(self._k_all), self._k_all - self._k_ref])
            W_mat = np.diag(weights)
            try:
                coeffs, *_ = np.linalg.lstsq(W_mat @ A_mat, W_mat @ phase_err, rcond=None)
                new_common = float(coeffs[0])
                new_slope = float(coeffs[1])

                # Second-order PLL with frequency integrator + leak
                beta = 0.08
                leak = 0.999
                self._cfo_freq = leak * self._cfo_freq + beta * new_common
                self._cfo_freq = np.clip(self._cfo_freq, -0.05, 0.05)
                self._dd_common += self._cfo_freq + self._dd_alpha * new_common
                self._dd_common = np.arctan2(np.sin(self._dd_common),
                                             np.cos(self._dd_common))
                self._dd_slope = np.clip(
                    (1 - self._dd_alpha) * self._dd_slope + self._dd_alpha * new_slope,
                    -0.02, 0.02
                )
            except np.linalg.LinAlgError:
                pass

        if not all_bits:
            return None

        logger.debug("OFDM: decoded %d symbols, %d bits per data sym",
                     len(all_bits), self.data_bits_per_sym)
        return np.concatenate(all_bits)

    def symbols_to_bytes(self, symbols: np.ndarray, original_byte_count: int) -> bytes:
        """Convert demodulated bits back to bytes.

        Applies descrambling (XOR with known PRNG) to undo the TX-side
        scrambler that prevents problematic bit patterns.

        Args:
            symbols: Array of bits (uint8, 0 or 1) — data subcarrier bits only.
            original_byte_count: Expected number of output bytes.

        Returns:
            Decoded bytes.
        """
        total_bits = original_byte_count * 8
        if len(symbols) > total_bits:
            symbols = symbols[:total_bits]
        pad = (8 - len(symbols) % 8) % 8
        if pad:
            symbols = np.pad(symbols, (0, pad), mode="constant")
        result = np.packbits(symbols).tobytes()
        # Descramble (same PRNG pattern as modulator)
        rng = np.random.default_rng(seed=12345)
        mask = bytes(rng.integers(0, 256, size=len(result), dtype=np.uint8).tolist())
        return bytes(a ^ b for a, b in zip(result, mask))

    def reset(self):
        """Reset demodulator state (for new transmission)."""
        self._channel_est = None
        self._dd_common = 0.0
        self._dd_slope = 0.0
        self._cfo_freq = 0.0
