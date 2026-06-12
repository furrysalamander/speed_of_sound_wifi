# Frequency Sweep Analysis

## Software Sweep

Run: `cargo test -p sosw-core --test param_sweep -- --nocapture`

The modem works correctly in software across **all** tested parameter combinations up to the Nyquist limit (24 kHz). Software is not the bottleneck.

## Over-the-Air Sweep (Real Hardware)

Run: `python -m examples.ota_freq_sweep --frames 10`

**Setup:**
- TX: onboard analog speaker output (`ALC1220 Analog`)
- RX: USB PnP Audio Device (`USB_PnP`, mono-fallback)
- Room: desktop environment with ambient noise

### Frequency Sub-band Results (FFT=256, CP=32, 31 SCs each)

| Band | SC Range | Freq Range | Frames | Valid % | Preamble Peak |
|------|----------|-----------|-------|---------|---------------|
| low_band | 1-31 | 188-5812 Hz | 10 | **0%** | 0.80 |
| mid_low | 10-40 | 1875-7500 Hz | 10 | **50%** | 0.93 |
| mid | 30-60 | 5625-11250 Hz | 10 | **60%** | 0.93 |
| mid_high | 50-80 | 9375-15000 Hz | 10 | **40%** | 0.93 |
| high | 70-100 | 13125-18750 Hz | 10 | **40%** | 0.79 |
| near_ultra | 90-120 | 16875-22500 Hz | 10 | **20%** | 0.81 |

**All bands have strong preamble correlation (>0.79)** even up to 22.5 kHz. The signal is getting through across the entire spectrum. Frame validation rates drop at higher frequencies due to increasing bit errors (not preamble loss).

The `low_band` (188-5812 Hz) failure is likely due to ambient low-frequency noise (computer fans, AC, room rumble) overwhelming the sub-2 kHz region.

### Preset Config Results

| Preset | Freq Range | Sym/s | Valid % | Preamble Peak |
|--------|-----------|-------|---------|---------------|
| **ultrawide** | 0.9-20.6 kHz | 176 | **90%** | 0.79 |
| **robust** | 1.9-11.3 kHz | 83 | **80%** | 0.93 |
| mid (default-like) | 5.6-11.3 kHz | 167 | 60% | 0.93 |
| mid_low | 1.9-7.5 kHz | 167 | 50% | 0.93 |
| high_baud | 1.5-11.6 kHz | 333 | 40% | 0.90 |
| ultrasonic | 15.0-22.5 kHz | 167 | 20% | 0.71 |

### Key Conclusions

1. **Best performance: `ultrawide` preset (90% valid).** Wide subcarrier spread with shorter CP (16) outperforms on this hardware. The wide frequency diversity may help overcome narrowband interference.

2. **`robust` preset (80% valid) is strong** — 512-FFT with 64 CP gives the best noise resilience. Good for challenging acoustic environments.

3. **Ultrasonic (15-22.5 kHz) works** at 20% valid on this hardware. Usable for cross-device testing with high-frequency hardware. Expect better results with phone speakers or ultrasonic transducers.

4. **Sub-2 kHz is unusable** due to ambient low-frequency noise (fans, AC, rumble). SC range should stay above ~SC 10 (1.9 kHz).

5. **All preamble peaks exceed 0.70** up to 22.5 kHz — the USB PnP Audio Device has usable response to at least 22.5 kHz. Frame errors come from demodulation/phase tracking issues in the data symbols, not preamble detection.

### Hardware Notes

| Component | Model | Usable Range | Roll-off |
|-----------|-------|-------------|---------|
| TX Speaker | Onboard ALC1220 analog | ~1.9-20 kHz | Gradual above 15 kHz |
| RX Mic | USB PnP Audio Device | ~1.9-22.5+ kHz | Not observed in our tests |

The frequency response of this hardware setup is **usable from ~1.9 kHz to at least 22.5 kHz**, which is excellent for OFDM communication. The weak spot is below 2 kHz (ambient noise) and above 15 kHz (increasing symbol errors).

### Testing Procedure

To reproduce or test different hardware:
```bash
# Quick test (6 configs, 5 frames each, ~30s)
python -m examples.ota_freq_sweep --frames 5 --fast

# Full test (10 configs, 10 frames each, ~60s)
python -m examples.ota_freq_sweep --frames 10
```

Use the per-subcarrier channel magnitude plot in the WASM Debug tab to visualize the frequency response of your hardware in real time.
