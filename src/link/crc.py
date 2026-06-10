"""CRC-32 checksum computation and verification.

Uses CRC-32 (polynomial 0xEDB88320, standard Ethernet/ZIP CRC) for
frame integrity verification.
"""

import struct
from typing import Tuple

import crcmod

# Pre-compile the CRC-32 function for performance
_crc32_func = crcmod.predefined.mkCrcFun("crc-32")


def compute_crc32(data: bytes) -> int:
    """Compute CRC-32 checksum of data.

    Args:
        data: Data bytes to checksum.

    Returns:
        32-bit CRC value.
    """
    return _crc32_func(data)


def compute_crc32_bytes(data: bytes) -> bytes:
    """Compute CRC-32 and return as 4 bytes (big-endian).

    Args:
        data: Data bytes to checksum.

    Returns:
        4-byte CRC value in big-endian format.
    """
    crc = compute_crc32(data)
    return struct.pack(">I", crc)


def verify_crc32(data: bytes, expected_crc: int) -> bool:
    """Verify CRC-32 checksum.

    Args:
        data: Data bytes to verify.
        expected_crc: Expected CRC value.

    Returns:
        True if CRC matches.
    """
    return compute_crc32(data) == expected_crc


def verify_crc32_bytes(data: bytes, expected_crc_bytes: bytes) -> bool:
    """Verify CRC-32 checksum from 4-byte representation.

    Args:
        data: Data bytes to verify.
        expected_crc_bytes: 4-byte CRC value (big-endian).

    Returns:
        True if CRC matches.
    """
    expected_crc = struct.unpack(">I", expected_crc_bytes)[0]
    return verify_crc32(data, expected_crc)


def compute_and_append_crc(data: bytes) -> bytes:
    """Compute CRC-32 and append to data.

    Args:
        data: Data bytes.

    Returns:
        Data with 4-byte CRC appended.
    """
    return data + compute_crc32_bytes(data)


def verify_and_strip_crc(data_with_crc: bytes) -> Tuple[bytes, bool]:
    """Verify CRC-32 and strip from data.

    Args:
        data_with_crc: Data with 4-byte CRC appended.

    Returns:
        Tuple of (data_without_crc, is_valid).
    """
    if len(data_with_crc) < 4:
        return data_with_crc, False

    data = data_with_crc[:-4]
    crc_bytes = data_with_crc[-4:]
    is_valid = verify_crc32_bytes(data, crc_bytes)
    return data, is_valid
