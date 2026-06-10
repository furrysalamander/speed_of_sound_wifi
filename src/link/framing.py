"""Frame protocol for data transmission.

Frame structure:
    [SYNC PREAMBLE][FRAME HEADER][PAYLOAD][FEC PARITY][CRC-32]

    - SYNC: 8-byte sync pattern for frame synchronization
    - HEADER: 4 bytes (2 payload_length + 2 sequence_number)
    - PAYLOAD: Variable length data (up to RS block size)
    - FEC PARITY: Reed-Solomon parity bytes
    - CRC-32: 4-byte checksum over header + payload + fec
"""

import logging
import struct
from typing import Optional, Tuple

import numpy as np

from src.config import Config
from src.link.crc import compute_crc32_bytes, verify_crc32_bytes
from src.link.fec import ReedSolomonFec

logger = logging.getLogger(__name__)


# Frame format constants
SYNC_PATTERN = b"\xAA\x55\xAA\x55\xAA\x55\xAA\x55"
HEADER_SIZE = 4  # 2 bytes payload_length + 2 bytes sequence_number
CRC_SIZE = 4  # CRC-32


class FrameAssembler:
    """Assemble data into transmission frames."""

    def __init__(self, config: Config):
        self.config = config
        self.fec = ReedSolomonFec(config)
        self._sequence_number: int = 0

    def assemble_frame(self, payload: bytes, frame_type: int = 0) -> bytes:
        """Assemble a complete frame with sync, header, FEC, and CRC.

        Args:
            payload: Data payload bytes.
            frame_type: Frame type identifier (0=data, 1=control).

        Returns:
            Complete frame bytes ready for modulation.
        """
        # Build header: payload_length (2 bytes) + sequence_number (2 bytes)
        # Upper nibble of first byte is frame_type
        header_byte0 = (frame_type << 4) | (len(payload) >> 8)
        header_byte1 = len(payload) & 0xFF
        header_byte2 = (self._sequence_number >> 8) & 0xFF
        header_byte3 = self._sequence_number & 0xFF
        header = bytes([header_byte0, header_byte1, header_byte2, header_byte3])

        # Combine header + payload for FEC encoding
        data_for_fec = header + payload

        # Apply FEC
        fec_encoded = self.fec.encode(data_for_fec)

        # Compute CRC over FEC-encoded data
        crc = compute_crc32_bytes(fec_encoded)

        # Build complete frame
        frame = SYNC_PATTERN + fec_encoded + crc

        self._sequence_number = (self._sequence_number + 1) & 0xFFFF
        return frame

    def assemble_control_frame(self, command: int, data: bytes = b"") -> bytes:
        """Assemble a control frame for protocol signaling.

        Args:
            command: Command byte (see ProtocolCommands).
            data: Optional data payload.

        Returns:
            Complete control frame bytes.
        """
        payload = bytes([command]) + data
        return self.assemble_frame(payload, frame_type=1)

    @property
    def sequence_number(self) -> int:
        return self._sequence_number

    def reset_sequence(self):
        self._sequence_number = 0


