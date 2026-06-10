"""File transfer protocol for acoustically coupled data transmission.

Protocol flow:
    TX: FILE_REQ -> DATA_FRAMES -> FILE_COMPLETE
    RX: FILE_ACK -> (receive data) -> FILE_VERIFY

Command codes:
    0x01: FILE_REQ  - Request to send file (filename, size)
    0x02: FILE_ACK  - Acknowledge file request
    0x03: DATA      - Data frame (chunk of file)
    0x04: DATA_ACK  - Acknowledge data frame
    0x05: FILE_COMPLETE - File transmission complete
    0x06: FILE_VERIFY   - Verify file received correctly (CRC)
    0x07: FILE_OK       - File verified OK
    0x08: FILE_FAIL     - File verification failed
    0x09: RETRY         - Request retransmission of frame
"""

import hashlib
import logging
import os
import struct
import threading
import time
from enum import IntEnum
from typing import Callable, Dict, Optional

from src.config import Config
from src.link.framing import FrameAssembler, FrameParser

logger = logging.getLogger(__name__)


class ProtocolCommand(IntEnum):
    FILE_REQ = 0x01
    FILE_ACK = 0x02
    DATA = 0x03
    DATA_ACK = 0x04
    FILE_COMPLETE = 0x05
    FILE_VERIFY = 0x06
    FILE_OK = 0x07
    FILE_FAIL = 0x08
    RETRY = 0x09
    HEARTBEAT = 0x0A


class TransferState:
    IDLE = "idle"
    REQUESTING = "requesting"
    TRANSFERRING = "transferring"
    VERIFYING = "verifying"
    COMPLETE = "complete"
    FAILED = "failed"


class TransferProgress:
    """Container for file transfer progress information."""

    def __init__(self):
        self.state: str = TransferState.IDLE
        self.filename: str = ""
        self.file_size: int = 0
        self.bytes_transferred: int = 0
        self.frames_sent: int = 0
        self.frames_received: int = 0
        self.frames_retransmitted: int = 0
        self.start_time: float = 0.0
        self.end_time: float = 0.0
        self.error_message: str = ""

    @property
    def progress_percent(self) -> float:
        if self.file_size == 0:
            return 0.0
        return (self.bytes_transferred / self.file_size) * 100.0

    @property
    def elapsed_time(self) -> float:
        if self.start_time == 0:
            return 0.0
        end = self.end_time if self.end_time > 0 else time.time()
        return end - self.start_time

    @property
    def throughput_bps(self) -> float:
        elapsed = self.elapsed_time
        if elapsed == 0:
            return 0.0
        return self.bytes_transferred / elapsed

    @property
    def throughput_string(self) -> str:
        bps = self.throughput_bps
        if bps >= 1000000:
            return f"{bps / 1000000:.2f} MB/s"
        elif bps >= 1000:
            return f"{bps / 1000:.2f} KB/s"
        else:
            return f"{bps:.0f} B/s"


