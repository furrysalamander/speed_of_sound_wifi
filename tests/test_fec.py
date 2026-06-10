"""Tests for Forward Error Correction (Reed-Solomon)."""

import pytest

from src.config import Config
from src.link.fec import ReedSolomonFec


@pytest.fixture
def config():
    """Create a test configuration."""
    return Config()


@pytest.fixture
def fec(config):
    """Create a Reed-Solomon FEC instance."""
    return ReedSolomonFec(config)


class TestReedSolomonFec:
    """Tests for Reed-Solomon FEC."""

    def test_encode_decode_roundtrip(self, fec):
        """Test that encode -> decode recovers original data."""
        data = b"Hello, World! This is a test of Reed-Solomon FEC."
        encoded = fec.encode(data)
        decoded, success = fec.decode(encoded)

        assert success is True
        assert decoded[: len(data)] == data

    def test_correct_single_byte_error(self, fec):
        """Test correction of a single byte error."""
        data = b"A" * 100
        encoded = fec.encode(data)

        # Corrupt one byte
        encoded_mutated = bytearray(encoded)
        encoded_mutated[50] ^= 0xFF  # Flip all bits in byte 50

        decoded, success = fec.decode(bytes(encoded_mutated))
        assert success is True
        assert decoded[: len(data)] == data

    def test_correct_multiple_byte_errors(self, fec):
        """Test correction of multiple byte errors."""
        data = b"B" * 200
        encoded = fec.encode(data)

        # Corrupt several bytes (within correction capability)
        encoded_mutated = bytearray(encoded)
        for i in range(10):
            encoded_mutated[20 + i * 5] ^= 0xFF

        decoded, success = fec.decode(bytes(encoded_mutated))
        assert success is True
        assert decoded[: len(data)] == data

    def test_fec_disabled(self):
        """Test that FEC can be disabled."""
        config = Config()
        config.fec.enabled = False
        fec = ReedSolomonFec(config)

        data = b"Test data"
        encoded = fec.encode(data)
        decoded, success = fec.decode(encoded)

        # When disabled, encode/decode should pass through
        assert encoded == data
        assert decoded == data
        assert success is True

    def test_data_size(self, fec):
        """Test that get_data_size returns correct value."""
        data_size = fec.get_data_size()
        assert data_size == 255 - fec.nsym

    def test_stats_tracking(self, fec):
        """Test that stats are tracked correctly."""
        data = b"Test" * 50
        encoded = fec.encode(data)
        decoded, _ = fec.decode(encoded)

        stats = fec.stats
        assert stats["blocks_encoded"] > 0
        assert stats["blocks_decoded"] > 0

    def test_large_data(self, fec):
        """Test with larger data that spans multiple RS blocks."""
        data = bytes(range(256)) * 4  # 1024 bytes, spans multiple blocks
        encoded = fec.encode(data)
        decoded, success = fec.decode(encoded)

        assert success is True
        assert decoded[: len(data)] == data
