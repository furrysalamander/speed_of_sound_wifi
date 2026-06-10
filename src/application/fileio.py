"""File I/O utilities for the file transfer protocol."""

import hashlib
import logging
import os
from typing import Callable, Optional, Tuple

logger = logging.getLogger(__name__)


def get_file_info(filepath: str) -> Tuple[int, str]:
    """Get file size and MD5 checksum.

    Args:
        filepath: Path to the file.

    Returns:
        Tuple of (file_size, md5_hexdigest).
    """
    file_size = os.path.getsize(filepath)
    md5 = hashlib.md5()

    with open(filepath, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            md5.update(chunk)

    return file_size, md5.hexdigest()


def read_file_chunks(
    filepath: str,
    chunk_size: int,
    progress_callback: Optional[Callable] = None,
) -> Tuple[bytes, int]:
    """Read file and split into chunks.

    Args:
        filepath: Path to the file.
        chunk_size: Size of each chunk in bytes.
        progress_callback: Optional callback(bytes_read, total_bytes).

    Returns:
        Tuple of (file_data, num_chunks).
    """
    with open(filepath, "rb") as f:
        data = f.read()

    file_size = len(data)
    num_chunks = (file_size + chunk_size - 1) // chunk_size

    if progress_callback:
        progress_callback(file_size, file_size)

    return data, num_chunks


def write_file_data(filepath: str, data: bytes, overwrite: bool = False) -> bool:
    """Write received data to a file.

    Args:
        filepath: Output file path.
        data: Data to write.
        overwrite: Whether to overwrite existing file.

    Returns:
        True if successful.
    """
    if os.path.exists(filepath) and not overwrite:
        logger.error(f"File already exists: {filepath}")
        return False

    # Create directory if needed
    directory = os.path.dirname(filepath)
    if directory and not os.path.exists(directory):
        os.makedirs(directory, exist_ok=True)

    with open(filepath, "wb") as f:
        f.write(data)

    logger.info(f"Wrote {len(data)} bytes to {filepath}")
    return True


def verify_file(filepath: str, expected_md5: str) -> bool:
    """Verify file integrity using MD5 checksum.

    Args:
        filepath: Path to the file.
        expected_md5: Expected MD5 hex digest.

    Returns:
        True if checksum matches.
    """
    actual_md5 = hashlib.md5()
    with open(filepath, "rb") as f:
        for chunk in iter(lambda: f.read(8192), b""):
            actual_md5.update(chunk)

    matches = actual_md5.hexdigest() == expected_md5
    if matches:
        logger.info(f"File verification OK: {filepath}")
    else:
        logger.error(
            f"File verification FAILED: {filepath} "
            f"(expected {expected_md5}, got {actual_md5.hexdigest()})"
        )
    return matches


def format_file_size(size_bytes: int) -> str:
    """Format file size in human-readable format.

    Args:
        size_bytes: Size in bytes.

    Returns:
        Formatted string (e.g., "1.23 MB").
    """
    if size_bytes < 1024:
        return f"{size_bytes} B"
    elif size_bytes < 1024 * 1024:
        return f"{size_bytes / 1024:.2f} KB"
    elif size_bytes < 1024 * 1024 * 1024:
        return f"{size_bytes / (1024 * 1024):.2f} MB"
    else:
        return f"{size_bytes / (1024 * 1024 * 1024):.2f} GB"
