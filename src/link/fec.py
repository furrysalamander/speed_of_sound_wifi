"""Forward Error Correction using Reed-Solomon codes.

Reed-Solomon codes are excellent for burst error correction, which is common
in acoustic channels where noise and distortion can corrupt consecutive bytes.
"""

import logging
from typing import Tuple

import reedsolo

from src.config import Config

logger = logging.getLogger(__name__)


class ReedSolomonFec:
    """Reed-Solomon Forward Error Correction encoder/decoder.

    Uses RS(255, K) codes where K is configurable. The default RS(255, 223)
    adds 32 parity bytes per 223 data bytes, allowing correction of up to
    16 erroneous bytes per block.
    """

    def __init__(self, config: Config):
        self.config = config
        self.nsym = config.fec.nsym  # number of parity symbols
        # RS encoder/decoder operating on 8-bit symbols
        self.rs = reedsolo.RSCodec(self.nsym)

        # Stats
        self._blocks_encoded: int = 0
        self._blocks_decoded: int = 0
        self._errors_corrected: int = 0
        self._uncorrectable_errors: int = 0

    def encode(self, data: bytes) -> bytes:
        """Encode data with Reed-Solomon parity.

        Args:
            data: Data bytes to encode (max 255 - nsym bytes per block).

        Returns:
            Encoded bytes including parity.
        """
        if not self.config.fec.enabled:
            return data

        # Split data into blocks that fit RS encoder
        data_len = 255 - self.nsym
        encoded_parts = []

        for i in range(0, len(data), data_len):
            block = data[i : i + data_len]
            # Pad block if necessary
            if len(block) < data_len:
                block = block + b"\x00" * (data_len - len(block))
            encoded_block = bytes(self.rs.encode(block))
            encoded_parts.append(encoded_block)
            self._blocks_encoded += 1

        return b"".join(encoded_parts)

    def decode(self, encoded_data: bytes) -> Tuple[bytes, bool]:
        """Decode Reed-Solomon encoded data and correct errors.

        Args:
            encoded_data: Encoded bytes including parity.

        Returns:
            Tuple of (decoded_data, success_flag).
            success_flag is False if uncorrectable errors were found.
        """
        if not self.config.fec.enabled:
            return encoded_data, True

        # Split into RS blocks
        block_size = 255
        decoded_parts = []
        all_success = True
        total_errors = 0

        for i in range(0, len(encoded_data), block_size):
            block = encoded_data[i : i + block_size]
            if len(block) < block_size:
                # Handle partial block at the end
                block = block + b"\x00" * (block_size - len(block))

            try:
                # RSCodec.decode returns (decoded_msg, encoded_corrected, error_locations)
                result = self.rs.decode(bytearray(block))
                decoded_block = bytes(result[0])
                error_locations = result[2]
                num_errors = len(error_locations) if error_locations else 0
                if num_errors > 0:
                    total_errors += num_errors
                    self._errors_corrected += num_errors
                    logger.debug(f"Corrected {num_errors} errors in RS block")
                self._blocks_decoded += 1
                decoded_parts.append(decoded_block)
            except reedsolo.ReedSolomonError as e:
                logger.error(f"Uncorrectable RS error: {e}")
                self._uncorrectable_errors += 1
                all_success = False
                # Return the block as-is (best effort)
                decoded_parts.append(block[: 255 - self.nsym])

        return b"".join(decoded_parts), all_success

    def get_data_size(self) -> int:
        """Return the maximum data size per RS block."""
        return 255 - self.nsym

    @property
    def stats(self) -> dict:
        """Return FEC statistics."""
        return {
            "blocks_encoded": self._blocks_encoded,
            "blocks_decoded": self._blocks_decoded,
            "errors_corrected": self._errors_corrected,
            "uncorrectable_errors": self._uncorrectable_errors,
        }
