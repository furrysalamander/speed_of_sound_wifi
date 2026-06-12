# WASM Debug Tab — Build Plan

## Files Changed

### `sosw-core/src/config.rs`
- Add `#[derive(Serialize, Deserialize)]` to `Config`
- Add serde dependency to sosw-core Cargo.toml
- Add preset constructors: `high_baud()`, `robust()`, `ultrasonic()`, `ultrawide()`

### `sosw-web/src/audio.rs`
- `start_rx(config: &Config)` — accepts Config param
- `modulate_frame(data: &[u8], config: &Config)` — accepts Config param
- `build_test_signal(config: &Config, n_frames: usize) -> Vec<f32>` — test pattern generator
- `start_continuous_tx(samples: Vec<f32>, sample_rate: f32) -> TxPlayback` — looped playback

### `sosw-web/src/lib.rs` (refactored)
- App with 3 tabs: RX, TX, Debug
- Module declarations for rx, tx, debug, presets

### `sosw-web/src/rx.rs` (new)
- RxPanel (simple RX demo, unchanged from current lib.rs)

### `sosw-web/src/tx.rs` (new)  
- TxPanel (simple TX demo, unchanged from current lib.rs)

### `sosw-web/src/presets.rs` (new)
- Preset struct with name/description/config
- `load_saved_config() -> Config` / `save_config(cfg)` via localStorage
- Frequency/baud rate display helpers

### `sosw-web/src/debug.rs` (new — biggest file)
- **DebugPanel** — orchestrates config, monitor, and loopback sections
- **ConfigSection** — preset buttons, FFT/CP/SC controls, derived readout
- **MonitorSection** — enhanced stats, per-subcarrier bar chart, waterfall
- **LoopbackSection** — test pattern TX, continuous TX, loopback results

### `sosw-web/src/waterfall.rs`
- Add `set_active_band(sc_min, sc_max, fft_size)` method
- Draw translucent band overlay on waterfall
- Add frequency axis labels (Hz)

## Implementation Order

1. Config serde + presets (foundation)
2. audio.rs config acceptance (dependency for everything)
3. Split lib.rs → lib.rs + rx.rs + tx.rs (no-op refactor)
4. presets.rs (new)
5. waterfall.rs enhancements
6. debug.rs (new)
7. Build + verify 🎉

## Key Design Decisions

- **Config is a reactive `RwSignal<Config>`** owned by DebugPanel, passed down
- **Config changes trigger stop/rebuild/start** cycle for RX/TX
- **FrameParser** integration: demodulator output → FrameParser → ParsedFrame (gives CRC/FEC stats)
- **Continuous TX**: generate N frames of test data, play as looped AudioBuffer, use sequence numbers for tracking
- **Per-subcarrier chart**: separate small canvas overlaid on waterfall or alongside it