class FileTransferProtocol:
    """File transfer protocol with ACK/retransmission.

    Handles the complete file transfer handshake:
    1. TX sends FILE_REQ with filename and size
    2. RX responds with FILE_ACK
    3. TX sends DATA frames with file chunks
    4. RX acknowledges each DATA frame with DATA_ACK
    5. TX sends FILE_COMPLETE when all data sent
    6. RX sends FILE_VERIFY with file CRC
    7. TX responds with FILE_OK or FILE_FAIL
    """

    def __init__(self, config: Config):
        self.config = config
        self.assembler = FrameAssembler(config)
        self.parser = FrameParser(config)

        self.progress = TransferProgress()

        # Callbacks for sending frames and receiving data
        self.on_frame_send: Optional[Callable] = None  # (frame_bytes) -> None
        self.on_control_received: Optional[Callable] = None  # (command, data) -> None
        self.on_progress_update: Optional[Callable] = None  # (TransferProgress) -> None

        # Internal state
        self._lock = threading.Lock()
        self._file_data: bytes = b""
        self._received_data: bytearray = bytearray()
        self._current_chunk: int = 0
        self._pending_acks: Dict[int, bytes] = {}  # chunk_num -> data for retransmission
        self._file_crc: str = ""

    def initiate_send(self, filepath: str):
        """Initiate sending a file.

        Args:
            filepath: Path to the file to send.
        """
        if not os.path.exists(filepath):
            logger.error(f"File not found: {filepath}")
            return False

        file_size = os.path.getsize(filepath)
        filename = os.path.basename(filepath)

        # Read file data
        with open(filepath, "rb") as f:
            self._file_data = f.read()

        # Compute file CRC
        self._file_crc = hashlib.md5(self._file_data).hexdigest()

        # Reset state
        self.progress = TransferProgress()
        self.progress.filename = filename
        self.progress.file_size = file_size
        self.progress.state = TransferState.REQUESTING
        self.progress.start_time = time.time()
        self._current_chunk = 0
        self._pending_acks.clear()

        logger.info(f"Initiating send of {filename} ({file_size} bytes)")

        # Send FILE_REQ
        req_data = filename.encode("utf-8").ljust(64, b"\x00")
        req_data += struct.pack(">Q", file_size)
        req_data += self._file_crc.encode("utf-8").ljust(32, b"\x00")
        self._send_control(ProtocolCommand.FILE_REQ, req_data)

        self._update_progress()
        return True

    def initiate_receive(self) -> bool:
        """Prepare to receive a file.

        Returns:
            True if ready to receive.
        """
        self.progress = TransferProgress()
        self.progress.state = TransferState.IDLE
        self._received_data = bytearray()
        self._current_chunk = 0
        self._pending_acks.clear()

        logger.info("Ready to receive file")
        return True

    def handle_received_frame(self, payload: bytes, seq: int, frame_type: int, valid: bool):
        """Process a received frame.

        Args:
            payload: Frame payload data.
            seq: Sequence number.
            frame_type: Frame type (0=data, 1=control).
            valid: Whether the frame passed FEC/CRC checks.
        """
        if not valid:
            logger.warning(f"Received invalid frame seq={seq}")
            return

        if frame_type == 1:  # Control frame
            if len(payload) < 1:
                return
            command = payload[0]
            control_data = payload[1:]
            self._handle_control(command, control_data, seq)
        else:  # Data frame
            self._handle_data(payload, seq)

    def _handle_control(self, command: int, data: bytes, seq: int):
        """Handle a control frame."""
        logger.debug(f"Received control: command={command:#x}, seq={seq}")

        if command == ProtocolCommand.FILE_REQ:
            # Extract filename, size, and CRC
            filename = data[:64].rstrip(b"\x00").decode("utf-8", errors="replace")
            file_size = struct.unpack(">Q", data[64:72])[0]
            file_crc = data[72:104].rstrip(b"\x00").decode("utf-8", errors="replace")

            self.progress.filename = filename
            self.progress.file_size = file_size
            self._file_crc = file_crc
            self.progress.state = TransferState.TRANSFERRING

            logger.info(f"Receiving file: {filename} ({file_size} bytes)")

            # Respond with FILE_ACK
            self._send_control(ProtocolCommand.FILE_ACK, b"")

        elif command == ProtocolCommand.FILE_ACK:
            logger.info("File request acknowledged, starting transfer")
            self.progress.state = TransferState.TRANSFERRING
            self._send_next_chunk()

        elif command == ProtocolCommand.DATA_ACK:
            # Extract acknowledged chunk number
            if len(data) >= 2:
                ack_chunk = struct.unpack(">H", data[:2])[0]
                if ack_chunk in self._pending_acks:
                    del self._pending_acks[ack_chunk]
                self._current_chunk = ack_chunk + 1
                logger.debug(f"ACK for chunk {ack_chunk}")
                self._send_next_chunk()

        elif command == ProtocolCommand.FILE_COMPLETE:
            logger.info("All data frames received")
            self.progress.state = TransferState.VERIFYING
            self.progress.bytes_transferred = len(self._received_data)

            # Compute received file CRC
            received_crc = hashlib.md5(bytes(self._received_data)).hexdigest()

            # Send FILE_VERIFY with received CRC
            verify_data = received_crc.encode("utf-8").ljust(32, b"\x00")
            self._send_control(ProtocolCommand.FILE_VERIFY, verify_data)

        elif command == ProtocolCommand.FILE_VERIFY:
            received_crc = data[:32].rstrip(b"\x00").decode("utf-8", errors="replace")
            if received_crc == self._file_crc:
                logger.info("File verification OK")
                self._send_control(ProtocolCommand.FILE_OK, b"")
                self.progress.state = TransferState.COMPLETE
            else:
                logger.error(f"File verification failed: expected {self._file_crc}, got {received_crc}")
                self._send_control(ProtocolCommand.FILE_FAIL, b"")
                self.progress.state = TransferState.FAILED
                self.progress.error_message = "CRC mismatch"

        elif command == ProtocolCommand.FILE_OK:
            logger.info("File transfer complete and verified")
            self.progress.state = TransferState.COMPLETE
            self.progress.end_time = time.time()

        elif command == ProtocolCommand.FILE_FAIL:
            logger.error("File verification failed on receiver")
            self.progress.state = TransferState.FAILED
            self.progress.error_message = "Receiver verification failed"

        elif command == ProtocolCommand.RETRY:
            if len(data) >= 2:
                retry_chunk = struct.unpack(">H", data[:2])[0]
                logger.info(f"Retransmitting chunk {retry_chunk}")
                self.progress.frames_retransmitted += 1
                if retry_chunk in self._pending_acks:
                    self._send_data_chunk(retry_chunk, self._pending_acks[retry_chunk])

        # Notify UI of control command
        if self.on_control_received:
            self.on_control_received(command, data)

        self._update_progress()

    def _handle_data(self, data: bytes, seq: int):
        """Handle a data frame."""
        # Extract chunk number from first 2 bytes
        if len(data) < 2:
            return

        chunk_num = struct.unpack(">H", data[:2])[0]
        chunk_data = data[2:]

        # Store data (handle out-of-order and duplicates)
        if chunk_num == self._current_chunk:
            self._received_data.extend(chunk_data)
            self.progress.frames_received += 1
            self.progress.bytes_transferred = len(self._received_data)

            # Send DATA_ACK
            ack_data = struct.pack(">H", chunk_num)
            self._send_control(ProtocolCommand.DATA_ACK, ack_data)

    def _send_next_chunk(self):
        """Send the next chunk of file data."""
        if self.progress.state != TransferState.TRANSFERRING:
            return

        chunk_size = self.config.frame.payload_size - 2  # minus 2 bytes for chunk number
        start = self._current_chunk * chunk_size
        end = min(start + chunk_size, len(self._file_data))

        if start >= len(self._file_data):
            # All data sent
            self._send_control(ProtocolCommand.FILE_COMPLETE, b"")
            self.progress.state = TransferState.VERIFYING
            self._update_progress()
            return

        chunk_data = self._file_data[start:end]
        self._send_data_chunk(self._current_chunk, chunk_data)

    def _send_data_chunk(self, chunk_num: int, data: bytes):
        """Send a data chunk as a frame."""
        # Prepend chunk number
        frame_data = struct.pack(">H", chunk_num) + data

        frame = self.assembler.assemble_frame(frame_data, frame_type=0)
        self._pending_acks[chunk_num] = data
        self.progress.frames_sent += 1

        if self.on_frame_send:
            self.on_frame_send(frame)

        self._update_progress()

    def _send_control(self, command: int, data: bytes):
        """Send a control frame."""
        frame = self.assembler.assemble_control_frame(command, data)

        if self.on_frame_send:
            self.on_frame_send(frame)

    def _update_progress(self):
        """Notify UI of progress update."""
        if self.on_progress_update:
            self.on_progress_update(self.progress)

    def cancel(self):
        """Cancel the current transfer."""
        self.progress.state = TransferState.IDLE
        self.progress.error_message = "Cancelled"
        self._update_progress()
        logger.info("Transfer cancelled")
