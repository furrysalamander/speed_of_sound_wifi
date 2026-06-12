# Speed of Sound WiFi

Acoustically coupled data transmission over audio with OFDM modulation.  
**Rust** (OFDM) and **Python** (legacy M-FSK) implementations.

## Rust WASM Web App

The `sosw-web` crate provides a browser-based OFDM modem with:

- **RX tab** — real-time audio capture, preamble detection, waterfall spectrogram
- **TX tab** — file upload and transmission via AudioBufferSourceNode
- **Debug tab** — config tuning, enhanced signal monitoring, loopback testing
  - 5 preset configs: Default, High Baud, Robust, Ultrasonic, Ultrawide
  - Subcarrier range sliders, FFT/CP presets, threshold/PLL controls
  - Per-subcarrier channel magnitude bar chart
  - Continuous test signal generation for cross-device RX testing
  - Config persistence via localStorage

```bash
cd sosw-web && trunk serve   # Dev server
cd sosw-web && trunk build --release   # Production build
```

See `sosw-web/src/` for source files (`debug.rs`, `presets.rs`, `rx.rs`, `tx.rs`, `audio.rs`, `waterfall.rs`).

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  Application Layer: File Transfer Protocol                  │
│  (FILE_REQ → FILE_ACK → DATA frames → FILE_COMPLETE)       │
├─────────────────────────────────────────────────────────────┤
│  Link Layer: Frame Protocol                                 │
│  [SYNC 8B][HEADER 4B][PAYLOAD variable][FEC parity][CRC-32]│
├─────────────────────────────────────────────────────────────┤
│  Physical Layer: M-FSK Modulation/Demodulation              │
│  2/4/8/16 tones across 200-18000 Hz                         │
├─────────────────────────────────────────────────────────────┤
│  Audio I/O: sounddevice (PortAudio/ALSA)                    │
│  Full-duplex 48kHz mono, 256-sample buffers                 │
└─────────────────────────────────────────────────────────────┘
```

## Features

- **M-FSK Modulation**: Configurable 2/4/8/16 tones with phase-continuous generation
- **Reed-Solomon FEC**: RS(255,223) codes, corrects up to 16 byte errors per block
- **CRC-32 Integrity**: Checksum verification over FEC-encoded data
- **Frame Synchronization**: 8-byte sync pattern with robust parser
- **File Transfer Protocol**: Handshake, chunked transfer, verification
- **Real-time Waterfall Displays**: GPU-accelerized spectrograms via PyQtGraph
- **Adjustable Parameters**: Baud rate, M-FSK order, frequency range, FEC strength
- **Dual Mode**: Combined (loopback) and separate TX/RX modes

## Installation

```bash
# Create virtual environment
python -m venv .venv
source .venv/bin/activate  # Linux/macOS
# .venv\Scripts\activate   # Windows

# Install dependencies
pip install -r requirements.txt
```

### System Dependencies

- **Linux**: `apt install portaudio19-dev libasound-dev` (Debian/Ubuntu)
- **macOS**: `brew install portaudio`
- **Windows**: Included with sounddevice wheel

## Usage

### GUI Mode (Default)

```bash
python -m src.main
```

### Headless Mode (Testing)

```bash
# Run with default config
python -m src.main --headless

# Specify audio devices
python -m src.main --headless --input-device 1 --output-device 2

# Custom baud rate and M-FSK
python -m src.main --headless --baud-rate 2000 --m-fsk 8
```

### List Audio Devices

```bash
python -c "from src.audio.devices import list_devices; print(list_devices())"
```

## Configuration

| Parameter | Default | Description |
|-----------|---------|-------------|
| `sample_rate` | 48000 | Audio sample rate (Hz) |
| `baud_rate` | 1000 | Symbols per second |
| `m_fsk` | 4 | Number of FSK tones (2/4/8/16) |
| `freq_min` | 200 | Minimum frequency (Hz) |
| `freq_max` | 18000 | Maximum frequency (Hz) |
| `fec_nsym` | 32 | RS parity symbols (corrects up to nsym/2 errors) |
| `payload_size` | 223 | Bytes per frame payload |

### Theoretical Throughput

```
bits_per_symbol = log2(M_FSK)
theoretical_bps = baud_rate × bits_per_symbol

Examples:
  1000 baud, M=4  → 2000 bps
  2000 baud, M=8  → 6000 bps
  5000 baud, M=16 → 20000 bps
```

## Testing

```bash
# Run all tests
python -m pytest tests/ -v

# Run specific test module
python -m pytest tests/test_modulation.py -v

# Run with coverage
python -m pytest tests/ -v --cov=src --cov-report=term-missing
```

## Project Structure

```
speed_of_sound_wifi/
├── src/
│   ├── config.py          # Master configuration
│   ├── main.py            # Entry point (GUI + headless)
│   ├── audio/
│   │   ├── io.py          # Audio stream management
│   │   └── devices.py     # Device enumeration
│   ├── physical/
│   │   ├── modulator.py   # M-FSK modulation
│   │   └── demodulator.py # M-FSK demodulation (FFT-based)
│   ├── link/
│   │   ├── fec.py         # Reed-Solomon FEC
│   │   ├── crc.py         # CRC-32 utilities
│   │   └── framing.py     # Frame assembly/parsing
│   ├── application/
│   │   ├── protocol.py    # File transfer protocol
│   │   └── fileio.py      # File I/O utilities
│   └── ui/
│       ├── main_window.py # Main application window
│       ├── waterfall.py   # Real-time waterfall display
│       └── controls.py    # UI controls and stats
├── tests/
│   ├── test_modulation.py # Physical layer tests
│   ├── test_fec.py        # FEC encoder/decoder tests
│   └── test_framing.py    # Frame protocol tests
├── requirements.txt
└── README.md
```

## Testing Baud Rate Limits

The primary goal is to find the maximum baud rate before link failure:

1. Start with loopback cable (audio out → mic in)
2. Set initial baud rate (e.g., 1000)
3. Transfer a test file
4. Increase baud rate incrementally
5. Monitor error rate and FEC corrections in the stats display
6. Note the baud rate where errors become uncorrectable

Factors affecting maximum baud rate:
- **Loopback cable**: Cleanest path, highest achievable baud rate
- **Speaker/microphone**: Room acoustics, distance, ambient noise
- **M-FSK order**: Higher M = more bits/symbol but closer frequency spacing
- **FEC strength**: More parity bytes = more error correction overhead
- **Sample rate**: Higher sample rate = finer frequency resolution

## License

MIT
