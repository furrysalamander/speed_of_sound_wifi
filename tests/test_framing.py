"""Tests for frame protocol."""

import pytest

from src.config import Config
from src.link.framing import FrameAssembler, FrameParser
from src.link.crc import compute_crc32, compute_crc32_bytes, verify_crc32_bytes


@pytest.fixture
def config():
    """Create a test configuration."""
    return Config()


@pytest.fixture
def assembler(config):
    """Create a frame assembler."""
    return FrameAssembler(config)


@pytest.fixture
def parser(config):
    """Create a frame parser."""
    return FrameParser(config)


class TestCrc:
    """Tests for CRC-32."""

    def test_compute_crc(self):
        """Test CRC computation."""
        data = b"Hello, World!"
        crc = compute_crc32(data)
        assert isinstance(crc, int)
        assert 0 <= crc <= 0xFFFFFFFF

    def test_crc_consistency(self):
        """Test that same data produces same CRC."""
        data = b"Test data"
        crc1 = compute_crc32(data)
        crc2 = compute_crc32(data)
        assert crc1 == crc2

    def test_crc_different_data(self):
        """Test that different data produces different CRC."""
        data1 = b"AAA"
        data2 = b"AAB"
        crc1 = compute_crc32(data1)
        crc2 = compute_crc32(data2)
        assert crc1 != crc2

    def test_crc_bytes_roundtrip(self):
        """Test CRC bytes encoding/decoding."""
        data = b"Test"
        crc_bytes = compute_crc32_bytes(data)
        assert len(crc_bytes) == 4
        assert verify_crc32_bytes(data, crc_bytes) is True

    def test_crc_verification_fail(self):
        """Test CRC verification with wrong data."""
        data = b"Correct"
        wrong_data = b"Wrong"
        crc_bytes = compute_crc32_bytes(data)
        assert verify_crc32_bytes(wrong_data, crc_bytes) is False


class TestFrameAssembler:
    """Tests for frame assembler."""

    def test_assemble_frame(self, assembler):
        """Test frame assembly."""
        payload = b"Hello, World!"
        frame = assembler.assemble_frame(payload)

        assert isinstance(frame, bytes)
        assert len(frame) > len(payload)
        # Frame should start with sync pattern
        assert frame.startswith(b"\xAA\x55\xAA\x55\xAA\x55\xAA\x55")

    def test_sequence_numbers_increment(self, assembler):
        """Test that sequence numbers increment."""
        payload = b"Test"
        frame1 = assembler.assemble_frame(payload)
        frame2 = assembler.assemble_frame(payload)

        # Frames should be different due to sequence number
        assert frame1 != frame2

    def test_control_frame(self, assembler):
        """Test control frame assembly."""
        frame = assembler.assemble_control_frame(0x01, b"data")
        assert isinstance(frame, bytes)
        assert len(frame) > 0


class TestFrameParser:
    """Tests for frame parser."""

    def test_parse_valid_frame(self, assembler, parser):
        """Test parsing a valid frame."""
        payload = b"Hello, World!"
        frame = assembler.assemble_frame(payload)

        # Feed frame to parser
        frames = parser.feed_bytes(frame)

        assert len(frames) > 0
        parsed_payload, seq, frame_type, valid = frames[0]
        assert valid is True
        assert frame_type == 0  # data frame
        assert parsed_payload == payload

    def test_parse_with_noise(self, assembler, parser):
        """Test parsing frame with some noise."""
        payload = b"Test data"
        frame = assembler.assemble_frame(payload)

        # Add some random bytes before the frame
        noisy_frame = b"\x00\x00\x00" + frame + b"\xFF\xFF"
        frames = parser.feed_bytes(noisy_frame)

        assert len(frames) > 0
        parsed_payload, seq, frame_type, valid = frames[0]
        assert valid is True

    def test_stats_tracking(self, assembler, parser):
        """Test that stats are tracked."""
        payload = b"Test"
        frame = assembler.assemble_frame(payload)
        parser.feed_bytes(frame)

        stats = parser.stats
        assert stats["frames_received"] > 0
        assert stats["frames_valid"] > 0


class TestFrameRoundTrip:
    """Integration tests for frame assembly and parsing."""

    def test_roundtrip_small_payload(self, config):
        """Test round-trip with small payload."""
        assembler = FrameAssembler(config)
        parser = FrameParser(config)

        payload = b"Hi"
        frame = assembler.assemble_frame(payload)
        frames = parser.feed_bytes(frame)

        assert len(frames) == 1
        parsed_payload, _, _, valid = frames[0]
        assert valid
        assert parsed_payload == payload

    def test_roundtrip_max_payload(self, config):
        """Test round-trip with maximum payload size."""
        assembler = FrameAssembler(config)
        parser = FrameParser(config)

        payload = b"X" * config.frame.payload_size
        frame = assembler.assemble_frame(payload)
        frames = parser.feed_bytes(frame)

        assert len(frames) == 1
        parsed_payload, _, _, valid = frames[0]
        assert valid
        assert parsed_payload == payload

    def test_multiple_frames(self, config):
        """Test parsing multiple frames."""
        assembler = FrameAssembler(config)
        parser = FrameParser(config)

        # Assemble and concatenate multiple frames
        all_frames = b""
        payloads = []
        for i in range(5):
            payload = f"Frame {i}".encode()
            payloads.append(payload)
            frame = assembler.assemble_frame(payload)
            all_frames += frame

        # Parse all frames
        frames = parser.feed_bytes(all_frames)
        assert len(frames) == 5

        for i, (parsed_payload, _, _, valid) in enumerate(frames):
            assert valid
            assert parsed_payload == payloads[i]
