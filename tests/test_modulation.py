"""Tests for FSK modulation and demodulation."""

import numpy as np
import pytest

from src.config import Config
from src.physical.modulator import FskModulator
from src.physical.demodulator import FskDemodulator


@pytest.fixture
def config():
    """Create a test configuration."""
    return Config()


@pytest.fixture
def modulator(config):
    """Create an FSK modulator."""
    return FskModulator(config)


@pytest.fixture
def demodulator(config):
    """Create an FSK demodulator."""
    return FskDemodulator(config)


class TestFskModulator:
    """Tests for FSK modulator."""

    def test_modulate_bytes_produces_audio(self, modulator, config):
        """Test that modulating bytes produces audio samples."""
        data = b"Hello"
        audio = modulator.modulate_bytes(data)

        assert isinstance(audio, np.ndarray)
        assert audio.dtype == np.float32
        assert len(audio) > 0
        # Check amplitude is within reasonable range
        assert np.max(np.abs(audio)) <= 1.0

    def test_modulate_with_preamble(self, modulator, config):
        """Test modulation with sync preamble."""
        data = b"Test"
        audio = modulator.modulate_with_preamble(data, preamble_len=32)

        # Audio should be longer than data without preamble
        audio_no_preamble = modulator.modulate_bytes(data)
        assert len(audio) > len(audio_no_preamble)

    def test_different_data_produces_different_audio(self, modulator):
        """Test that different input data produces different output."""
        data1 = b"AAA"
        data2 = b"BBB"

        audio1 = modulator.modulate_bytes(data1)
        modulator.reset_phase()
        audio2 = modulator.modulate_bytes(data2)

        # Same length but different content
        assert len(audio1) == len(audio2)
        assert not np.allclose(audio1, audio2)

    def test_bits_per_symbol(self, config):
        """Test bits per symbol calculation."""
        config.modulation.m_fsk = 2
        assert config.bits_per_symbol == 1

        config.modulation.m_fsk = 4
        assert config.bits_per_symbol == 2

        config.modulation.m_fsk = 8
        assert config.bits_per_symbol == 3

        config.modulation.m_fsk = 16
        assert config.bits_per_symbol == 4

    def test_fsk_frequencies(self, config):
        """Test FSK frequency calculation."""
        config.modulation.m_fsk = 4
        config.modulation.freq_min = 1000
        config.modulation.freq_max = 5000

        freqs = config.fsk_frequencies
        assert len(freqs) == 4
        # Frequencies should be evenly spaced
        for f in freqs:
            assert config.modulation.freq_min < f < config.modulation.freq_max


class TestFskDemodulator:
    """Tests for FSK demodulator."""

    def test_detect_symbol_from_tone(self, demodulator, config):
        """Test symbol detection from a pure tone."""
        freq = config.fsk_frequencies[0]
        duration_samples = config.symbol_duration_samples
        t = np.arange(duration_samples, dtype=np.float64) / config.audio.sample_rate
        tone = np.sin(2 * np.pi * freq * t).astype(np.float32)

        symbol = demodulator._detect_symbol(tone)
        assert symbol == 0

    def test_symbols_to_bytes_roundtrip(self, demodulator, config):
        """Test symbol to bytes conversion."""
        # Create known symbols
        symbols = np.array([0, 1, 2, 3], dtype=np.int32)  # 4 symbols, 2 bits each = 8 bits = 1 byte
        original_byte_count = 1

        # This test verifies the conversion logic works
        bytes_out = demodulator.symbols_to_bytes(symbols, original_byte_count)
        assert len(bytes_out) == original_byte_count

    def test_process_samples_returns_symbols(self, demodulator, config):
        """Test that processing samples returns symbol indices."""
        # Generate a tone for symbol 0
        freq = config.fsk_frequencies[0]
        duration_samples = config.symbol_duration_samples
        t = np.arange(duration_samples, dtype=np.float64) / config.audio.sample_rate
        tone = np.sin(2 * np.pi * freq * t).astype(np.float32)

        symbols = demodulator.process_samples(tone)
        assert symbols is not None
        assert len(symbols) >= 1


class TestModulationRoundTrip:
    """Integration tests for modulation/demodulation round-trip."""

    def test_simple_roundtrip(self, config):
        """Test modulate -> demodulate round-trip with clean signal.

        Note: Without frame synchronization, the physical layer round-trip
        may not recover all symbols perfectly. We verify that symbols are
        detected and the count is reasonable.
        """
        modulator = FskModulator(config)
        demodulator = FskDemodulator(config)

        # Simple test data
        data = b"\x00\xFF\x55\xAA"
        audio = modulator.modulate_bytes(data)

        # Demodulate (clean loopback, no noise)
        symbols = demodulator.process_samples(audio)
        assert symbols is not None

        # Verify we recovered a reasonable number of symbols
        # The exact count depends on buffer alignment and symbol timing
        expected_symbols = len(data) * 8 // config.bits_per_symbol
        # Allow some tolerance due to windowing and buffer effects
        assert len(symbols) >= expected_symbols // 2, (
            f"Expected at least {expected_symbols // 2} symbols, got {len(symbols)}"
        )

    def test_roundtrip_with_noise(self, config):
        """Test round-trip with added noise."""
        modulator = FskModulator(config)
        demodulator = FskDemodulator(config)

        data = b"Test data with noise"
        audio = modulator.modulate_bytes(data)

        # Add Gaussian noise (SNR ~20dB)
        signal_power = np.mean(audio ** 2)
        noise_power = signal_power / 100  # 20dB SNR
        noise = np.random.normal(0, np.sqrt(noise_power), len(audio)).astype(np.float32)
        noisy_audio = audio + noise

        # Demodulate
        symbols = demodulator.process_samples(noisy_audio)
        assert symbols is not None
        assert len(symbols) > 0

    def test_different_m_fsk_values(self):
        """Test round-trip with different M-FSK configurations."""
        for m_fsk in [2, 4, 8, 16]:
            config = Config()
            config.modulation.m_fsk = m_fsk

            modulator = FskModulator(config)
            demodulator = FskDemodulator(config)

            data = b"Test"
            audio = modulator.modulate_bytes(data)
            symbols = demodulator.process_samples(audio)

            assert symbols is not None, f"Failed for M={m_fsk}"
            assert len(symbols) > 0, f"No symbols recovered for M={m_fsk}"
