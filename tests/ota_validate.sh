#!/usr/bin/env bash
set -euo pipefail

# OTA Validation Test
# Transmits known data and verifies reception over the air.
# Usage: ./tests/ota_validate.sh [tx_device] [rx_device] [n_frames]
# Example: ./tests/ota_validate.sh "pulse" "USB" 68

TX_DEVICE="${1:-pulse}"
RX_DEVICE="${2:-USB}"
N_FRAMES="${3:-68}"
PAYLOAD_SIZE=442  # matches Config::ofdm_default()

SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$SCRIPT_DIR"

INPUT_FILE=$(mktemp /tmp/ota_input_XXXXXX.bin)
OUTPUT_FILE=$(mktemp /tmp/ota_output_XXXXXX.bin)

cleanup() {
    rm -f "$INPUT_FILE" "$OUTPUT_FILE"
}
trap cleanup EXIT

echo "=== OTA Validation ==="
echo "TX device: $TX_DEVICE"
echo "RX device: $RX_DEVICE"
echo "Frames: $N_FRAMES (payload size: $PAYLOAD_SIZE)"
echo "Total payload: $((N_FRAMES * PAYLOAD_SIZE)) bytes"

# Generate test payload
python3 -c "
import sys
n = $N_FRAMES
ps = $PAYLOAD_SIZE
sys.stdout.buffer.write(bytes(i % 256 for i in range(n * ps)))
" > "$INPUT_FILE"

echo "Payload generated: $(wc -c < "$INPUT_FILE") bytes"

# Start RX in background
echo "Starting RX..."
cargo run --release -p sosw-cli -- rx \
    --count "$N_FRAMES" \
    --device "$RX_DEVICE" \
    --output "$OUTPUT_FILE" &
RX_PID=$!
sleep 2

echo "Starting TX..."
cargo run --release -p sosw-cli -- tx \
    "$INPUT_FILE" \
    --device "$TX_DEVICE"
echo "TX complete"

echo "Waiting for RX to finish..."
wait $RX_PID 2>/dev/null || true
echo "RX complete"

# Analyze results
if [ ! -f "$OUTPUT_FILE" ]; then
    echo "FAIL: No output file generated"
    exit 1
fi

RX_SIZE=$(stat -c%s "$OUTPUT_FILE" 2>/dev/null || stat -f%z "$OUTPUT_FILE" 2>/dev/null || echo 0)
echo "Output size: $RX_SIZE bytes"

# Compute frame-level match
MATCHING=0
for ((i=0; i<N_FRAMES; i++)); do
    OFF=$((i * PAYLOAD_SIZE))
    SENT=$(xxd -l "$PAYLOAD_SIZE" -s "$OFF" "$INPUT_FILE" 2>/dev/null | md5sum | cut -d' ' -f1)
    RECV=$(xxd -l "$PAYLOAD_SIZE" -s "$((i * PAYLOAD_SIZE))" "$OUTPUT_FILE" 2>/dev/null | md5sum | cut -d' ' -f1)
    if [ "$SENT" = "$RECV" ]; then
        MATCHING=$((MATCHING + 1))
    fi
done

PCT=$((MATCHING * 100 / N_FRAMES))
echo ""
echo "=== Results ==="
echo "Frames received: $MATCHING / $N_FRAMES = ${PCT}%"

if [ "$PCT" -ge 90 ]; then
    echo "Result: PASS (threshold >= 90%)"
    exit 0
else
    echo "Result: FAIL (threshold >= 90%)"
    exit 1
fi
