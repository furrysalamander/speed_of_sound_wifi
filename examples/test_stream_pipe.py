#!/usr/bin/env python3
"""Test the streaming TX+RX pipeline end-to-end.

Launches RX in a subprocess, then TX, and verifies received data matches.
"""

import subprocess
import sys
import time
from pathlib import Path

import numpy as np

PYTHON = str(Path(sys.executable).parent / "python") if "venv" in sys.executable else sys.executable

# Generate random test data (8 frames = 4 bursts of 2 frames = 1784 bytes)
data = np.random.default_rng(seed=123).integers(0, 256, size=1784, dtype=np.uint8).tobytes()
with open("/tmp/stream_pipe_test_in.bin", "wb") as f:
    f.write(data)
print(f"Test data: {len(data)} bytes, first 8: {data[:8].hex()}")

# Start RX first
rx_proc = subprocess.Popen(
    [sys.executable, "-m", "examples.stream_rx",
     "--output", "/tmp/stream_pipe_test_out.bin",
     "--output-name", "USB_PnP"],
    stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
)
time.sleep(0.5)  # let RX initialize

# Run TX (continuous = zero gaps between bursts to keep AGC locked)
tx_proc = subprocess.run(
    [sys.executable, "-m", "examples.stream_tx_continuous",
     "--input", "/tmp/stream_pipe_test_in.bin",
     "--input-name", "analog-stereo",
     "--frames-per-burst", "2"],
    capture_output=True, text=True,
)
print(f"TX stderr: {tx_proc.stderr.strip() if tx_proc.stderr else 'none'}")

# Wait for RX to finish processing
time.sleep(4.0)

# Get RX stderr before terminating
rx_proc.terminate()
try:
    rx_proc.wait(timeout=5)
except subprocess.TimeoutExpired:
    rx_proc.kill()

rx_stderr = rx_proc.stderr.read().decode() if rx_proc.stderr else ""
print(f"RX stderr:")
for line in rx_stderr.split("\n"):
    if line.strip():
        print(f"  {line.strip()}")

# Compare
with open("/tmp/stream_pipe_test_out.bin", "rb") as f:
    received = f.read()

match = received == data
print(f"Received: {len(received)} bytes, match={match}")
if not match:
    # Show first mismatch
    for i in range(min(len(received), len(data))):
        if received[i] != data[i]:
            print(f"  First mismatch at byte {i}: got {received[i]:02x}, expected {data[i]:02x}")
            break
    if len(received) != len(data):
        print(f"  Length mismatch: rx={len(received)} tx={len(data)}")