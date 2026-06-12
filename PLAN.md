# Rust Rewrite Plan: Speed of Sound WiFi

## Architecture Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| FSK legacy code | **Dropped entirely** | Deprecated; all OTA results are OFDM-only. ~1500 LOC removed. |
| Web UI framework | **Leptos** | Fine-grained reactivity, Rust-idiomatic, good mobile perf, ~40KB bundle. |
| Desktop/testing target | **CLI with `cpal`** | Essential for OTA dev and CI testing. |
| Number of crates | **3**: `sosw-core`, `sosw-cli`, `sosw-web` | Clean split: engine / desktop CLI / web app. |
| RS FEC | **Custom GF(256) implementation** | Full control, no external dep risk. |
| FFT | `rustfft` with `wasm_simd` | WASM-compatible, power-of-2 optimized. |
| Numerics | `ndarray` + `num-complex` | Closest to NumPy idiom. 2x2 LS solved manually. |
| Audio on web | `ScriptProcessorNode` (fallback to `AudioWorklet`) | Simpler API for initial implementation. |
| Video decode on web | `ffmpeg.wasm` later | Post-MVP. Focus on data transport first. |

## Project Structure

```
speed_of_sound_wifi/
├── Cargo.toml                    # Workspace [sosw-core, sosw-cli]
├── AGENTS.md                     # Updated for Rust commands
│
├── sosw-core/
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs                # Re-exports, traits (Modulator, Demodulator)
│       ├── config.rs             # Config structs (port of config.py)
│       ├── physical/
│       │   ├── mod.rs
│       │   ├── qpsk.rs           # Gray QPSK map/demap
│       │   ├── preamble.rs       # Preamble gen (seed=42) + cross-correlation
│       │   ├── ofdm_mod.rs       # IFFT, CP, raised-cosine, normalize
│       │   └── ofdm_demod.rs     # Timing, channel est, DD phase track, demap
│       └── link/
│           ├── mod.rs
│           ├── gf256.rs          # GF(2^8) exp/log tables
│           ├── fec.rs            # RS(255,223) encoder + Berlekamp-Massey decoder
│           ├── crc.rs            # CRC-32 wrapper
│           ├── scrambler.rs      # ChaCha12 XOR scrambler (seed=12345)
│           └── frame.rs          # FrameAssembler, FrameParser
│
├── sosw-cli/
│   ├── Cargo.toml                # cpal, clap, sosw-core
│   └── src/
│       ├── main.rs               # CLI: tx/rx/test/list-devices
│       ├── audio.rs              # cpal AudioTx/AudioRx impl
│       ├── tx_cmd.rs             # TX: file -> modulate -> play
│       └── rx_cmd.rs             # RX: capture -> demod -> stdout/stats
│
├── sosw-web/
│   ├── Cargo.toml                # leptos, wasm-bindgen, web-sys, sosw-core
│   ├── index.html                # Trunk entry
│   └── src/
│       ├── lib.rs                # Leptos mount + wasm-bindgen init
│       ├── app.rs                # App shell, router
│       ├── pages/
│       │   ├── mod.rs
│       │   ├── home.rs           # Landing, mode selector
│       │   └── rx_demo.rs        # Capture, demod, stats, waterfall
│       ├── components/
│       │   ├── mod.rs
│       │   ├── waterfall.rs      # Canvas-based real-time spectrogram
│       │   └── stats_panel.rs    # Frame rate, drops, SNR, CFO, preamble peak
│       └── audio/
│           ├── mod.rs
│           ├── context.rs        # AudioContext init/management
│           └── rx.rs             # ScriptProcessorNode RX
│
├── tests/
│   ├── ofdm_roundtrip.rs         # Software loopback (single + multi-frame)
│   ├── fec_tests.rs              # RS encode/decode, error injection
│   └── framing_tests.rs          # Frame assemble/parse roundtrip
│
└── benches/
    └── ofdm_bench.rs             # criterion benchmarks
```

## Implementation Phases

| Phase | Scope | Est. LOC |
|-------|-------|----------|
| 1 | sosw-core: config, GF256, CRC, scrambler, QPSK | ~300 |
| 2 | sosw-core: preamble, OFDM modulator + demodulator | ~500 |
| 3 | sosw-core: RS FEC, frame assembler/parser | ~550 |
| 4 | sosw-cli: cpal audio, TX/RX modes, test mode | ~400 |
| 5 | sosw-web: Leptos app, ScriptProcessorNode, TX/RX pages | ~800 |
| 6 | sosw-web: test mode UI (waterfall, channel plot, drops) | ~300 |
| 7 | Polish: benchmarks, bundle audit, mobile testing | ~100 |
| **Total** | | **~2,950** |

## Performance Budget (WASM, no SIMD)

| Operation | Per-frame cost | Budget (258 ms) | Margin |
|-----------|---------------|-----------------|--------|
| Modulate (43 symbols) | ~200 µs | 258,000 µs | 1,290× |
| Demodulate (full frame) | ~500 µs | 258,000 µs | 516× |
| Frame parse + RS decode | ~100 µs | — | Ample |
| Total DSP per frame | ~800 µs | 258,000 µs | 322× |

Bottleneck is audio I/O, not DSP. Mobile WASM will be comfortable.

## Traits for Future Extensibility

```rust
pub trait Modulator {
    fn modulate_frame(&mut self, data: &[u8]) -> Vec<f32>;
}

pub trait Demodulator {
    fn process_samples(&mut self, samples: &[f32]) -> Option<DemodResult>;
}

pub struct DemodResult {
    pub bytes: Vec<u8>,
    pub preamble_peak: f32,
    pub cfo_rad_per_sym: f32,
    pub mean_h_magnitude: f32,
    pub per_sc_h: Vec<f32>,
}
```

## RS Cross-Validation Strategy

1. Export 100 test vectors from Python (random data -> RS encode -> output)
2. Store as `tests/fixtures/rs_test_vectors.json`
3. Verify Rust encoder produces identical parity bytes
4. Verify Rust decoder corrects 0..16 byte errors identically

## What Gets Dropped

- `src/physical/modulator.py` (FSK), `demodulator.py` (FSK)
- `src/application/` (file transfer protocol, FSK-dependent)
- `src/ui/` (PyQt6 GUI)
- `examples/stream_*.py`, `test_streaming.py`, `test_stream_pipe.py`
- `tests/test_modulation.py` (FSK tests)

## Build Commands

```bash
# Desktop
cargo build --release -p sosw-cli
cargo test
cargo bench

# Web
cd sosw-web && trunk serve         # Dev server
cd sosw-web && trunk build --release # Production build
```