class FrameParser:
    """Parse received bytes into frames and extract payload."""

    def __init__(self, config: Config):
        self.config = config
        self.fec = ReedSolomonFec(config)
        self._buffer = bytearray()

        # Stats
        self._frames_received: int = 0
        self._frames_valid: int = 0
        self._frames_crc_fail: int = 0
        self._frames_fec_fail: int = 0

    def feed_bytes(self, data: bytes):
        """Feed received bytes into the parser.

        Args:
            data: New bytes to parse.

        Returns:
            List of (payload, sequence_number, frame_type, is_valid) tuples.
        """
        self._buffer.extend(data)
        frames = []

        while len(self._buffer) >= len(SYNC_PATTERN) + HEADER_SIZE + CRC_SIZE:
            # Search for sync pattern (allow up to 2 byte errors for robustness)
            sync_idx = self._buffer.find(SYNC_PATTERN)
            if sync_idx == -1:
                # Soft search: find position with best Hamming match to sync pattern
                best_idx = -1
                best_score = 0
                for i in range(len(self._buffer) - len(SYNC_PATTERN) + 1):
                    chunk = self._buffer[i:i+len(SYNC_PATTERN)]
                    score = sum(1 for a, b in zip(chunk, SYNC_PATTERN) if a == b)
                    if score > best_score:
                        best_score = score
                        best_idx = i
                if best_score >= 5:  # at least 5 of 8 bytes match
                    sync_idx = best_idx
                else:
                    # No sync found, clear buffer
                    self._buffer.clear()
                    break

            # Skip anything before sync
            if sync_idx > 0:
                self._buffer = self._buffer[sync_idx:]

            # Check if we have enough data for a minimum frame
            min_frame_size = len(SYNC_PATTERN) + self.fec.get_data_size() + self.fec.nsym + CRC_SIZE
            if len(self._buffer) < min_frame_size:
                break  # wait for more data

            # Try to parse frame starting at sync
            frame_start = 0  # sync is at start of buffer now

            # Extract FEC-encoded data (after sync, before CRC)
            # We need to figure out the frame size
            # RS block size is 255 bytes
            rs_block_size = 255

            # Try different numbers of RS blocks.
            # Compute max possible blocks from buffer length to avoid wasted CRC checks.
            max_possible = (len(self._buffer) - len(SYNC_PATTERN) - CRC_SIZE) // rs_block_size
            parsed = False
            for num_blocks in range(1, min(max_possible + 1, 20)):
                fec_data_len = num_blocks * rs_block_size
                total_frame_size = len(SYNC_PATTERN) + fec_data_len + CRC_SIZE

                if len(self._buffer) < total_frame_size:
                    continue

                # Extract FEC data and CRC
                fec_data = bytes(
                    self._buffer[
                        frame_start + len(SYNC_PATTERN) : frame_start
                        + len(SYNC_PATTERN)
                        + fec_data_len
                    ]
                )
                crc_bytes = bytes(
                    self._buffer[
                        frame_start + len(SYNC_PATTERN) + fec_data_len : frame_start
                        + total_frame_size
                    ]
                )

                # Verify CRC
                if verify_crc32_bytes(fec_data, crc_bytes):
                    # CRC valid, decode FEC
                    decoded_data, fec_ok = self.fec.decode(fec_data)
                    self._frames_received += 1

                    if fec_ok:
                        self._frames_valid += 1
                    else:
                        self._frames_fec_fail += 1

                    # Parse header
                    if len(decoded_data) >= HEADER_SIZE:
                        header_byte0 = decoded_data[0]
                        header_byte1 = decoded_data[1]
                        header_byte2 = decoded_data[2]
                        header_byte3 = decoded_data[3]

                        frame_type = (header_byte0 >> 4) & 0x0F
                        payload_length = ((header_byte0 & 0x0F) << 8) | header_byte1
                        seq_number = (header_byte2 << 8) | header_byte3

                        # Extract payload
                        payload_end = HEADER_SIZE + payload_length
                        if payload_end <= len(decoded_data):
                            payload = decoded_data[HEADER_SIZE:payload_end]
                            frames.append((payload, seq_number, frame_type, True))
                        else:
                            logger.warning(
                                f"Payload length {payload_length} exceeds decoded data length"
                            )
                            self._frames_crc_fail += 1
                    else:
                        logger.warning("Header too short")
                        self._frames_crc_fail += 1

                    # Advance buffer past this frame
                    self._buffer = self._buffer[total_frame_size:]
                    parsed = True
                    break
                else:
                    self._frames_crc_fail += 1

            if not parsed:
                # Could not parse frame, skip one byte after sync and retry
                self._buffer = self._buffer[frame_start + 1:]

        return frames

    @property
    def stats(self) -> dict:
        """Return framing statistics."""
        return {
            "frames_received": self._frames_received,
            "frames_valid": self._frames_valid,
            "frames_crc_fail": self._frames_crc_fail,
            "frames_fec_fail": self._frames_fec_fail,
        }
